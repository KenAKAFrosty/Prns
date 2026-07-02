use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

use crate::framed_stream;
use crate::tcp::tokio_socket::{tune, RECONNECT_WAIT};
use personal_rns::interfaces::tcp::core;
use personal_rns::interfaces::{
    ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind, InterfaceStatus, TransferRates,
};
use personal_rns::reactor::airtime::AirtimeLedger;
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use personal_rns::reactor::interface_seam::{Interface, InterfaceSeam};
use personal_rns::reactor::throughput::ThroughputLedger;
use personal_rns::runtime::{Fleet, InterfaceSupervisor};

/// One client connected to our TCP server — the server-spawned side of an RNS TCP pair (the
/// reference's `spawned_interface`). A distinct engine interface over an already-accepted
/// [`TcpStream`](tokio::net::TcpStream), speaking the same HDLC framing the client and serial speak.
/// The [`TcpServer`] supervisor stands one up per connection and drops it when the stream closes;
/// unlike the client this end never reconnects — a vanished peer is just gone, and the client owns
/// the reconnect. Generic over the stream so the body serves a real socket in production and a
/// duplex pipe under test. `bitrate_bps` is the server's claim about its pipe, carried into the
/// member's declared MTU through the reference's tier table.
pub struct TcpServerConnection<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    stream: Option<S>,
    bitrate_bps: u32,
    status: TokioInterfaceStatus,
}

impl<S> TcpServerConnection<S> {
    /// Wrap an accepted connection. `channel_tag` uniquely tags this connection within the
    /// server-peer medium — the peer's `ip:port`. The supervisor owes its uniqueness across
    /// concurrent clients (distinct source ports give distinct tags); the attach path rejects a live
    /// collision loudly.
    #[must_use]
    pub fn new(channel_tag: Vec<u8>, stream: S, bitrate_bps: u32) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, &channel_tag);
        Self {
            id,
            channel_tag,
            stream: Some(stream),
            bitrate_bps,
            status: TokioInterfaceStatus::new(id, ConnectionState::Connected),
        }
    }

    /// This interface's id, minted from the channel tag. The supervisor lets the reactor cull
    /// it when the connection drops (the run future ends), so it never holds the id itself.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// A clone of this member's live-status handle, for a face to render the connected peer beside
    /// the server. Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Interface for TcpServerConnection<S> {
    const HW_MTU: usize = core::TCP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::TcpServerPeer;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, self.bitrate_bps)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let Some(stream) = self.stream.take() else {
            return;
        };
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut buffers = framed_stream::FramedBuffers::<
            framed_stream::HdlcFraming,
            { core::READ_BUF_LEN },
            { core::FRAME_CAP },
            { core::FRAMED_LEN },
        >::new();
        framed_stream::serve::<
            framed_stream::HdlcFraming,
            { core::READ_BUF_LEN },
            { core::FRAME_CAP },
            { core::FRAMED_LEN },
            _,
            _,
        >(
            stream,
            &mut buffers,
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
}

/// The listening end of an RNS TCP pair (`TCPServerInterface` parity): bind a port and stand up a
/// [`TcpServerConnection`] member per client that connects, each a distinct engine interface — the
/// reference spawns a child interface per accepted connection, and this is that fan-out. It owns no
/// wire of its own (its `process_outgoing` is the members'); a member whose connection drops
/// deregisters itself when its run future ends, so the supervisor accepts and forgets, and tearing
/// the supervisor down cascades to every member. Attach with `handle.supervise(TcpServer::bind(..))`.
///
/// `bitrate_bps` is the host's claim about its pipe — it sets each member's declared MTU through the
/// reference's tier table, so claim honestly ([`core::TCP_BITRATE_GUESS_BPS`] when genuinely unknown).
pub struct TcpServer {
    listener: TcpListener,
    bitrate_bps: u32,
    channel_tag: Vec<u8>,
    status: TcpServerStatus,
}

impl TcpServer {
    pub async fn bind(addr: impl tokio::net::ToSocketAddrs, bitrate_bps: u32) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let channel_tag = listener.local_addr()?.to_string().into_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpServer, &channel_tag);
        Ok(Self {
            listener,
            bitrate_bps,
            channel_tag,
            status: TcpServerStatus::new(id),
        })
    }

    /// This supervisor's id — `TcpServer`-kind, derived from its bound address. The members it stands
    /// up are `TcpServerPeer`-kind, a distinct id each. For the app that names the server card.
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::TcpServer, &self.channel_tag)
    }

    /// A clone of this listener's aggregate live-status handle: Dormant while bound with no client,
    /// Live once a client connects, with bytes and transfer rates summed across the connected clients.
    /// Each accepted client also keeps its own [`TokioInterfaceStatus`], rendered as a peer card beside
    /// the aggregate. Call before [`supervise`](personal_rns::runtime::TokioPrnsHandle::supervise) consumes it.
    #[must_use]
    pub fn status(&self) -> TcpServerStatus {
        self.status.clone()
    }

    /// The address the listener bound — with the OS-assigned port resolved, for a face to show and a
    /// client to dial.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

impl InterfaceSupervisor for TcpServer {
    const KIND: InterfaceKind = InterfaceKind::TcpServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run(self, fleet: Fleet) {
        loop {
            match self.listener.accept().await {
                Ok((stream, peer)) => {
                    tune(&stream);
                    let connection = TcpServerConnection::new(
                        peer.to_string().into_bytes(),
                        stream,
                        self.bitrate_bps,
                    );
                    self.status.admit(connection.status());
                    let _ = fleet.add(connection);
                }
                Err(_) => tokio::time::sleep(RECONNECT_WAIT).await,
            }
        }
    }
}

/// The TCP listener's aggregate live status, an [`InterfaceStatus`] over the whole server: the app
/// renders it as one card (a "WiFi"/"TCP" listener) whose [`connection`](InterfaceStatus::connection)
/// is Dormant (bound, no client) or Live (one or more clients connected), and whose bytes and
/// [`transfer_rates`](InterfaceStatus::transfer_rates) are summed across the connected clients. Each
/// accepted client keeps its own [`TokioInterfaceStatus`], exposed through [`members`](Self::members)
/// so a face can render the clients as ordinary peer cards beside the aggregate.
#[derive(Clone)]
pub struct TcpServerStatus {
    shared: Arc<TcpServerShared>,
}

struct TcpServerShared {
    id: InterfaceId,
    members: Mutex<Vec<TokioInterfaceStatus>>,
}

impl TcpServerStatus {
    fn new(id: InterfaceId) -> Self {
        Self {
            shared: Arc::new(TcpServerShared {
                id,
                members: Mutex::new(Vec::new()),
            }),
        }
    }

    fn admit(&self, status: TokioInterfaceStatus) {
        if let Ok(mut members) = self.shared.members.lock() {
            members.retain(|member| !matches!(member.connection(), ConnectionState::Disconnected));
            members.push(status);
        }
    }

    /// A snapshot of each connected client's own live status, for rendering the clients as ordinary
    /// peer cards beside the aggregate. Cheap: each handle is an `Arc` clone.
    #[must_use]
    pub fn members(&self) -> Vec<TokioInterfaceStatus> {
        match self.shared.members.lock() {
            Ok(members) => members.clone(),
            Err(_) => Vec::new(),
        }
    }
}

impl InterfaceStatus for TcpServerStatus {
    fn id(&self) -> InterfaceId {
        self.shared.id
    }

    fn connection(&self) -> ConnectionState {
        let live = match self.shared.members.lock() {
            Ok(members) => members
                .iter()
                .any(|member| matches!(member.connection(), ConnectionState::Connected)),
            Err(_) => false,
        };
        if live {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    fn rx_bytes(&self) -> u64 {
        self.shared
            .members
            .lock()
            .map(|members| members.iter().map(InterfaceStatus::rx_bytes).sum())
            .unwrap_or(0)
    }

    fn tx_bytes(&self) -> u64 {
        self.shared
            .members
            .lock()
            .map(|members| members.iter().map(InterfaceStatus::tx_bytes).sum())
            .unwrap_or(0)
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        let members = self.shared.members.lock().ok()?;
        members
            .iter()
            .filter_map(InterfaceStatus::transfer_rates)
            .reduce(|acc, rates| TransferRates {
                rx_bps: acc.rx_bps.saturating_add(rates.rx_bps),
                tx_bps: acc.tx_bps.saturating_add(rates.tx_bps),
            })
    }
}

impl personal_rns::interfaces::ReportsStatus for TcpServer {
    fn status_view(&self) -> Option<personal_rns::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![personal_rns::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl<S> personal_rns::interfaces::ReportsStatus for TcpServerConnection<S> {
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
    use personal_rns::interfaces::rns_serial_framing::{self, RnsSerialDecoder, ESC, FLAG};

    use personal_rns::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::mpsc::{self, UnboundedSender};

    /// A hand-driven seam: it captures every `next_inbound` and supplies `next_outbound` from a
    /// grant lane the test fills — so a member's framing can be exercised in isolation.
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

    fn duplex_member(tag: &[u8], bitrate_bps: u32) -> TcpServerConnection<tokio::io::DuplexStream> {
        let (near, _far) = tokio::io::duplex(64);
        TcpServerConnection::new(tag.to_vec(), near, bitrate_bps)
    }

    #[test]
    fn the_member_id_is_a_tcp_server_peer_kind_from_the_tag() {
        let iface = duplex_member(b"127.0.0.1:54321", core::TCP_BITRATE_GUESS_BPS);
        assert_eq!(iface.id().kind(), Some(InterfaceKind::TcpServerPeer));
        let same = duplex_member(b"127.0.0.1:54321", core::TCP_BITRATE_GUESS_BPS);
        assert_eq!(iface.id(), same.id(), "the same peer addr is the same id");
        let other = duplex_member(b"127.0.0.1:54322", core::TCP_BITRATE_GUESS_BPS);
        assert_ne!(iface.id(), other.id(), "a different peer is a different id");
    }

    #[test]
    fn the_member_descriptor_declares_the_servers_bitrate() {
        let iface = duplex_member(b"peer", 12_345_678);
        let descriptor = iface.descriptor();
        assert_eq!(descriptor.id, iface.id());
        assert_eq!(
            descriptor.bitrate_bps,
            Some(12_345_678),
            "the server's pipe claim rides into the member's descriptor",
        );
    }

    #[test]
    fn the_aggregate_status_is_dormant_until_a_client_connects() {
        let status = TcpServerStatus::new(InterfaceId::from_channel_tag(
            InterfaceKind::TcpServer,
            b"127.0.0.1:4242",
        ));
        assert_eq!(
            status.connection(),
            ConnectionState::Disconnected,
            "a bound listener with no client reads as Dormant",
        );
        assert_eq!(status.id().kind(), Some(InterfaceKind::TcpServer));

        let client = duplex_member(b"127.0.0.1:54321", core::TCP_BITRATE_GUESS_BPS);
        let client_status = client.status();
        status.admit(client_status.clone());
        assert_eq!(
            status.connection(),
            ConnectionState::Connected,
            "a connected client makes the listener Live",
        );
        assert_eq!(status.members().len(), 1);

        client_status.set_connection(ConnectionState::Disconnected);
        assert_eq!(
            status.connection(),
            ConnectionState::Disconnected,
            "the listener falls back to Dormant when its last client drops",
        );
    }

    #[tokio::test]
    async fn a_member_frames_and_deframes_across_a_real_socket() {
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

        // The server side: accept one connection and serve it as a member.
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.expect("the listener accepts");
            TcpServerConnection::new(
                peer.to_string().into_bytes(),
                stream,
                core::TCP_BITRATE_GUESS_BPS,
            )
            .run(seam)
            .await;
        });

        let mut client = TcpStream::connect(addr).await.expect("the client connects");

        // Inbound: a framed payload (FLAG/ESC exercise the escaping) crosses the real socket and
        // lands deframed at the seam.
        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        write_framed(&mut client, &payload).await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the member deframes within the window")
            .expect("the member task is alive");
        assert_eq!(received, payload);

        // Outbound: the seam yields a frame; it leaves the socket framed.
        let out_payload = [0xAAu8, FLAG, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let decoded = tokio::time::timeout(Duration::from_secs(2), read_deframed(&mut client))
            .await
            .expect("the frame leaves within the window");
        assert_eq!(decoded, out_payload);
    }
}
