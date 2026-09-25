use core::future::{poll_fn, Future};
use core::pin::{pin, Pin};
use core::task::Poll;

use super::{receive_frame, BleFrameReceiveError, BleReceiveError, BleSink, BleSource};

/// Receives whole BLE frames across an adapter's inbound boundary. Implementations must retain
/// the frame until forwarding completes; their pending future applies receive-side backpressure.
/// A refusal is a typed failure, not successful delivery. Use `Infallible` for an infallible seam.
#[allow(async_fn_in_trait)]
pub trait BleFrameForwarder {
    type Error;

    async fn forward(&mut self, frame: &[u8]) -> Result<(), Self::Error>;
}

/// Settled work, or an ingress failure that requires retiring the possibly partial transport.
#[derive(Debug, PartialEq, Eq)]
pub enum BleDuplexOutcome<ReceiveError, WorkError, ForwardError> {
    Finished(Result<(), WorkError>),
    ReceiveFailed(ReceiveError),
    InvalidReceiveLength(usize),
    /// Work may already have settled; adapters needing exact TX accounting record it at the sink.
    ForwardFailed(ForwardError),
}

enum Progress<W, R> {
    Finished(W),
    Received(R),
}

async fn prefer_work<W: Future, R: Future>(
    mut work: Pin<&mut W>,
    receive: R,
) -> Progress<W::Output, R::Output> {
    let mut receive = pin!(receive);
    poll_fn(|context| {
        if let Poll::Ready(result) = work.as_mut().poll(context) {
            return Poll::Ready(Progress::Finished(result));
        }
        receive.as_mut().poll(context).map(Progress::Received)
    })
    .await
}

/// Keep receiving whole frames during one uninterrupted send, using caller-owned storage.
/// Forwarding backpressure stops further reads but never stops polling the send. A completed
/// receive is forwarded or explicitly refused before returning, even if sending finishes first.
/// Empty frames are ignored.
///
/// `source` must preserve partial-frame progress when a pending receive is cancelled. A receive
/// or forwarding failure, or cancellation, can abandon a partial send; retire that transport
/// rather than retrying the same frame on it. No executor, allocator, clock, or queue is required.
pub async fn send_frame_duplex<Source: BleSource, Sink: BleSink, Forwarder: BleFrameForwarder>(
    source: &mut Source,
    sink: &mut Sink,
    outbound: &[u8],
    inbound: &mut [u8],
    forwarder: Forwarder,
) -> BleDuplexOutcome<Source::Error, Sink::Error, Forwarder::Error> {
    receive_frames_during(sink.send_frame(outbound), source, inbound, forwarder).await
}

/// Keep receiving until `work` settles, without cancelling a completed receive's forwarding.
/// The work may include a send and a wait for other peers; it remains polled during ingress
/// backpressure. Completion wins over starting another receive, and empty frames are ignored.
///
/// Pending receives must preserve partial-frame progress on cancellation. Ingress failure
/// cancels the work, so callers must retire any possibly partial transport. External cancellation
/// can also abandon forwarding; adapters must track that boundary when deciding which peers to
/// retire. Uses caller-owned storage and requires no executor, allocator, clock, or queue.
pub async fn receive_frames_during<
    Work: Future<Output = Result<(), WorkError>>,
    WorkError,
    Source: BleSource,
    Forwarder: BleFrameForwarder,
>(
    work: Work,
    source: &mut Source,
    inbound: &mut [u8],
    mut forwarder: Forwarder,
) -> BleDuplexOutcome<Source::Error, WorkError, Forwarder::Error> {
    let mut work = pin!(work);
    loop {
        let frame = match prefer_work(work.as_mut(), receive_frame(source, inbound)).await {
            Progress::Finished(result) => return BleDuplexOutcome::Finished(result),
            Progress::Received(Ok([])) => continue,
            Progress::Received(Ok(frame)) => frame,
            Progress::Received(Err(BleFrameReceiveError::Length(
                BleReceiveError::BufferTooSmall { length, .. },
            ))) => {
                return BleDuplexOutcome::InvalidReceiveLength(length);
            }
            Progress::Received(Err(BleFrameReceiveError::Source(error))) => {
                return BleDuplexOutcome::ReceiveFailed(error)
            }
        };
        let mut forwarding = pin!(forwarder.forward(frame));
        match prefer_work(work.as_mut(), forwarding.as_mut()).await {
            Progress::Finished(result) => {
                if let Err(error) = forwarding.await {
                    return BleDuplexOutcome::ForwardFailed(error);
                }
                return BleDuplexOutcome::Finished(result);
            }
            Progress::Received(Ok(())) => {}
            Progress::Received(Err(error)) => return BleDuplexOutcome::ForwardFailed(error),
        }
    }
}

#[cfg(test)]
mod tests;
