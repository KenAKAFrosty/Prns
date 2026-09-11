use tokio::io::{AsyncRead, AsyncWrite};

use prns_core::engine::InstantMillis;
#[cfg(feature = "i2p")]
use prns_core::interfaces::i2p::{I2pIdleWatchdog, WATCHDOG_TICK_INTERVAL};
use prns_core::interfaces::i2p::{I2pReadObservation, I2pWatchdogVerdict, HDLC_KEEPALIVE};
#[cfg(any(feature = "kiss", feature = "ax25", feature = "tcp"))]
use prns_core::interfaces::kiss_framing::{self, KissScanner};
#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone",
    feature = "i2p"
))]
use prns_core::interfaces::rns_serial_framing::{self, RnsSerialScanner};
use prns_core::interfaces::{BitrateBps, FrameSink, InterfaceStatus};
#[cfg(any(target_os = "windows", test))]
use prns_core::interfaces::{BROADCAST_WIRE_FRAME_LEN, IFAC_MAX_SIZE};
use prns_core::units::DurationMillis;
#[cfg(any(target_os = "windows", test))]
use prns_core::wire::{WireContext, WirePacketHeader};
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::InterfaceSeam;
use prns_runtime::manifold::throughput::ThroughputLedger;

const OUTBOUND_BATCH_TARGET_BYTES: usize = 256 * 1024;
#[cfg(any(target_os = "windows", test))]
const RESOURCE_WINDOW_BATCH_TARGET_BYTES: usize = 768 * 1024;
#[cfg(any(target_os = "windows", test))]
const RESOURCE_WINDOW_BATCH_FRAME_MAX_BYTES: usize = 8 * 1024 + IFAC_MAX_SIZE;

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, PartialEq, Eq)]
enum OutboundBatchClass {
    LatencyBounded,
    WindowBatchableResource,
    CompatibilitySizedResource,
}

#[cfg(any(target_os = "windows", test))]
fn outbound_batch_class(frame: &[u8]) -> OutboundBatchClass {
    match WirePacketHeader::parse(frame) {
        Ok((header, _))
            if header.context == WireContext::Resource
                && frame.len() <= BROADCAST_WIRE_FRAME_LEN =>
        {
            OutboundBatchClass::CompatibilitySizedResource
        }
        Ok((header, _))
            if header.context == WireContext::Resource
                && frame.len() <= RESOURCE_WINDOW_BATCH_FRAME_MAX_BYTES =>
        {
            OutboundBatchClass::WindowBatchableResource
        }
        Ok(_) | Err(_) => OutboundBatchClass::LatencyBounded,
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_outbound_batch_target_bytes(frame: &[u8], buffer_capacity: usize) -> usize {
    match outbound_batch_class(frame) {
        OutboundBatchClass::LatencyBounded => OUTBOUND_BATCH_TARGET_BYTES.min(buffer_capacity),
        OutboundBatchClass::WindowBatchableResource => {
            RESOURCE_WINDOW_BATCH_TARGET_BYTES.min(buffer_capacity)
        }
        OutboundBatchClass::CompatibilitySizedResource => buffer_capacity,
    }
}

#[cfg(target_os = "windows")]
fn outbound_batch_target_bytes(frame: &[u8], buffer_capacity: usize) -> usize {
    windows_outbound_batch_target_bytes(frame, buffer_capacity)
}

#[cfg(not(target_os = "windows"))]
fn outbound_batch_target_bytes(_frame: &[u8], buffer_capacity: usize) -> usize {
    OUTBOUND_BATCH_TARGET_BYTES.min(buffer_capacity)
}

pub trait StreamDeframer {
    fn new() -> Self;
    fn reset(&mut self);
    /// The next framing outcome at or after `*offset` in `input`, advancing `offset` past the
    /// bytes consumed. A completed frame leaves its payload in `sink`; a rejected frame clears
    /// the sink and self-heals at the next delimiter.
    fn next_frame_into(
        &mut self,
        input: &[u8],
        offset: &mut usize,
        sink: &mut dyn FrameSink,
    ) -> StreamDeframeOutcome;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDeframeOutcome {
    AwaitingInput,
    Frame { len: usize },
    Rejected,
}

pub trait Framing {
    type Deframer: StreamDeframer;
    fn encode(input: &[u8], output: &mut [u8]) -> Option<usize>;
}

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone",
    feature = "i2p"
))]
pub struct HdlcFraming;

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone",
    feature = "i2p"
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
    ) -> StreamDeframeOutcome {
        match RnsSerialScanner::next_frame_into(self, input, offset, sink) {
            Ok(Some(len)) => StreamDeframeOutcome::Frame { len },
            Ok(None) => StreamDeframeOutcome::AwaitingInput,
            Err(_) => StreamDeframeOutcome::Rejected,
        }
    }
}

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone",
    feature = "i2p"
))]
impl Framing for HdlcFraming {
    type Deframer = RnsSerialScanner;

    fn encode(input: &[u8], output: &mut [u8]) -> Option<usize> {
        rns_serial_framing::encode(input, output).ok()
    }
}

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
    ) -> StreamDeframeOutcome {
        match KissScanner::next_frame_into(self, input, offset, sink) {
            Ok(Some(len)) => StreamDeframeOutcome::Frame { len },
            Ok(None) => StreamDeframeOutcome::AwaitingInput,
            Err(_) => StreamDeframeOutcome::Rejected,
        }
    }
}

#[cfg(any(feature = "kiss", feature = "ax25", feature = "tcp"))]
impl Framing for KissFraming {
    type Deframer = KissScanner;

    fn encode(input: &[u8], output: &mut [u8]) -> Option<usize> {
        kiss_framing::encode(input, output).ok()
    }
}

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

pub struct WireMeters<'a> {
    pub status: &'a TokioInterfaceStatus,
    pub airtime: &'a mut AirtimeLedger,
    pub throughput: &'a mut ThroughputLedger,
    pub bitrate: BitrateBps,
    pub started: tokio::time::Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WriteProgressTimeout(std::time::Duration);

impl WriteProgressTimeout {
    #[cfg(any(feature = "tcp", test))]
    pub(crate) const fn after(duration: std::time::Duration) -> Self {
        Self(duration)
    }

    fn deadline_from(self, now: tokio::time::Instant) -> tokio::time::Instant {
        now + self.0
    }
}

trait StreamWatchdog {
    fn observe_read(&mut self, now: InstantMillis) -> I2pReadObservation;
    fn observe_ordinary_write(&mut self, now: InstantMillis);
    async fn wait_for_tick(&mut self, started: tokio::time::Instant) -> I2pWatchdogVerdict;
}

#[derive(Clone, Copy)]
enum StreamPollOrder {
    ReadFirst,
    WriteFirst,
}

enum StreamProgress {
    Read(std::io::Result<usize>),
    Write(std::io::Result<usize>),
}

#[derive(Clone, Copy)]
enum PendingWriteClass {
    Ordinary,
    Keepalive,
}

struct PendingWrite {
    len: usize,
    written: usize,
    class: PendingWriteClass,
    progress_deadline: Option<tokio::time::Instant>,
}

impl PendingWrite {
    fn new(len: usize, class: PendingWriteClass, timeout: Option<WriteProgressTimeout>) -> Self {
        Self {
            len,
            written: 0,
            class,
            progress_deadline: timeout
                .map(|timeout| timeout.deadline_from(tokio::time::Instant::now())),
        }
    }

    fn advance(
        &mut self,
        written: usize,
        timeout: Option<WriteProgressTimeout>,
    ) -> Option<(PendingWriteClass, usize)> {
        self.written += written;
        self.progress_deadline =
            timeout.map(|timeout| timeout.deadline_from(tokio::time::Instant::now()));
        (self.written == self.len).then_some((self.class, self.len))
    }
}

fn poll_stream_read<S: AsyncRead + Unpin>(
    stream: &mut S,
    read_buf: &mut [u8],
    cx: &mut std::task::Context<'_>,
) -> std::task::Poll<std::io::Result<usize>> {
    let mut destination = tokio::io::ReadBuf::new(read_buf);
    match std::pin::Pin::new(stream).poll_read(cx, &mut destination) {
        std::task::Poll::Ready(Ok(())) => std::task::Poll::Ready(Ok(destination.filled().len())),
        std::task::Poll::Ready(Err(error)) => std::task::Poll::Ready(Err(error)),
        std::task::Poll::Pending => std::task::Poll::Pending,
    }
}

fn poll_stream_write<S: AsyncWrite + Unpin>(
    stream: &mut S,
    write_buf: &[u8],
    cx: &mut std::task::Context<'_>,
) -> std::task::Poll<std::io::Result<usize>> {
    std::pin::Pin::new(stream).poll_write(cx, write_buf)
}

async fn next_stream_progress<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    read_buf: &mut [u8],
    write_buf: Option<&[u8]>,
    poll_order: StreamPollOrder,
) -> StreamProgress {
    std::future::poll_fn(|cx| match (poll_order, write_buf) {
        (StreamPollOrder::ReadFirst, Some(write_buf)) => {
            match poll_stream_read(stream, read_buf, cx) {
                std::task::Poll::Ready(read) => std::task::Poll::Ready(StreamProgress::Read(read)),
                std::task::Poll::Pending => {
                    poll_stream_write(stream, write_buf, cx).map(StreamProgress::Write)
                }
            }
        }
        (StreamPollOrder::WriteFirst, Some(write_buf)) => {
            match poll_stream_write(stream, write_buf, cx) {
                std::task::Poll::Ready(written) => {
                    std::task::Poll::Ready(StreamProgress::Write(written))
                }
                std::task::Poll::Pending => {
                    poll_stream_read(stream, read_buf, cx).map(StreamProgress::Read)
                }
            }
        }
        (_, None) => poll_stream_read(stream, read_buf, cx).map(StreamProgress::Read),
    })
    .await
}

fn keepalive_write(frame_buf: &mut [u8], timeout: Option<WriteProgressTimeout>) -> PendingWrite {
    frame_buf[..HDLC_KEEPALIVE.len()].copy_from_slice(&HDLC_KEEPALIVE);
    PendingWrite::new(HDLC_KEEPALIVE.len(), PendingWriteClass::Keepalive, timeout)
}

fn record_tx_write(
    status: &TokioInterfaceStatus,
    throughput: &mut ThroughputLedger,
    airtime: &mut AirtimeLedger,
    started: tokio::time::Instant,
    bitrate: BitrateBps,
    written: usize,
) {
    status.add_tx(written as u64);
    let now = InstantMillis(started.elapsed().as_millis() as u64);
    throughput.record_tx(now, written as u64);
    status.set_transfer_rates(throughput.rates());
    status.set_airtime(airtime.record_tx(now, frame_airtime_us(written, bitrate)));
}

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "kiss",
    feature = "ax25",
    feature = "rnode",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
struct NoIdleWatchdog;

#[cfg(any(
    feature = "tcp",
    feature = "serial",
    feature = "kiss",
    feature = "ax25",
    feature = "rnode",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
))]
impl StreamWatchdog for NoIdleWatchdog {
    fn observe_read(&mut self, _now: InstantMillis) -> I2pReadObservation {
        I2pReadObservation::Responsive
    }

    fn observe_ordinary_write(&mut self, _now: InstantMillis) {}

    async fn wait_for_tick(&mut self, _started: tokio::time::Instant) -> I2pWatchdogVerdict {
        std::future::pending().await
    }
}

#[cfg(feature = "i2p")]
struct I2pStreamWatchdog {
    state: I2pIdleWatchdog,
    interval: tokio::time::Interval,
}

#[cfg(feature = "i2p")]
impl I2pStreamWatchdog {
    fn start(now: InstantMillis) -> Self {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(WATCHDOG_TICK_INTERVAL.0));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Self {
            state: I2pIdleWatchdog::start(now),
            interval,
        }
    }
}

#[cfg(feature = "i2p")]
impl StreamWatchdog for I2pStreamWatchdog {
    fn observe_read(&mut self, now: InstantMillis) -> I2pReadObservation {
        self.state.observe_read(now)
    }

    fn observe_ordinary_write(&mut self, now: InstantMillis) {
        self.state.observe_ordinary_write(now);
    }

    async fn wait_for_tick(&mut self, started: tokio::time::Instant) -> I2pWatchdogVerdict {
        self.interval.tick().await;
        self.state.tick(elapsed_millis(started))
    }
}

#[cfg(any(
    feature = "serial",
    feature = "kiss",
    feature = "ax25",
    feature = "rnode",
    feature = "pipe",
    feature = "shared-instance",
    test
))]
pub async fn serve<F, const READ_LEN: usize, const FRAMED_LEN: usize, S, Seam>(
    stream: S,
    buffers: &mut FramedBuffers<F, READ_LEN, FRAMED_LEN>,
    seam: &mut Seam,
    meters: &mut WireMeters<'_>,
) where
    F: Framing,
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    serve_inner(stream, buffers, seam, meters, NoIdleWatchdog, None).await;
}

#[cfg(any(
    feature = "tcp",
    feature = "backbone",
    feature = "wifi-aware",
    feature = "wifi-direct"
))]
pub(crate) async fn serve_with_write_progress_timeout<
    F,
    const READ_LEN: usize,
    const FRAMED_LEN: usize,
    S,
    Seam,
>(
    stream: S,
    buffers: &mut FramedBuffers<F, READ_LEN, FRAMED_LEN>,
    seam: &mut Seam,
    meters: &mut WireMeters<'_>,
    timeout: WriteProgressTimeout,
) where
    F: Framing,
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    serve_inner(stream, buffers, seam, meters, NoIdleWatchdog, Some(timeout)).await;
}

#[cfg(feature = "i2p")]
pub(crate) async fn serve_with_hdlc_idle_watchdog<
    const READ_LEN: usize,
    const FRAMED_LEN: usize,
    S,
    Seam,
>(
    stream: S,
    buffers: &mut FramedBuffers<HdlcFraming, READ_LEN, FRAMED_LEN>,
    seam: &mut Seam,
    meters: &mut WireMeters<'_>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
{
    let now = elapsed_millis(meters.started);
    serve_inner(
        stream,
        buffers,
        seam,
        meters,
        I2pStreamWatchdog::start(now),
        None,
    )
    .await;
}

async fn serve_inner<F, const READ_LEN: usize, const FRAMED_LEN: usize, S, Seam, Watchdog>(
    mut stream: S,
    buffers: &mut FramedBuffers<F, READ_LEN, FRAMED_LEN>,
    seam: &mut Seam,
    meters: &mut WireMeters<'_>,
    mut watchdog: Watchdog,
    write_progress_timeout: Option<WriteProgressTimeout>,
) where
    F: Framing,
    S: AsyncRead + AsyncWrite + Unpin,
    Seam: InterfaceSeam,
    Watchdog: StreamWatchdog,
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
    let mut pending_write: Option<PendingWrite> = None;
    let mut deferred_outbound: Option<std::vec::Vec<u8>> = None;
    let mut poll_order = StreamPollOrder::WriteFirst;

    loop {
        if pending_write.is_none() {
            if let Some(outbound) = deferred_outbound.take() {
                pending_write = F::encode(&outbound, frame_buf).map(|len| {
                    PendingWrite::new(len, PendingWriteClass::Ordinary, write_progress_timeout)
                });
            }
        }
        let write_buf = pending_write
            .as_ref()
            .map(|pending| &frame_buf[pending.written..pending.len]);
        let write_progress_deadline = pending_write
            .as_ref()
            .and_then(|pending| pending.progress_deadline);
        tokio::select! {
            progress = next_stream_progress(&mut stream, read_buf, write_buf, poll_order) => {
                let read = match progress {
                    StreamProgress::Read(read) => {
                        poll_order = StreamPollOrder::WriteFirst;
                        read
                    }
                    StreamProgress::Write(written) => {
                        poll_order = StreamPollOrder::ReadFirst;
                        let written = match written {
                            Ok(0) | Err(_) => return,
                            Ok(written) => written,
                        };
                        let completed = {
                            let Some(pending) = pending_write.as_mut() else {
                                return;
                            };
                            pending.advance(written, write_progress_timeout)
                        };
                        if let Some((class, written)) = completed {
                            pending_write = None;
                            if matches!(class, PendingWriteClass::Ordinary) {
                                watchdog.observe_ordinary_write(elapsed_millis(started));
                                record_tx_write(
                                    status,
                                    throughput,
                                    airtime,
                                    started,
                                    bitrate,
                                    written,
                                );
                            }
                        }
                        continue;
                    }
                };
                let read = match read {
                    Ok(0) | Err(_) => return,
                    Ok(read) => read,
                };
                let now = elapsed_millis(started);
                if matches!(
                    watchdog.observe_read(now),
                    I2pReadObservation::Recovered
                ) {
                    status.set_connection(prns_core::interfaces::ConnectionState::Connected);
                }
                status.add_rx(read as u64);
                throughput.record_rx(now, read as u64);
                status.set_transfer_rates(throughput.rates());
                let mut offset = 0;
                let chunk = &read_buf[..read];
                while offset < chunk.len() {
                    let sink = seam.inbound_sink().await;
                    match deframer.next_frame_into(chunk, &mut offset, sink) {
                        StreamDeframeOutcome::AwaitingInput => {}
                        StreamDeframeOutcome::Frame { len: 0 } => {}
                        StreamDeframeOutcome::Frame { len: _ } => {
                            status.count_frame_in();
                            seam.commit_inbound().await;
                            status.count_frame_delivered();
                        }
                        StreamDeframeOutcome::Rejected => status.count_frame_undecodable(),
                    }
                }
            }
            outbound = seam.next_outbound(), if pending_write.is_none() => {
                let batch_target = outbound_batch_target_bytes(outbound, frame_buf.len());
                let Some(mut filled) = F::encode(outbound, &mut *frame_buf) else {
                    continue;
                };
                while filled < batch_target {
                    let Some(next) = seam.try_next_outbound() else {
                        break;
                    };
                    if let Some(more) = F::encode(next, &mut frame_buf[filled..]) {
                        filled += more;
                        continue;
                    }
                    deferred_outbound = Some(next.to_vec());
                    break;
                }
                if filled > 0 {
                    pending_write = Some(PendingWrite::new(
                        filled,
                        PendingWriteClass::Ordinary,
                        write_progress_timeout,
                    ));
                }
            }
            verdict = watchdog.wait_for_tick(started), if pending_write.is_none() => {
                match verdict {
                    I2pWatchdogVerdict::Continue => {}
                    I2pWatchdogVerdict::Degrade => {
                        status.set_connection(prns_core::interfaces::ConnectionState::Degraded);
                    }
                    I2pWatchdogVerdict::TransmitKeepalive => {
                        pending_write = Some(keepalive_write(frame_buf, write_progress_timeout));
                    }
                    I2pWatchdogVerdict::DegradeAndTransmitKeepalive => {
                        status.set_connection(prns_core::interfaces::ConnectionState::Degraded);
                        pending_write = Some(keepalive_write(frame_buf, write_progress_timeout));
                    }
                    I2pWatchdogVerdict::Disconnect => return,
                }
            }
            () = wait_for_write_progress_deadline(write_progress_deadline), if write_progress_deadline.is_some() => {
                if let Some(timeout) = write_progress_timeout {
                    let timeout_ms = timeout.0.as_millis();
                    crate::diagnostic_log::warn!(
                        "byte-stream interface {:?} made no write progress for {timeout_ms} ms; disconnecting",
                        status.id().as_bytes(),
                    );
                }
                return;
            }
        }
    }
}

async fn wait_for_write_progress_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn elapsed_millis(started: tokio::time::Instant) -> InstantMillis {
    InstantMillis(DurationMillis::from_duration_saturating(started.elapsed()).0)
}

#[cfg(all(test, feature = "tcp"))]
mod tests {
    use super::*;
    use prns_core::interfaces::rns_serial_framing::RnsSerialDecoder;
    use prns_core::interfaces::{ConnectionState, FrameSinkError, InterfaceId};
    use prns_core::wire::HEADER_MIN_LEN;
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};

    struct LaneSeam {
        outbound: TokioGrantConsumer,
        inbound: std::vec::Vec<u8>,
        inbound_commits: Arc<AtomicUsize>,
    }

    #[derive(Default)]
    struct OneByteSink(std::vec::Vec<u8>);

    impl FrameSink for OneByteSink {
        fn clear(&mut self) {
            self.0.clear();
        }

        fn frame_len(&self) -> usize {
            self.0.len()
        }

        fn free_capacity(&self) -> usize {
            1usize.saturating_sub(self.0.len())
        }

        fn push(&mut self, byte: u8) -> Result<(), FrameSinkError> {
            if self.0.len() == 1 {
                return Err(FrameSinkError::Full);
            }
            self.0.push(byte);
            Ok(())
        }

        fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError> {
            if run.len() > self.free_capacity() {
                return Err(FrameSinkError::Full);
            }
            self.0.extend_from_slice(run);
            Ok(())
        }
    }

    #[test]
    fn host_deframer_distinguishes_waiting_frames_keepalives_and_rejections() {
        use prns_core::interfaces::rns_serial_framing::FLAG;

        let mut scanner = RnsSerialScanner::new();
        let mut sink = std::vec::Vec::new();
        let mut offset = 0;
        assert_eq!(
            StreamDeframer::next_frame_into(&mut scanner, &[FLAG, 0x01], &mut offset, &mut sink,),
            StreamDeframeOutcome::AwaitingInput,
        );
        assert_eq!(sink, [0x01]);

        offset = 0;
        assert_eq!(
            StreamDeframer::next_frame_into(&mut scanner, &[0x02, FLAG], &mut offset, &mut sink,),
            StreamDeframeOutcome::Frame { len: 2 },
        );
        assert_eq!(sink, [0x01, 0x02]);

        sink.clear();
        offset = 0;
        assert_eq!(
            StreamDeframer::next_frame_into(&mut scanner, &[FLAG, FLAG], &mut offset, &mut sink,),
            StreamDeframeOutcome::Frame { len: 0 },
        );

        scanner.reset();
        let mut tiny = OneByteSink::default();
        offset = 0;
        assert_eq!(
            StreamDeframer::next_frame_into(
                &mut scanner,
                &[FLAG, 0x01, 0x02, FLAG],
                &mut offset,
                &mut tiny,
            ),
            StreamDeframeOutcome::Rejected,
        );
        assert_eq!(tiny.frame_len(), 0);
    }

    #[test]
    fn resource_frame_width_selects_its_batching_class() {
        const BUFFER_CAPACITY: usize = OUTBOUND_BATCH_TARGET_BYTES * 4;
        let mut frame = std::vec![0u8; BROADCAST_WIRE_FRAME_LEN];
        frame[HEADER_MIN_LEN - 1] = WireContext::Resource.to_byte();
        assert_eq!(
            outbound_batch_class(&frame),
            OutboundBatchClass::CompatibilitySizedResource
        );
        assert_eq!(
            windows_outbound_batch_target_bytes(&frame, BUFFER_CAPACITY),
            BUFFER_CAPACITY
        );

        frame[HEADER_MIN_LEN - 1] = WireContext::None.to_byte();
        assert_eq!(
            outbound_batch_class(&frame),
            OutboundBatchClass::LatencyBounded
        );
        assert_eq!(
            windows_outbound_batch_target_bytes(&frame, BUFFER_CAPACITY),
            OUTBOUND_BATCH_TARGET_BYTES
        );
        assert_eq!(
            outbound_batch_class(&frame[..HEADER_MIN_LEN - 1]),
            OutboundBatchClass::LatencyBounded
        );

        frame.resize(RESOURCE_WINDOW_BATCH_FRAME_MAX_BYTES, 0);
        frame[HEADER_MIN_LEN - 1] = WireContext::Resource.to_byte();
        assert_eq!(
            outbound_batch_class(&frame),
            OutboundBatchClass::WindowBatchableResource
        );
        assert_eq!(
            windows_outbound_batch_target_bytes(&frame, BUFFER_CAPACITY),
            RESOURCE_WINDOW_BATCH_TARGET_BYTES
        );

        frame.push(0);
        assert_eq!(
            outbound_batch_class(&frame),
            OutboundBatchClass::LatencyBounded
        );
        assert_eq!(
            windows_outbound_batch_target_bytes(&frame, BUFFER_CAPACITY),
            OUTBOUND_BATCH_TARGET_BYTES
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_resource_batching_remains_latency_bounded() {
        const BUFFER_CAPACITY: usize = OUTBOUND_BATCH_TARGET_BYTES * 4;
        let mut frame = std::vec![0u8; RESOURCE_WINDOW_BATCH_FRAME_MAX_BYTES];
        frame[HEADER_MIN_LEN - 1] = WireContext::Resource.to_byte();
        assert_eq!(
            outbound_batch_target_bytes(&frame, BUFFER_CAPACITY),
            OUTBOUND_BATCH_TARGET_BYTES
        );
    }

    impl InterfaceSeam for LaneSeam {
        fn fill_random(&mut self, bytes: &mut [u8]) {
            bytes.fill(0);
        }

        async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
            &mut self.inbound
        }

        async fn commit_inbound(&mut self) {
            self.inbound_commits.fetch_add(1, Ordering::Relaxed);
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
                inbound_commits: Arc::new(AtomicUsize::new(0)),
            };
            let status = TokioInterfaceStatus::new_accounted(
                InterfaceId::new([7u8; 8]),
                ConnectionState::Connected,
            );
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

    #[tokio::test]
    async fn a_large_queued_outbound_burst_yields_at_the_byte_target() {
        const PAYLOAD_LEN: usize = OUTBOUND_BATCH_TARGET_BYTES / 2;
        const FRAMED_LEN: usize = OUTBOUND_BATCH_TARGET_BYTES * 2;

        let (mut producer, consumer) = tokio_grant_lane(PAYLOAD_LEN, 3);
        let payloads = [
            std::vec![0x11; PAYLOAD_LEN],
            std::vec![0x22; PAYLOAD_LEN],
            std::vec![0x33; PAYLOAD_LEN],
        ];
        for payload in &payloads {
            producer
                .try_grant()
                .expect("lane has free slots")
                .fill(payload);
            producer.commit();
        }

        let (near, mut far) = tokio::io::duplex(FRAMED_LEN);
        let writes = Arc::new(AtomicUsize::new(0));
        let counted = WriteCounting {
            stream: near,
            writes: writes.clone(),
        };

        let served = tokio::spawn(async move {
            let mut buffers = FramedBuffers::<HdlcFraming, 4096, FRAMED_LEN>::new();
            let mut seam = LaneSeam {
                outbound: consumer,
                inbound: std::vec::Vec::new(),
                inbound_commits: Arc::new(AtomicUsize::new(0)),
            };
            let status = TokioInterfaceStatus::new_accounted(
                InterfaceId::new([8u8; 8]),
                ConnectionState::Connected,
            );
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

        let mut decoder = RnsSerialDecoder::<PAYLOAD_LEN>::new();
        let mut decoded: std::vec::Vec<std::vec::Vec<u8>> = std::vec::Vec::new();
        let mut buf = std::vec![0u8; FRAMED_LEN];
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
        assert_eq!(decoded, payloads);
        assert_eq!(writes.load(Ordering::Relaxed), 2);

        drop(far);
        served.await.expect("the serve loop returns on stream drop");
    }

    #[tokio::test]
    async fn inbound_frames_advance_while_an_opposite_write_is_backpressured() {
        let outbound = [0x11; 4096];
        let (mut producer, consumer) = tokio_grant_lane(4096, 1);
        producer
            .try_grant()
            .expect("the lane has a free slot")
            .fill(&outbound);
        producer.commit();

        let (near, mut far) = tokio::io::duplex(64);
        let writes = Arc::new(AtomicUsize::new(0));
        let counted = WriteCounting {
            stream: near,
            writes: writes.clone(),
        };
        let inbound_commits = Arc::new(AtomicUsize::new(0));
        let observed_commits = inbound_commits.clone();

        let served = tokio::spawn(async move {
            let mut buffers = FramedBuffers::<HdlcFraming, 4096, 8192>::new();
            let mut seam = LaneSeam {
                outbound: consumer,
                inbound: std::vec::Vec::new(),
                inbound_commits,
            };
            let status = TokioInterfaceStatus::new_accounted(
                InterfaceId::new([9u8; 8]),
                ConnectionState::Connected,
            );
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

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while writes.load(Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the outbound write starts");

        let mut inbound = [0u8; 64];
        let inbound_len = HdlcFraming::encode(b"inbound", &mut inbound).expect("the frame fits");
        tokio::io::AsyncWriteExt::write_all(&mut far, &inbound[..inbound_len])
            .await
            .expect("the inbound direction remains writable");

        tokio::time::timeout(std::time::Duration::from_millis(250), async {
            while observed_commits.load(Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the inbound frame advances independently of the blocked write");

        let mut decoder = RnsSerialDecoder::<4096>::new();
        let mut decoded = None;
        let mut buf = [0u8; 64];
        while decoded.is_none() {
            let read = tokio::io::AsyncReadExt::read(&mut far, &mut buf)
                .await
                .expect("the backpressured frame remains readable");
            assert_ne!(read, 0, "the stream stays up until the frame arrives");
            let mut offset = 0;
            while offset < read {
                if let Ok(Some(frame)) = decoder.feed_slice_next(&buf[..read], &mut offset) {
                    if !frame.is_empty() {
                        decoded = Some(frame.to_vec());
                    }
                }
            }
        }
        assert_eq!(decoded.as_deref(), Some(outbound.as_slice()));
        assert!(writes.load(Ordering::Relaxed) > 1);

        drop(far);
        served.await.expect("the serve loop returns on stream drop");
    }

    #[tokio::test(start_paused = true)]
    async fn a_writer_without_progress_disconnects_even_while_reads_advance() {
        let (mut producer, consumer) = tokio_grant_lane(4096, 1);
        let (near, mut far) = tokio::io::duplex(1);
        let writes = Arc::new(AtomicUsize::new(0));
        let counted = WriteCounting {
            stream: near,
            writes: writes.clone(),
        };
        let inbound_commits = Arc::new(AtomicUsize::new(0));
        let observed_commits = inbound_commits.clone();
        let served = tokio::spawn(async move {
            let mut buffers = FramedBuffers::<HdlcFraming, 4096, 8192>::new();
            let mut seam = LaneSeam {
                outbound: consumer,
                inbound: std::vec::Vec::new(),
                inbound_commits,
            };
            let status = TokioInterfaceStatus::new_accounted(
                InterfaceId::new([11u8; 8]),
                ConnectionState::Connected,
            );
            let mut airtime = AirtimeLedger::default();
            let mut throughput = ThroughputLedger::new();
            let mut meters = WireMeters {
                status: &status,
                airtime: &mut airtime,
                throughput: &mut throughput,
                bitrate: BitrateBps::guess(1_000_000),
                started: tokio::time::Instant::now(),
            };
            serve_with_write_progress_timeout(
                counted,
                &mut buffers,
                &mut seam,
                &mut meters,
                WriteProgressTimeout::after(std::time::Duration::from_millis(10)),
            )
            .await;
        });

        let mut frame = [0u8; 64];
        let len = HdlcFraming::encode(b"inbound", &mut frame).expect("the frame fits");
        tokio::io::AsyncWriteExt::write_all(&mut far, &frame[..len])
            .await
            .expect("the readable half starts live");
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while observed_commits.load(Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the inbound frame advances before any outbound write");

        producer
            .try_grant()
            .expect("the lane has a free slot")
            .fill(&[0x11; 4096]);
        producer.commit();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while writes.load(Ordering::Relaxed) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the outbound write reaches backpressure");

        tokio::io::AsyncWriteExt::write_all(&mut far, &frame[..len])
            .await
            .expect("the readable half remains live during write backpressure");
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while observed_commits.load(Ordering::Relaxed) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("inbound processing continues while the write is stalled");

        tokio::time::timeout(std::time::Duration::from_secs(1), served)
            .await
            .expect("the stalled writer reaches its progress deadline")
            .expect("the serve task exits cleanly");
        assert!(
            writes.load(Ordering::Relaxed) >= 2,
            "the stream accepted one byte before withholding further progress"
        );
        assert_eq!(observed_commits.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn a_frame_that_overflows_a_partial_batch_retains_order_and_bytes() {
        const FRAMED_LEN: usize = 128;
        let first = [prns_core::interfaces::rns_serial_framing::FLAG; 30];
        let second = [prns_core::interfaces::rns_serial_framing::FLAG; 60];
        let (mut producer, consumer) = tokio_grant_lane(second.len(), 2);
        for payload in [&first[..], &second[..]] {
            producer
                .try_grant()
                .expect("the lane has a free slot")
                .fill(payload);
            producer.commit();
        }

        let (near, mut far) = tokio::io::duplex(1024);
        let writes = Arc::new(AtomicUsize::new(0));
        let counted = WriteCounting {
            stream: near,
            writes: writes.clone(),
        };

        let served = tokio::spawn(async move {
            let mut buffers = FramedBuffers::<HdlcFraming, FRAMED_LEN, FRAMED_LEN>::new();
            let mut seam = LaneSeam {
                outbound: consumer,
                inbound: std::vec::Vec::new(),
                inbound_commits: Arc::new(AtomicUsize::new(0)),
            };
            let status = TokioInterfaceStatus::new_accounted(
                InterfaceId::new([10u8; 8]),
                ConnectionState::Connected,
            );
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

        let mut decoder = RnsSerialDecoder::<FRAMED_LEN>::new();
        let mut decoded: std::vec::Vec<std::vec::Vec<u8>> = std::vec::Vec::new();
        let mut buf = [0u8; FRAMED_LEN];
        while decoded.len() < 2 {
            let read = tokio::io::AsyncReadExt::read(&mut far, &mut buf)
                .await
                .expect("the wire remains readable");
            assert_ne!(read, 0, "the stream stays up until both frames arrive");
            let mut offset = 0;
            while offset < read {
                if let Ok(Some(frame)) = decoder.feed_slice_next(&buf[..read], &mut offset) {
                    if !frame.is_empty() {
                        decoded.push(frame.to_vec());
                    }
                }
            }
        }
        assert_eq!(decoded, [first.to_vec(), second.to_vec()]);
        assert_eq!(writes.load(Ordering::Relaxed), 2);

        drop(far);
        served.await.expect("the serve loop returns on stream drop");
    }
}
