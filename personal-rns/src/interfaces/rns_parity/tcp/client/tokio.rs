use std::string::String;

use tokio::net::TcpStream;

use crate::interfaces::framed_stream;
use crate::interfaces::rns_parity::tcp::core;
use crate::interfaces::rns_parity::tcp::tokio_socket::{tune, CONNECT_TIMEOUT};
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::airtime::AirtimeLedger;
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;
use std::time::Duration;

/// The initiating end of an RNS TCP pair (`TCPClientInterface` parity): owns the
/// connection's whole lifecycle — resolve and connect to `target` (re-resolved every
/// attempt, so a peer behind dynamic DNS heals), apply the socket discipline, serve until
/// the stream drops, wait `reconnect`, connect again. Point-to-point: one engine
/// interface, one peer. `bitrate_bps` is the host's claim about its pipe — it sets the
/// declared hardware MTU through the reference's tier table, so claim honestly
/// ([`core::TCP_BITRATE_GUESS_BPS`] when genuinely unknown).
pub struct TcpClientInterface {
    id: InterfaceId,
    target: String,
    bitrate_bps: u32,
    reconnect: Duration,
    status: TokioInterfaceStatus,
}

impl TcpClientInterface {
    #[must_use]
    pub fn new(target: String, bitrate_bps: u32, reconnect: Duration) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, target.as_bytes());
        Self::new_with_id(id, target, bitrate_bps, reconnect)
    }

    /// Build with a caller-chosen id instead of one derived from the dial target — for advanced
    /// setups that drive the reactor by hand and must pin the interface to a routing key their own
    /// wiring references. Ordinary nodes call [`new`](Self::new).
    #[must_use]
    pub fn new_with_id(
        id: InterfaceId,
        target: String,
        bitrate_bps: u32,
        reconnect: Duration,
    ) -> Self {
        Self {
            id,
            target,
            bitrate_bps,
            reconnect,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        }
    }

    /// This interface's id: minted by [`new`](Self::new), or the one handed to
    /// [`new_with_id`](Self::new_with_id). For the app that wants to name it (an
    /// [`AnnounceTarget::Interface`](crate::engine::AnnounceTarget), a log line).
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

impl Interface for TcpClientInterface {
    const HW_MTU: usize = core::TCP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::TcpClient;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, self.bitrate_bps)
    }

    fn channel_tag(&self) -> &[u8] {
        self.target.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers: Option<
            framed_stream::FramedBuffers<
                framed_stream::HdlcFraming,
                { core::READ_BUF_LEN },
                { core::FRAME_CAP },
                { core::FRAMED_LEN },
            >,
        > = None;
        loop {
            let connected =
                tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(self.target.as_str()))
                    .await;
            if let Ok(Ok(stream)) = connected {
                tune(&stream);
                self.status.set_connection(ConnectionState::Connected);
                framed_stream::serve::<
                    framed_stream::HdlcFraming,
                    { core::READ_BUF_LEN },
                    { core::FRAME_CAP },
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
                    Some(self.bitrate_bps),
                    started,
                )
                .await;
                self.status.set_connection(ConnectionState::Disconnected);
            }
            tokio::time::sleep(self.reconnect).await;
        }
    }
}

impl crate::interfaces::ReportsStatus for TcpClientInterface {
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
    use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder, ESC, FLAG};
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
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

    async fn write_framed(socket: &mut TcpStream, payload: &[u8]) {
        let mut framed = [0u8; 64];
        let n = rns_serial_framing::encode(payload, &mut framed).expect("encodes the payload");
        socket
            .write_all(&framed[..n])
            .await
            .expect("writes onto the wire");
    }

    async fn read_deframed(socket: &mut TcpStream) -> std::vec::Vec<u8> {
        let mut decoder = std::boxed::Box::new(RnsSerialDecoder::<{ core::FRAME_CAP }>::new());
        let mut buf = [0u8; 256];
        loop {
            let n = socket.read(&mut buf).await.expect("reads from the wire");
            assert_ne!(n, 0, "the wire stays up while a frame is owed");
            for &byte in &buf[..n] {
                if let Ok(Some(frame)) = decoder.feed(byte) {
                    if !frame.is_empty() {
                        return frame.to_vec();
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn the_client_frames_deframes_and_reconnects_across_real_sockets() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral test port");
        let addr = listener.local_addr().expect("the bound address is known");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(core::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            outbound: out_rx,
        };

        let interface = TcpClientInterface::new(
            addr.to_string(),
            core::TCP_BITRATE_GUESS_BPS,
            Duration::from_millis(10),
        );
        tokio::spawn(interface.run(seam));

        let (mut first, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("the client connects within the window")
            .expect("the listener accepts");

        // Inbound: a framed payload (FLAG/ESC bytes exercise the escaping) crosses the real
        // socket and lands deframed at the seam.
        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        write_framed(&mut first, &payload).await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(received, payload);

        // Outbound: the seam yields a frame; it leaves the socket framed.
        let out_payload = [0xAAu8, FLAG, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let decoded = tokio::time::timeout(Duration::from_secs(2), read_deframed(&mut first))
            .await
            .expect("the frame leaves within the window");
        assert_eq!(decoded, out_payload);

        // The connection drops; the client reconnects on its own and the next frame still
        // arrives — the reference's RECONNECT_WAIT loop, compressed for the test.
        drop(first);
        let (mut second, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("the client reconnects within the window")
            .expect("the listener accepts again");
        write_framed(&mut second, b"after the reconnect").await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the reconnected interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(received, b"after the reconnect");
    }
}
