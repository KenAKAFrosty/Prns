use std::string::String;

use tokio::net::TcpStream;

use crate::framed_stream;
use crate::tcp::tokio_socket::{tune, CONNECT_TIMEOUT};
use prns_core::interfaces::tcp::core;
use prns_core::interfaces::tcp::core::TcpWireFraming;
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::reactor::airtime::AirtimeLedger;
use prns_runtime::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use prns_runtime::reactor::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::reactor::throughput::ThroughputLedger;
use std::time::Duration;

/// The initiating end of an RNS TCP pair (`TCPClientInterface` parity): owns the
/// connection's whole lifecycle — resolve and connect to `target` (re-resolved every
/// attempt, so a peer behind dynamic DNS heals), apply the socket discipline, serve until
/// the stream drops, wait `reconnect`, connect again. Point-to-point: one engine
/// interface, one peer. `bitrate` is the host's claim about its pipe — it sets the
/// declared hardware MTU through the reference's tier table, so claim honestly
/// ([`core::TCP_BITRATE_ESTIMATE`] when genuinely unknown).
pub struct TcpClientInterface {
    id: InterfaceId,
    target: String,
    channel_tag: std::vec::Vec<u8>,
    policy: EffectiveInterfacePolicy,
    reconnect: Duration,
    framing: TcpWireFraming,
    status: TokioInterfaceStatus,
}

impl TcpClientInterface {
    #[must_use]
    pub fn new(target: String, bitrate: BitrateBps, reconnect: Duration) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, target.as_bytes());
        Self::with_id_policy_and_framing(
            id,
            target,
            core::policy_for_bitrate(bitrate),
            reconnect,
            TcpWireFraming::Hdlc,
        )
    }

    #[must_use]
    pub fn with_policy(
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect: Duration,
    ) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, target.as_bytes());
        Self::with_id_policy_and_framing(id, target, policy, reconnect, TcpWireFraming::Hdlc)
    }

    #[must_use]
    pub fn with_policy_and_framing(
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect: Duration,
        framing: TcpWireFraming,
    ) -> Self {
        let channel_tag = channel_tag(&target, framing);
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, &channel_tag);
        Self::with_id_policy_and_framing(id, target, policy, reconnect, framing)
    }

    #[must_use]
    pub fn with_framing(
        target: String,
        bitrate: BitrateBps,
        reconnect: Duration,
        framing: TcpWireFraming,
    ) -> Self {
        let channel_tag = channel_tag(&target, framing);
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpClient, &channel_tag);
        Self::with_id_policy_and_framing(
            id,
            target,
            core::policy_for_bitrate(bitrate),
            reconnect,
            framing,
        )
    }

    /// Build with a caller-chosen id instead of one derived from the dial target — for advanced
    /// setups that drive the reactor by hand and must pin the interface to a routing key their own
    /// wiring references. Ordinary nodes call [`new`](Self::new).
    #[must_use]
    pub fn new_with_id(
        id: InterfaceId,
        target: String,
        bitrate: BitrateBps,
        reconnect: Duration,
    ) -> Self {
        Self::with_id_policy_and_framing(
            id,
            target,
            core::policy_for_bitrate(bitrate),
            reconnect,
            TcpWireFraming::Hdlc,
        )
    }

    #[must_use]
    pub fn new_with_id_and_framing(
        id: InterfaceId,
        target: String,
        bitrate: BitrateBps,
        reconnect: Duration,
        framing: TcpWireFraming,
    ) -> Self {
        Self::with_id_policy_and_framing(
            id,
            target,
            core::policy_for_bitrate(bitrate),
            reconnect,
            framing,
        )
    }

    #[must_use]
    pub fn with_id_and_policy(
        id: InterfaceId,
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect: Duration,
    ) -> Self {
        Self::with_id_policy_and_framing(id, target, policy, reconnect, TcpWireFraming::Hdlc)
    }

    #[must_use]
    pub fn with_id_policy_and_framing(
        id: InterfaceId,
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect: Duration,
        framing: TcpWireFraming,
    ) -> Self {
        let channel_tag = channel_tag(&target, framing);
        Self {
            id,
            target,
            channel_tag,
            policy,
            reconnect,
            framing,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        }
    }

    /// This interface's id: minted by [`new`](Self::new), or the one handed to
    /// [`new_with_id`](Self::new_with_id). For the app that wants to name it (an
    /// [`AnnounceTarget::Interface`](prns_core::engine::AnnounceTarget), a log line).
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

    fn descriptor(&self) -> InterfaceDescriptor {
        core::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let interface_origin = seam.interface_origin().as_str();
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers: Option<
            framed_stream::FramedBuffers<
                framed_stream::HdlcFraming,
                { core::READ_BUF_LEN },
                { core::FRAMED_LEN },
            >,
        > = None;
        let mut kiss_buffers: Option<
            framed_stream::FramedBuffers<
                framed_stream::KissFraming,
                { core::READ_BUF_LEN },
                { core::KISS_FRAMED_LEN },
            >,
        > = None;
        loop {
            #[cfg(feature = "tracing")]
            let connected = tracing::Instrument::instrument(
                tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(self.target.as_str())),
                tracing::debug_span!(
                    target: "prns.interface",
                    "prns.interface.connect",
                    interface_kind = "tcp_client",
                    interface_origin,
                    peer = %self.target,
                ),
            )
            .await;
            #[cfg(not(feature = "tracing"))]
            let connected =
                tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(self.target.as_str()))
                    .await;
            if let Ok(Ok(stream)) = connected {
                tune(&stream);
                crate::diagnostic_log::debug!(
                    "tcp-client [{interface_origin}]: connected {}",
                    self.target
                );
                self.status.set_connection(ConnectionState::Connected);
                seam.request_tunnel_synthesis().await;
                let mut meters = framed_stream::WireMeters {
                    status: &self.status,
                    airtime: &mut airtime,
                    throughput: &mut throughput,
                    bitrate: self.policy.bitrate,
                    started,
                };
                match self.framing {
                    TcpWireFraming::Hdlc => {
                        framed_stream::serve::<
                            framed_stream::HdlcFraming,
                            { core::READ_BUF_LEN },
                            { core::FRAMED_LEN },
                            _,
                            _,
                        >(
                            stream,
                            buffers.get_or_insert_with(framed_stream::FramedBuffers::new),
                            &mut seam,
                            &mut meters,
                        )
                        .await;
                    }
                    TcpWireFraming::Kiss => {
                        framed_stream::serve::<
                            framed_stream::KissFraming,
                            { core::READ_BUF_LEN },
                            { core::KISS_FRAMED_LEN },
                            _,
                            _,
                        >(
                            stream,
                            kiss_buffers.get_or_insert_with(framed_stream::FramedBuffers::new),
                            &mut seam,
                            &mut meters,
                        )
                        .await;
                    }
                }
                crate::diagnostic_log::debug!(
                    "tcp-client [{interface_origin}]: dropped {}, retrying",
                    self.target
                );
                self.status.set_connection(ConnectionState::Disconnected);
            } else {
                crate::diagnostic_log::debug!(
                    "tcp-client [{interface_origin}]: connect failed {}, retrying",
                    self.target
                );
            }
            tokio::time::sleep(self.reconnect).await;
        }
    }
}

fn channel_tag(target: &str, framing: TcpWireFraming) -> std::vec::Vec<u8> {
    let mut channel_tag = target.as_bytes().to_vec();
    if framing == TcpWireFraming::Kiss {
        channel_tag.extend_from_slice(b"\0kiss");
    }
    channel_tag
}

impl prns_core::interfaces::ReportsStatus for TcpClientInterface {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::kiss_framing::{self, KissDecoder, FEND, FESC};
    use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder, ESC, FLAG};
    use prns_runtime::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::{self, UnboundedSender};

    /// A hand-driven seam: it captures every `next_inbound` and supplies `next_outbound` from a
    /// grant lane the test fills — so the interface's framing can be exercised in isolation.
    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        sink: std::vec::Vec<u8>,
        outbound: TokioGrantConsumer,
    }

    use prns_core::interfaces::FrameSink;

    impl InterfaceSeam for MockSeam {
        async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
            &mut self.sink
        }

        async fn commit_inbound(&mut self) {
            if !self.sink.is_empty() {
                let _ = self.inbound.send(std::mem::take(&mut self.sink));
            }
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

    async fn read_kiss_deframed(socket: &mut TcpStream) -> std::vec::Vec<u8> {
        let mut decoder = std::boxed::Box::new(KissDecoder::<{ core::FRAME_CAP }>::new());
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
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        let interface = TcpClientInterface::new(
            addr.to_string(),
            core::TCP_BITRATE_ESTIMATE,
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

    #[tokio::test]
    async fn the_client_honors_kiss_framing_in_both_directions() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral test port");
        let addr = listener.local_addr().expect("the bound address is known");
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(core::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };
        let interface = TcpClientInterface::with_framing(
            addr.to_string(),
            core::TCP_BITRATE_GUESS_BPS,
            Duration::from_millis(10),
            TcpWireFraming::Kiss,
        );
        tokio::spawn(interface.run(seam));
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("the client connects within the window")
            .expect("the listener accepts");

        let inbound = [0x01u8, FEND, FESC, 0x02];
        let mut framed = [0u8; 32];
        let framed_len =
            kiss_framing::encode(&inbound, &mut framed).expect("encodes the inbound payload");
        socket
            .write_all(&framed[..framed_len])
            .await
            .expect("writes the inbound frame");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(received, inbound);

        let outbound = [0xAAu8, FEND, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&outbound);
        out_tx.commit();
        let received =
            tokio::time::timeout(Duration::from_secs(2), read_kiss_deframed(&mut socket))
                .await
                .expect("the frame leaves within the window");
        assert_eq!(received, outbound);
    }

    #[test]
    fn framing_is_part_of_the_effective_tcp_channel() {
        let hdlc = TcpClientInterface::with_framing(
            "peer.example:4242".to_string(),
            core::TCP_BITRATE_GUESS_BPS,
            Duration::from_secs(5),
            TcpWireFraming::Hdlc,
        );
        let kiss = TcpClientInterface::with_framing(
            "peer.example:4242".to_string(),
            core::TCP_BITRATE_GUESS_BPS,
            Duration::from_secs(5),
            TcpWireFraming::Kiss,
        );
        assert_ne!(hdlc.id(), kiss.id());
        assert_ne!(hdlc.channel_tag(), kiss.channel_tag());
    }
}
