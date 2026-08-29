use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

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
use prns_core::interfaces::{BitrateBps, FrameSink};
use prns_core::units::DurationMillis;
use prns_runtime::manifold::airtime::{frame_airtime_us, AirtimeLedger};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::InterfaceSeam;
use prns_runtime::manifold::throughput::ThroughputLedger;

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

trait StreamWatchdog {
    fn observe_read(&mut self, now: InstantMillis) -> I2pReadObservation;
    fn observe_ordinary_write(&mut self, now: InstantMillis);
    async fn wait_for_tick(&mut self, started: tokio::time::Instant) -> I2pWatchdogVerdict;
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
    feature = "tcp",
    feature = "serial",
    feature = "kiss",
    feature = "ax25",
    feature = "rnode",
    feature = "pipe",
    feature = "shared-instance",
    feature = "backbone"
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
    serve_inner(stream, buffers, seam, meters, NoIdleWatchdog).await;
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
    serve_inner(stream, buffers, seam, meters, I2pStreamWatchdog::start(now)).await;
}

async fn serve_inner<F, const READ_LEN: usize, const FRAMED_LEN: usize, S, Seam, Watchdog>(
    mut stream: S,
    buffers: &mut FramedBuffers<F, READ_LEN, FRAMED_LEN>,
    seam: &mut Seam,
    meters: &mut WireMeters<'_>,
    mut watchdog: Watchdog,
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

    loop {
        tokio::select! {
            read = stream.read(&mut *read_buf) => {
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
                    watchdog.observe_ordinary_write(elapsed_millis(started));
                    record_tx_write(filled);
                    filled = F::encode(next, &mut *frame_buf).unwrap_or(0);
                    break;
                }
                if filled > 0 {
                    if stream.write_all(&frame_buf[..filled]).await.is_err() {
                        return;
                    }
                    watchdog.observe_ordinary_write(elapsed_millis(started));
                    record_tx_write(filled);
                }
            }
            verdict = watchdog.wait_for_tick(started) => {
                match verdict {
                    I2pWatchdogVerdict::Continue => {}
                    I2pWatchdogVerdict::Degrade => {
                        status.set_connection(prns_core::interfaces::ConnectionState::Degraded);
                    }
                    I2pWatchdogVerdict::TransmitKeepalive => {
                        if stream.write_all(&HDLC_KEEPALIVE).await.is_err() {
                            return;
                        }
                    }
                    I2pWatchdogVerdict::DegradeAndTransmitKeepalive => {
                        status.set_connection(prns_core::interfaces::ConnectionState::Degraded);
                        if stream.write_all(&HDLC_KEEPALIVE).await.is_err() {
                            return;
                        }
                    }
                    I2pWatchdogVerdict::Disconnect => return,
                }
            }
        }
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
    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};

    struct LaneSeam {
        outbound: TokioGrantConsumer,
        inbound: std::vec::Vec<u8>,
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

    impl InterfaceSeam for LaneSeam {
        fn fill_entropy(&mut self, bytes: &mut [u8]) {
            bytes.fill(0);
        }

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
}
