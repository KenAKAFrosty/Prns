//! The serve loop every framed byte-stream interface shares under tokio: read a wire, deframe
//! straight into the seam's granted inbound slot, frame the seam's outbound back down, generic
//! over a [`Framing`] codec.
//! Serial, TCP, and the shared-instance link pass [`HdlcFraming`]; KISS and AX.25 pass
//! [`KissFraming`]. An interface owns a [`FramedBuffers`] and lends it to [`serve`] per
//! connection, reused across reconnects and never re-allocated; `serve` resets the deframer on
//! entry to discard any half-frame an earlier drop left mid-stream.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use prns_core::engine::InstantMillis;
#[cfg(any(feature = "kiss", feature = "ax25", feature = "tcp"))]
use prns_core::interfaces::kiss_framing::{self, KissScanner};
#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
use prns_core::interfaces::rns_serial_framing::{self, RnsSerialScanner};
use prns_core::interfaces::{BitrateBps, FrameSink};
use prns_runtime::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use prns_runtime::reactor::interface_seam::InterfaceSeam;
use prns_runtime::reactor::throughput::ThroughputLedger;

/// A streaming deframer [`serve`] drives over one connection: built fresh, reset between
/// connections, then fed wire bytes a chunk at a time, writing each frame into the caller's
/// [`FrameSink`] — the seam's granted slot, so the frame bytes land once, already across the
/// seam. Each framing's scanner implements it, so the serve loop names only this contract, not
/// a concrete scanner.
pub trait StreamDeframer {
    fn new() -> Self;
    fn reset(&mut self);
    /// The next complete frame at or after `*offset` in `input`, its payload left in `sink`,
    /// advancing `offset` past the bytes consumed. `Some(len)` is the payload length now in the
    /// sink (`0` is a delimiter-only keepalive); `None` when the chunk is exhausted — mid-frame
    /// the partial payload stays accumulated in the sink for the next chunk — or when a
    /// malformed or oversized frame was swallowed (the scanner clears the sink, self-heals, and
    /// realigns at the next delimiter), in which case `offset` has still advanced so the
    /// caller's loop makes progress.
    fn next_frame_into(
        &mut self,
        input: &[u8],
        offset: &mut usize,
        sink: &mut dyn FrameSink,
    ) -> Option<usize>;
}

/// The wire framing a byte-stream interface speaks — a scanner paired with its encoder, named
/// by a zero-sized marker; the frame ceiling is the sink's (the seam sizes its slots from the
/// interface descriptor), and the encoder is a stateless associated function.
pub trait Framing {
    type Deframer: StreamDeframer;
    /// Frame `input` into `output`, returning the encoded length, or `None` if `output` is too
    /// small (the caller sizes `output` to the framing's worst case, so this does not happen in
    /// practice; a too-small buffer just drops the frame rather than panicking).
    fn encode(input: &[u8], output: &mut [u8]) -> Option<usize>;
}

/// RNS HDLC-like serial framing (`0x7E` flag, `0x7D` escape) — what serial, TCP, and the
/// shared-instance link speak.
#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
pub struct HdlcFraming;

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
impl StreamDeframer for RnsSerialScanner {
    fn new() -> Self {
        RnsSerialScanner::new()
    }

    fn reset(&mut self) {
        RnsSerialScanner::reset(self);
    }

    fn next_frame_into(
        &mut self,
        input: &[u8],
        offset: &mut usize,
        sink: &mut dyn FrameSink,
    ) -> Option<usize> {
        RnsSerialScanner::next_frame_into(self, input, offset, sink)
            .ok()
            .flatten()
    }
}

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
impl Framing for HdlcFraming {
    type Deframer = RnsSerialScanner;

    fn encode(input: &[u8], output: &mut [u8]) -> Option<usize> {
        rns_serial_framing::encode(input, output).ok()
    }
}

/// KISS TNC framing (`0xC0` FEND) — what the KISS and AX.25 interfaces speak.
#[cfg(any(feature = "kiss", feature = "ax25", feature = "tcp"))]
pub struct KissFraming;

#[cfg(any(feature = "kiss", feature = "ax25", feature = "tcp"))]
impl StreamDeframer for KissScanner {
    fn new() -> Self {
        KissScanner::new()
    }

    fn reset(&mut self) {
        KissScanner::reset(self);
    }

    fn next_frame_into(
        &mut self,
        input: &[u8],
        offset: &mut usize,
        sink: &mut dyn FrameSink,
    ) -> Option<usize> {
        KissScanner::next_frame_into(self, input, offset, sink)
            .ok()
            .flatten()
    }
}

#[cfg(any(feature = "kiss", feature = "ax25", feature = "tcp"))]
impl Framing for KissFraming {
    type Deframer = KissScanner;

    fn encode(input: &[u8], output: &mut [u8]) -> Option<usize> {
        kiss_framing::encode(input, output).ok()
    }
}

/// The reusable scratch a framed serve loop works in: the deframer's scan state and the read
/// and outbound-frame buffers, heap-held so no megabyte of buffer ever rides the stack. An
/// interface lends one to [`serve`] per connection (allocated once across reconnects; a target
/// that never answers, holding one behind an `Option`, never allocates at all).
/// Inbound frames accumulate in the seam's granted slot, not here — the deframer carries no
/// frame buffer of its own.
pub struct FramedBuffers<F, const READ_LEN: usize, const FRAMED_LEN: usize>
where
    F: Framing,
{
    deframer: F::Deframer,
    read_buf: std::boxed::Box<[u8]>,
    frame_buf: std::boxed::Box<[u8]>,
}

impl<F, const READ_LEN: usize, const FRAMED_LEN: usize> Default
    for FramedBuffers<F, READ_LEN, FRAMED_LEN>
where
    F: Framing,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<F, const READ_LEN: usize, const FRAMED_LEN: usize> FramedBuffers<F, READ_LEN, FRAMED_LEN>
where
    F: Framing,
{
    pub fn new() -> Self {
        Self {
            deframer: <F::Deframer as StreamDeframer>::new(),
            read_buf: std::vec![0u8; READ_LEN].into_boxed_slice(),
            frame_buf: std::vec![0u8; FRAMED_LEN].into_boxed_slice(),
        }
    }
}

/// Serve one connection until the stream drops: read bytes and deframe them up to the seam,
/// drain the seam and frame outbound onto the wire. Returns on any IO error so the caller
/// can reconnect. The deframer and buffers are the caller's [`FramedBuffers`], reset on entry and
/// reused across reconnects. The framing `F` is the same the buffers were minted with.
/// An outbound burst coalesces: frames already queued behind the one being written encode into
/// the same buffer and leave in one wire write, flushing at most twice per wake so a saturating
/// sender cannot starve the read half.
/// The per-connection accounting one served stream reports into: the shared
/// status handle, the airtime and throughput ledgers, the nominal bitrate that
/// prices a frame's airtime, and the serve epoch the ledgers' clocks count from.
pub struct WireMeters<'a> {
    pub status: &'a TokioInterfaceStatus,
    pub airtime: &'a mut AirtimeLedger,
    pub throughput: &'a mut ThroughputLedger,
    pub bitrate: BitrateBps,
    pub started: tokio::time::Instant,
}

pub async fn serve<F, const READ_LEN: usize, const FRAMED_LEN: usize, S, Seam>(
    mut stream: S,
    buffers: &mut FramedBuffers<F, READ_LEN, FRAMED_LEN>,
    seam: &mut Seam,
    meters: &mut WireMeters<'_>,
) where
    F: Framing,
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    let FramedBuffers {
        deframer,
        read_buf,
        frame_buf,
    } = buffers;
    let WireMeters {
        status,
        airtime,
        throughput,
        bitrate,
        started,
    } = meters;
    let (bitrate, started) = (*bitrate, *started);
    deframer.reset();
    let read_buf: &mut [u8] = read_buf;
    let frame_buf: &mut [u8] = frame_buf;

    loop {
        tokio::select! {
            read = stream.read(&mut *read_buf) => {
                let read = match read {
                    Ok(0) | Err(_) => return,
                    Ok(read) => read,
                };
                status.add_rx(read as u64);
                let now = InstantMillis(started.elapsed().as_millis() as u64);
                throughput.record_rx(now, read as u64);
                status.set_transfer_rates(throughput.rates());
                let mut offset = 0;
                let chunk = &read_buf[..read];
                while offset < chunk.len() {
                    let sink = seam.inbound_sink().await;
                    if deframer.next_frame_into(chunk, &mut offset, sink).is_some() {
                        seam.commit_inbound().await;
                    }
                }
            }
            outbound = seam.next_outbound() => {
                let Some(mut filled) = F::encode(outbound, &mut *frame_buf) else {
                    continue;
                };
                let mut record_tx_write = |written: usize| {
                    status.add_tx(written as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_tx(now, written as u64);
                    status.set_transfer_rates(throughput.rates());
                    status.set_airtime(airtime.record_tx(now, frame_airtime_us(written, bitrate)));
                };
                while let Some(next) = seam.try_next_outbound() {
                    if let Some(more) = F::encode(next, &mut frame_buf[filled..]) {
                        filled += more;
                        continue;
                    }
                    if stream.write_all(&frame_buf[..filled]).await.is_err() {
                        return;
                    }
                    record_tx_write(filled);
                    filled = F::encode(next, &mut *frame_buf).unwrap_or(0);
                    break;
                }
                if filled > 0 {
                    if stream.write_all(&frame_buf[..filled]).await.is_err() {
                        return;
                    }
                    record_tx_write(filled);
                }
            }
        }
    }
}

#[cfg(all(test, feature = "tcp"))]
mod tests {
    use super::*;
    use prns_core::interfaces::rns_serial_framing::RnsSerialDecoder;
    use prns_core::interfaces::{ConnectionState, InterfaceId};
    use prns_runtime::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};

    struct LaneSeam {
        outbound: TokioGrantConsumer,
        inbound: std::vec::Vec<u8>,
    }

    impl InterfaceSeam for LaneSeam {
        async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
            &mut self.inbound
        }

        async fn commit_inbound(&mut self) {
            self.inbound.clear();
        }

        async fn next_outbound(&mut self) -> &[u8] {
            self.outbound.release();
            self.outbound.peek().await.frame()
        }

        fn try_next_outbound(&mut self) -> Option<&[u8]> {
            self.outbound.release();
            Some(self.outbound.try_peek()?.frame())
        }
    }

    struct WriteCounting<S> {
        stream: S,
        writes: Arc<AtomicUsize>,
    }

    impl<S: AsyncRead + Unpin> AsyncRead for WriteCounting<S> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.stream).poll_read(cx, buf)
        }
    }

    impl<S: AsyncWrite + Unpin> AsyncWrite for WriteCounting<S> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            Pin::new(&mut self.stream).poll_write(cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.stream).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.stream).poll_shutdown(cx)
        }
    }

    #[tokio::test]
    async fn a_queued_outbound_burst_leaves_in_one_wire_write() {
        let (mut producer, consumer) = tokio_grant_lane(64, 8);
        let payloads: [&[u8]; 3] = [b"first frame", b"second frame", b"third frame"];
        for payload in payloads {
            producer
                .try_grant()
                .expect("lane has free slots")
                .fill(payload);
            producer.commit();
        }

        let (near, mut far) = tokio::io::duplex(64 * 1024);
        let writes = Arc::new(AtomicUsize::new(0));
        let counted = WriteCounting {
            stream: near,
            writes: writes.clone(),
        };

        let served = tokio::spawn(async move {
            let mut buffers = FramedBuffers::<HdlcFraming, 4096, 8192>::new();
            let mut seam = LaneSeam {
                outbound: consumer,
                inbound: std::vec::Vec::new(),
            };
            let status =
                TokioInterfaceStatus::new(InterfaceId::new([7u8; 8]), ConnectionState::Connected);
            let mut airtime = AirtimeLedger::default();
            let mut throughput = ThroughputLedger::new();
            let mut meters = WireMeters {
                status: &status,
                airtime: &mut airtime,
                throughput: &mut throughput,
                bitrate: BitrateBps::guess(1_000_000),
                started: tokio::time::Instant::now(),
            };
            serve(counted, &mut buffers, &mut seam, &mut meters).await;
        });

        let mut decoder = RnsSerialDecoder::<4096>::new();
        let mut decoded: std::vec::Vec<std::vec::Vec<u8>> = std::vec::Vec::new();
        let mut buf = [0u8; 4096];
        while decoded.len() < payloads.len() {
            let read = tokio::io::AsyncReadExt::read(&mut far, &mut buf)
                .await
                .expect("reads from the wire");
            assert_ne!(read, 0, "the wire stays up while frames are owed");
            let mut offset = 0;
            while offset < read {
                if let Ok(Some(frame)) = decoder.feed_slice_next(&buf[..read], &mut offset) {
                    if !frame.is_empty() {
                        decoded.push(frame.to_vec());
                    }
                }
            }
        }
        assert_eq!(decoded, payloads.map(<[u8]>::to_vec));
        assert_eq!(
            writes.load(Ordering::Relaxed),
            1,
            "the queued burst coalesced into a single wire write",
        );

        drop(far);
        served.await.expect("the serve loop returns on stream drop");
    }
}
