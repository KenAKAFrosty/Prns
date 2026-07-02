use std::future::Future;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::framed_stream::{self, KissFraming};
use crate::kiss::{configure_tnc, CONFIGURE_SETTLE};
use personal_rns::interfaces::ax25_kiss::core::{self, Ax25AddressError, AX25_HEADER_SIZE};
use personal_rns::interfaces::kiss::core::TncConfig;
use personal_rns::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use personal_rns::reactor::airtime::AirtimeLedger;
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use personal_rns::reactor::interface_seam::{Interface, InterfaceSeam};
use personal_rns::reactor::throughput::ThroughputLedger;

/// The seam adapter that turns the plain KISS link into an AX.25-KISS one: it sits between the
/// reactor's seam and the shared serve loop, wrapping each outbound packet in the interface's fixed
/// AX.25 UI header and stripping that header off each inbound frame. Because the wrap is a constant
/// prefix and the strip is a constant length, the KISS framing and serve loop underneath carry it
/// unchanged — AX.25 is just a header on the payload, not a different wire framing.
struct Ax25Seam<S> {
    inner: S,
    header: [u8; AX25_HEADER_SIZE],
    /// Scratch holding `header ++ packet` for the borrow `next_outbound` lends the serve loop.
    /// Allocated once and reused; cleared per frame.
    outbound: std::vec::Vec<u8>,
}

impl<S: InterfaceSeam> InterfaceSeam for Ax25Seam<S> {
    async fn next_inbound(&mut self, frame: &[u8]) {
        // Strip the AX.25 header; a frame with no payload past it (or none at all) is dropped, as
        // RNS does (`process_incoming` only delivers when `len(data) > HEADER_SIZE`).
        if frame.len() > AX25_HEADER_SIZE {
            self.inner.next_inbound(&frame[AX25_HEADER_SIZE..]).await;
        }
    }

    async fn next_outbound(&mut self) -> &[u8] {
        self.outbound.clear();
        self.outbound.extend_from_slice(&self.header);
        let packet = self.inner.next_outbound().await;
        self.outbound.extend_from_slice(packet);
        &self.outbound
    }
}

/// An AX.25-KISS interface (RNS `AX25KISSInterface` parity): Reticulum packets wrapped in an AX.25
/// UI frame and carried as KISS over a serial TNC. It is the [`KissInterface`] mechanics — open,
/// settle, write the TNC config, serve — with one addition: every packet is wrapped in the fixed
/// AX.25 header built from the configured callsign/SSID, and that header is stripped on receive.
///
/// [`KissInterface`]: crate::kiss::KissInterface
pub struct Ax25KissInterface<Open> {
    id: InterfaceId,
    open: Open,
    reconnect: Duration,
    settle: Duration,
    tnc: TncConfig,
    header: [u8; AX25_HEADER_SIZE],
    channel_tag: std::vec::Vec<u8>,
    status: TokioInterfaceStatus,
}

impl<Open> Ax25KissInterface<Open> {
    /// Build with RNS's default TNC timing and config-settle delay. Fails if `callsign`/`ssid` is
    /// not a valid AX.25 address (3–6 ASCII chars, SSID 0–15). `channel_tag` names *which* serial
    /// device this is, exactly as for the serial and KISS interfaces.
    pub fn new(
        open: Open,
        reconnect: Duration,
        callsign: &str,
        ssid: u8,
        channel_tag: &[u8],
    ) -> Result<Self, Ax25AddressError> {
        Self::with_settings(
            open,
            reconnect,
            CONFIGURE_SETTLE,
            TncConfig::default(),
            callsign,
            ssid,
            channel_tag,
        )
    }

    /// Build with explicit TNC timing and a custom config-settle delay — the daemon supplies the
    /// configured knobs, and tests pass `Duration::ZERO` to skip the real TNC boot wait. Fails if
    /// the callsign/SSID is not a valid AX.25 address.
    #[allow(clippy::too_many_arguments)]
    pub fn with_settings(
        open: Open,
        reconnect: Duration,
        settle: Duration,
        tnc: TncConfig,
        callsign: &str,
        ssid: u8,
        channel_tag: &[u8],
    ) -> Result<Self, Ax25AddressError> {
        let header = core::build_header(callsign, ssid)?;
        let channel_tag = channel_tag.to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::Ax25Kiss, &channel_tag);
        Ok(Self {
            id,
            open,
            reconnect,
            settle,
            tnc,
            header,
            channel_tag,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        })
    }

    /// This interface's id, derived from its device `channel_tag`.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// A clone of this interface's live-status handle for the app to read on its own render cadence.
    /// Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl<Open, Fut, S> Interface for Ax25KissInterface<Open>
where
    Open: FnMut() -> Fut,
    Fut: Future<Output = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    const HW_MTU: usize = core::AX25_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Ax25Kiss;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, seam: Seam) {
        let bitrate_bps = core::descriptor(self.id).bitrate_bps;
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        // The adapter owns the seam and persists across reconnects, carrying its reusable scratch.
        let mut seam = Ax25Seam {
            inner: seam,
            header: self.header,
            outbound: std::vec::Vec::with_capacity(core::AX25_FRAME_LEN),
        };
        let mut buffers: Option<
            framed_stream::FramedBuffers<
                KissFraming,
                { core::READ_BUF_LEN },
                { core::AX25_FRAME_LEN },
                { core::FRAMED_LEN },
            >,
        > = None;
        loop {
            if let Ok(mut stream) = (self.open)().await {
                if !self.settle.is_zero() {
                    tokio::time::sleep(self.settle).await;
                }
                if configure_tnc(&mut stream, &self.tnc).await.is_ok() {
                    self.status.set_connection(ConnectionState::Connected);
                    framed_stream::serve::<
                        KissFraming,
                        { core::READ_BUF_LEN },
                        { core::AX25_FRAME_LEN },
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

impl<Open> personal_rns::interfaces::ReportsStatus for Ax25KissInterface<Open> {
    fn status_view(&self) -> Option<personal_rns::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![personal_rns::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::interfaces::kiss_framing::{self, FEND, FESC};
    use personal_rns::interfaces::InterfaceStatus;
    use personal_rns::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc::{self, UnboundedSender};

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

    /// KISS-frame an already-assembled body (`header ++ payload`) onto the wire.
    async fn write_kiss(wire: &mut tokio::io::DuplexStream, body: &[u8]) {
        let mut framed = std::vec![0u8; kiss_framing::max_encoded_len(body.len())];
        let n = kiss_framing::encode(body, &mut framed).expect("encodes the body");
        wire.write_all(&framed[..n])
            .await
            .expect("writes the frame");
    }

    #[tokio::test]
    async fn wraps_outbound_in_ax25_and_strips_the_header_inbound_over_a_real_stream() {
        let callsign = "N0CALL";
        let ssid = 3;
        let header = core::build_header(callsign, ssid).unwrap();

        let (interface_wire, mut test_wire) = tokio::io::duplex(2048);
        let mut wire = Some(interface_wire);
        let open = move || {
            let taken = wire.take();
            async move { taken.ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected)) }
        };

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(core::AX25_FRAME_LEN, 2);
        let seam = MockSeam {
            inbound: in_tx,
            outbound: out_rx,
        };

        let interface = Ax25KissInterface::with_settings(
            open,
            Duration::from_millis(10),
            Duration::ZERO,
            TncConfig::default(),
            callsign,
            ssid,
            b"test-ax25",
        )
        .expect("a valid AX.25 address");
        let status = interface.status();
        tokio::spawn(interface.run(seam));

        // The TNC config sequence is written first — the same five frames KISS sends.
        let mut config = [0u8; 20];
        tokio::time::timeout(Duration::from_secs(2), test_wire.read_exact(&mut config))
            .await
            .expect("the config frames arrive")
            .expect("the wire stays up through config");
        assert_eq!(
            config,
            [
                FEND,
                kiss_framing::CMD_TXDELAY,
                35,
                FEND,
                FEND,
                kiss_framing::CMD_TXTAIL,
                2,
                FEND,
                FEND,
                kiss_framing::CMD_P,
                64,
                FEND,
                FEND,
                kiss_framing::CMD_SLOTTIME,
                2,
                FEND,
                FEND,
                kiss_framing::CMD_READY,
                1,
                FEND,
            ]
        );

        // Inbound: a KISS frame whose body is `header ++ payload` lands at the seam with the AX.25
        // header stripped — only the payload. FEND/FESC in the payload exercise KISS escaping.
        let payload = [0x01u8, 0x02, FEND, FESC, 0x03];
        let mut body = header.to_vec();
        body.extend_from_slice(&payload);
        write_kiss(&mut test_wire, &body).await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(received, payload, "the AX.25 header is stripped inbound");

        // Outbound: the seam yields a packet; it leaves the wire as a KISS frame whose body is the
        // interface's AX.25 header followed by the packet.
        let out_payload = [0xAAu8, FEND, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();

        let mut decoder = core::Decoder::new();
        let mut buf = [0u8; 128];
        let body = tokio::time::timeout(Duration::from_secs(2), async {
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
            &body[..AX25_HEADER_SIZE],
            &header,
            "outbound is AX.25-wrapped"
        );
        assert_eq!(
            &body[AX25_HEADER_SIZE..],
            &out_payload,
            "with the packet after the header"
        );

        // Live status reflects the connection and bytes both ways.
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
