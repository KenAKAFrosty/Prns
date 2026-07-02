use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::interfaces::framed_stream::{self, KissFraming};
use crate::interfaces::kiss::core::{self, TncConfig};
use crate::interfaces::kiss_framing;
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::airtime::AirtimeLedger;
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;

/// How long to wait after opening the port before writing the TNC config, mirroring RNS
/// `configure_device`'s `sleep(2.0)` — a real TNC needs a moment to boot before it will accept
/// config, and bytes written into that window are lost. Tests pass `Duration::ZERO`.
pub const CONFIGURE_SETTLE: Duration = Duration::from_secs(2);

/// A KISS TNC interface (RNS `KISSInterface` parity): RNS packets framed as KISS over a serial
/// link. Like the serial interface it owns its medium's whole lifecycle — `open` yields a fresh
/// async byte stream, and the interface reconnects on its own — but on each connection it first
/// settles, writes the TNC config frames (preamble, TX-tail, persistence, slot time, ready), and
/// only then serves frames. A single never-dropping stream is just a factory that yields once.
pub struct KissInterface<Open> {
    id: InterfaceId,
    open: Open,
    reconnect: Duration,
    settle: Duration,
    tnc: TncConfig,
    channel_tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

impl<Open> KissInterface<Open> {
    /// Build with RNS's default TNC timing and config-settle delay. `channel_tag` names *which*
    /// serial device this is (the port name or a stable device id), exactly as for the serial
    /// interface — the same device across a reopen should pass the same bytes so its routes survive.
    #[must_use]
    pub fn new(open: Open, reconnect: Duration, channel_tag: &[u8]) -> Self {
        Self::with_settings(
            open,
            reconnect,
            CONFIGURE_SETTLE,
            TncConfig::default(),
            channel_tag,
        )
    }

    /// Build with explicit TNC timing and a custom config-settle delay — the daemon supplies the
    /// configured knobs, and tests pass `Duration::ZERO` to skip the real TNC boot wait.
    #[must_use]
    pub fn with_settings(
        open: Open,
        reconnect: Duration,
        settle: Duration,
        tnc: TncConfig,
        channel_tag: &[u8],
    ) -> Self {
        let channel_tag = channel_tag.to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Kiss, &channel_tag);
        Self {
            id,
            open,
            reconnect,
            settle,
            tnc,
            channel_tag,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        }
    }

    /// This interface's id, derived from its device `channel_tag`, for the app that wants to name
    /// it (an [`AnnounceTarget::Interface`](crate::engine::AnnounceTarget), a log line).
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// A clone of this interface's live-status handle for the app to read on its own render
    /// cadence. Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

/// Write the TNC config sequence onto a freshly opened link — the four-byte KISS command frames
/// RNS `configure_device` sends before the link carries traffic. Returns the stream's IO error so
/// the run loop can treat a TNC that vanished mid-config as a dropped connection and reconnect.
/// Shared with the AX.25-KISS interface, whose TNC setup is identical.
pub(crate) async fn configure_tnc<S: AsyncWrite + Unpin>(
    stream: &mut S,
    tnc: &TncConfig,
) -> io::Result<()> {
    for (command, value) in tnc.command_sequence() {
        stream
            .write_all(&kiss_framing::command_frame(command, value))
            .await?;
    }
    Ok(())
}

impl<Open, Fut, S> Interface for KissInterface<Open>
where
    Open: FnMut() -> Fut,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    const HW_MTU: usize = core::KISS_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Kiss;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let bitrate_bps = core::descriptor(self.id).bitrate_bps;
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers: Option<
            framed_stream::FramedBuffers<
                KissFraming,
                { core::READ_BUF_LEN },
                { core::KISS_FRAME_LEN },
                { core::FRAMED_LEN },
            >,
        > = None;
        loop {
            if let Ok(mut stream) = (self.open)().await {
                if !self.settle.is_zero() {
                    tokio::time::sleep(self.settle).await;
                }
                // A TNC that drops while being configured is just a dropped connection: skip serving
                // and fall through to the reconnect wait, exactly as a mid-serve drop would.
                if configure_tnc(&mut stream, &self.tnc).await.is_ok() {
                    self.status.set_connection(ConnectionState::Connected);
                    framed_stream::serve::<
                        KissFraming,
                        { core::READ_BUF_LEN },
                        { core::KISS_FRAME_LEN },
                        { core::FRAMED_LEN },
                        _,
                        _,
                    >(
                        stream,
                        buffers.get_or_insert_with(framed_stream::FramedBuffers::new),
                        &mut seam,
                        &self.status,
                        &mut airtime,
                        &mut throughput,
                        bitrate_bps,
                        started,
                    )
                    .await;
                    self.status.set_connection(ConnectionState::Disconnected);
                }
            }
            tokio::time::sleep(self.reconnect).await;
        }
    }
}

impl<Open> crate::interfaces::ReportsStatus for KissInterface<Open> {
    fn status_view(&self) -> Option<crate::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![crate::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::kiss_framing::{self, FEND, FESC};
    use crate::interfaces::InterfaceStatus;
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc::{self, UnboundedSender};

    /// A hand-driven seam: it captures every `next_inbound` and supplies `next_outbound` from a
    /// grant lane the test fills — so the interface's framing can be exercised in isolation.
    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        outbound: TokioGrantConsumer,
    }

    impl InterfaceSeam for MockSeam {
        async fn next_inbound(&mut self, frame: &[u8]) {
            let _ = self.inbound.send(frame.to_vec());
        }

        async fn next_outbound(&mut self) -> &[u8] {
            self.outbound.release();
            self.outbound.peek().await.frame()
        }
    }

    #[tokio::test]
    async fn configures_the_tnc_then_frames_and_deframes_kiss_across_a_real_async_stream() {
        // A duplex stands in for the serial wire: the factory yields its end once, then refuses
        // (the reconnect loop just retries harmlessly until the test drops the task).
        let (interface_wire, mut test_wire) = tokio::io::duplex(1024);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(core::KISS_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            outbound: out_rx,
        };

        // settle = ZERO so the test does not wait the real two-second TNC boot delay.
        let interface = KissInterface::with_settings(
            open,
            Duration::from_millis(10),
            Duration::ZERO,
            TncConfig::default(),
            b"test-kiss",
        );
        let status = interface.status();
        tokio::spawn(interface.run(seam));

        // On connect the interface writes the TNC config sequence before it serves anything.
        let mut config = [0u8; 20];
        tokio::time::timeout(Duration::from_secs(2), test_wire.read_exact(&mut config))
            .await
            .expect("the config frames arrive within the window")
            .expect("the wire stays up through the config write");
        assert_eq!(
            config,
            [
                FEND,
                kiss_framing::CMD_TXDELAY,
                35,
                FEND, // preamble 350 ms / 10
                FEND,
                kiss_framing::CMD_TXTAIL,
                2,
                FEND, // tx-tail 20 ms / 10
                FEND,
                kiss_framing::CMD_P,
                64,
                FEND, // persistence
                FEND,
                kiss_framing::CMD_SLOTTIME,
                2,
                FEND, // slot time 20 ms / 10
                FEND,
                kiss_framing::CMD_READY,
                1,
                FEND, // flow-control ready
            ]
        );

        // Inbound: a KISS data frame (FEND/FESC in the payload exercise the escaping) crosses the
        // wire and lands deframed at the seam.
        let payload = [0x01u8, 0x02, FEND, FESC, 0x03];
        let mut framed = [0u8; 32];
        let n = kiss_framing::encode(&payload, &mut framed).expect("encodes the payload");
        test_wire
            .write_all(&framed[..n])
            .await
            .expect("writes onto the wire");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(
            received, payload,
            "the interface deframes inbound KISS frames"
        );

        // Outbound: the seam yields a frame; the interface frames it onto the wire as a KISS data
        // frame; the test reads it back and deframes to the original.
        let out_payload = [0xAAu8, FEND, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();

        let mut decoder = core::Decoder::new();
        let mut buf = [0u8; 64];
        let decoded = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let n = test_wire.read(&mut buf).await.expect("reads from the wire");
                for &byte in &buf[..n] {
                    if let Ok(Some(frame)) = decoder.feed(byte) {
                        if !frame.is_empty() {
                            return frame.to_vec();
                        }
                    }
                }
            }
        })
        .await
        .expect("the interface frames outbound within the window");
        assert_eq!(
            decoded, out_payload,
            "the interface frames outbound packets"
        );

        // The interface's live status reflects what crossed — the connection and bytes both ways.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if status.connection() == ConnectionState::Connected
                    && status.rx_bytes() > 0
                    && status.tx_bytes() > 0
                    && status.airtime().is_some()
                    && status.transfer_rates().is_some()
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the live status reflects the connection + bytes both ways within the window");
    }
}
