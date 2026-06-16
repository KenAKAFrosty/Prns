use std::io;
use std::net::{IpAddr, SocketAddr};
use std::string::String;
use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};
use tokio::net::{TcpListener, TcpStream};

use crate::interfaces::framed_stream;
use crate::interfaces::rns_parity::tcp::core;
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::airtime::AirtimeLedger;
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;

/// How long one connect attempt gets (`TCPClientInterface.INITIAL_CONNECT_TIMEOUT`).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a failed or dropped connection waits before the next attempt
/// (`TCPClientInterface.RECONNECT_WAIT`).
pub const RECONNECT_WAIT: Duration = Duration::from_secs(5);
/// The reference's keepalive discipline: probe after 5s idle, every 2s, and a peer that
/// misses 12 probes — or leaves writes unacknowledged for 24s — is declared dead, so the
/// stream drops and the reconnect loop takes over.
pub const TCP_PROBE_AFTER: Duration = Duration::from_secs(5);
pub const TCP_PROBE_INTERVAL: Duration = Duration::from_secs(2);
pub const TCP_PROBES: u32 = 12;
pub const TCP_USER_TIMEOUT: Duration = Duration::from_secs(24);

/// The reference's socket discipline, best-effort: Nagle off (a Reticulum frame should hit
/// the wire when it's written, not pool behind the ACK clock) and the keepalive probes
/// above. A socket that refuses an option still carries frames, so refusals don't fail the
/// connection.
fn tune(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
    let socket = SockRef::from(stream);
    let keepalive = TcpKeepalive::new()
        .with_time(TCP_PROBE_AFTER)
        .with_interval(TCP_PROBE_INTERVAL)
        .with_retries(TCP_PROBES);
    let _ = socket.set_tcp_keepalive(&keepalive);
    #[cfg(target_os = "linux")]
    let _ = socket.set_tcp_user_timeout(Some(TCP_USER_TIMEOUT));
}

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
        let id = InterfaceId::from_reachability_tag(InterfaceKind::TcpClient, target.as_bytes());
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

    fn reachability_tag(&self) -> &[u8] {
        self.target.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        loop {
            let connected =
                tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(self.target.as_str()))
                    .await;
            if let Ok(Ok(stream)) = connected {
                tune(&stream);
                self.status.set_connection(ConnectionState::Connected);
                framed_stream::serve::<
                    { core::READ_BUF_LEN },
                    { core::FRAME_CAP },
                    { core::FRAMED_LEN },
                    _,
                    _,
                >(
                    stream,
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

/// The listening end (`TCPServerInterface` parity, scoped to point-to-point): bind once,
/// then accept one peer at a time — serve that connection until it drops, accept the next.
/// The reference spawns a child interface per accepted client; our interface table is fixed
/// at engine construction, so multi-client fan-out is deferred and a second client waits in
/// the accept backlog until the first drops.
pub struct TcpServerInterface {
    id: InterfaceId,
    listener: TcpListener,
    bitrate_bps: u32,
    reachability_tag: heapless::Vec<u8, 18>,
    status: TokioInterfaceStatus,
}

/// The bytes that tag this server channel: the address it listens on (one interface per bound
/// address — the per-client fan-out the reference spawns is deferred, see the type docs).
fn server_reachability_tag(local: SocketAddr) -> heapless::Vec<u8, 18> {
    let mut tag = heapless::Vec::new();
    match local.ip() {
        IpAddr::V4(v4) => {
            let _ = tag.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            let _ = tag.extend_from_slice(&v6.octets());
        }
    }
    let _ = tag.extend_from_slice(&local.port().to_be_bytes());
    tag
}

impl TcpServerInterface {
    /// Bind the listener up front, before [`run`](Interface::run) — so the bound address
    /// (an OS-assigned port included) is readable and a bind refusal surfaces here, not
    /// silently inside the accept loop. `bitrate_bps` is the host's claim about its pipe —
    /// it sets the declared hardware MTU through the reference's tier table, so claim
    /// honestly ([`core::TCP_BITRATE_GUESS_BPS`] when genuinely unknown).
    pub async fn bind(addr: impl tokio::net::ToSocketAddrs, bitrate_bps: u32) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Self::assemble(None, listener, bitrate_bps)
    }

    /// Bind with a caller-chosen id instead of one derived from the listen address — for advanced
    /// setups that drive the reactor by hand and must pin the interface to a routing key their own
    /// wiring references. Ordinary nodes call [`bind`](Self::bind).
    pub async fn bind_with_id(
        id: InterfaceId,
        addr: impl tokio::net::ToSocketAddrs,
        bitrate_bps: u32,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Self::assemble(Some(id), listener, bitrate_bps)
    }

    fn assemble(
        id_override: Option<InterfaceId>,
        listener: TcpListener,
        bitrate_bps: u32,
    ) -> io::Result<Self> {
        let reachability_tag = server_reachability_tag(listener.local_addr()?);
        let id = id_override.unwrap_or_else(|| {
            InterfaceId::from_reachability_tag(InterfaceKind::TcpServer, &reachability_tag)
        });
        Ok(Self {
            id,
            listener,
            bitrate_bps,
            reachability_tag,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        })
    }

    /// This interface's id: minted by [`bind`](Self::bind), or the one handed to
    /// [`bind_with_id`](Self::bind_with_id). For the app that wants to name it (an
    /// [`AnnounceTarget::Interface`](crate::engine::AnnounceTarget), a log line).
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// A clone of this interface's live-status handle for the app to read on its own render
    /// cadence. Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl Interface for TcpServerInterface {
    const HW_MTU: usize = core::TCP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::TcpServer;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, self.bitrate_bps)
    }

    fn reachability_tag(&self) -> &[u8] {
        &self.reachability_tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        loop {
            let Ok((stream, _)) = self.listener.accept().await else {
                tokio::time::sleep(RECONNECT_WAIT).await;
                continue;
            };
            tune(&stream);
            self.status.set_connection(ConnectionState::Connected);
            framed_stream::serve::<
                { core::READ_BUF_LEN },
                { core::FRAME_CAP },
                { core::FRAMED_LEN },
                _,
                _,
            >(
                stream,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder, ESC, FLAG};
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, HostCommand, TokioGrantConsumer};
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

    #[tokio::test]
    async fn the_server_serves_the_next_connection_after_the_first_drops() {
        let interface = TcpServerInterface::bind("127.0.0.1:0", core::TCP_BITRATE_GUESS_BPS)
            .await
            .expect("binds an ephemeral test port");
        let addr = interface.local_addr().expect("the bound address is known");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(core::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            outbound: out_rx,
        };
        tokio::spawn(interface.run(seam));

        let mut first = TcpStream::connect(addr)
            .await
            .expect("the first client connects");
        write_framed(&mut first, b"from the first peer").await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the server deframes within the window")
            .expect("the interface task is alive");
        assert_eq!(received, b"from the first peer");

        let out_payload = [0xC5u8, FLAG, ESC, 0x42];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let decoded = tokio::time::timeout(Duration::from_secs(2), read_deframed(&mut first))
            .await
            .expect("the frame leaves within the window");
        assert_eq!(decoded, out_payload);

        // The first peer drops; the next connection is served by the same interface.
        drop(first);
        let mut second = TcpStream::connect(addr)
            .await
            .expect("the second client connects");
        write_framed(&mut second, b"from the second peer").await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the server serves the next connection within the window")
            .expect("the interface task is alive");
        assert_eq!(received, b"from the second peer");
    }

    /// The live-reactor capstone over real TCP: the mirror of
    /// `a_link_establishes_and_carries_data_across_two_live_reactors`, with the in-memory
    /// loopback replaced by a `TcpServerInterface` ↔ `TcpClientInterface` pair on localhost —
    /// announce, establish (MTU signalled by both TCP descriptors), data both ways, the
    /// ProveAll proof settling Delivered, the unproven direction timing out.
    #[tokio::test]
    async fn a_link_establishes_and_carries_data_across_two_live_reactors_over_tcp() {
        use crate::engine::test_support::{
            fixed_secret_key, personal_node_destination, second_secret_key, Cap,
        };
        use crate::engine::{
            AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EngineState,
            EstablishLink, IssuedCommand, Journaled, LinkEstablished, RatchetPolicy, SendLink,
            SendLinkFailure, SendLinkPayload, Settlement,
        };
        use crate::reactor::impls::tokio_reactor::{run, Egress, TokioHost, TokioInterfaceSeam};
        use crate::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
        use crate::routing::delivery::Delivery;
        use crate::routing::links::LinkId;
        use crate::routing::upstream_app_destinations::ProofStrategy;

        let b_interface = TcpServerInterface::bind("127.0.0.1:0", core::TCP_BITRATE_GUESS_BPS)
            .await
            .expect("binds an ephemeral test port");
        let responder_iface = b_interface.id();
        let addr = b_interface
            .local_addr()
            .expect("the bound address is known");
        let a_interface = TcpClientInterface::new(
            addr.to_string(),
            core::TCP_BITRATE_GUESS_BPS,
            Duration::from_millis(10),
        );
        let initiator_iface = a_interface.id();

        let initiator_engine = EngineState::<Cap>::new(second_secret_key());
        let (a_notify_tx, a_notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (a_in_tx, a_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (a_out_tx, a_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let a_seam = TokioInterfaceSeam::new(initiator_iface, a_in_tx, a_notify_tx, a_out_rx);
        let a_egress = Egress::new(std::vec![(initiator_iface, a_out_tx)]);
        let (a_command_tx, a_command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (a_heard_tx, mut a_heard_rx) = mpsc::unbounded_channel::<()>();
        let (a_settled_tx, mut a_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let (a_delivered_tx, mut a_delivered_rx) =
            mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
        let a_app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { .. } => {
                let _ = a_heard_tx.send(());
            }
            Journaled::CommandSettled { id, settlement } => {
                let _ = a_settled_tx.send((id, settlement));
            }
            Journaled::Delivered(Delivery::Link(link)) => {
                let _ = a_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
            }
            _ => {}
        };

        let responder_engine = {
            let mut engine: EngineState<Cap> = EngineState::new(fixed_secret_key());
            let node = engine.held_identity_hashes()[0];
            engine
                .register_single_destination(
                    &node,
                    "personal",
                    &["node"],
                    b"hello-personal",
                    ProofStrategy::ProveAll,
                    RatchetPolicy::NoRatchets,
                )
                .expect("registers the proving destination");
            engine
        };
        let (b_notify_tx, b_notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (b_in_tx, b_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (b_out_tx, b_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let b_seam = TokioInterfaceSeam::new(responder_iface, b_in_tx, b_notify_tx, b_out_rx);
        let b_egress = Egress::new(std::vec![(responder_iface, b_out_tx)]);
        let (b_command_tx, b_command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (b_established_tx, mut b_established_rx) = mpsc::unbounded_channel::<LinkEstablished>();
        let (b_settled_tx, mut b_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let (b_delivered_tx, mut b_delivered_rx) =
            mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
        let b_app = move |journaled: Journaled<'_>| match journaled {
            Journaled::LinkEstablished(established) => {
                let _ = b_established_tx.send(established);
            }
            Journaled::CommandSettled { id, settlement } => {
                let _ = b_settled_tx.send((id, settlement));
            }
            Journaled::Delivered(Delivery::Link(link)) => {
                let _ = b_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
            }
            _ => {}
        };

        tokio::spawn(run(
            initiator_engine,
            std::vec![core::descriptor(
                initiator_iface,
                core::TCP_BITRATE_GUESS_BPS
            )],
            std::vec![],
            TokioHost::new(),
            a_notify_rx,
            std::vec![(initiator_iface, a_in_rx)],
            a_command_rx,
            a_egress,
            a_app,
        ));
        tokio::spawn(run(
            responder_engine,
            std::vec![core::descriptor(
                responder_iface,
                core::TCP_BITRATE_GUESS_BPS
            )],
            std::vec![],
            TokioHost::new(),
            b_notify_rx,
            std::vec![(responder_iface, b_in_rx)],
            b_command_rx,
            b_egress,
            b_app,
        ));
        tokio::spawn(a_interface.run(a_seam));
        tokio::spawn(b_interface.run(b_seam));

        b_command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(1),
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination: personal_node_destination(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            }))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), a_heard_rx.recv())
            .await
            .expect("the announce crosses the wire")
            .expect("the initiator reactor is alive");

        a_command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(7),
                command: EngineCommand::EstablishLink(EstablishLink {
                    destination: personal_node_destination(),
                }),
            }))
            .unwrap();

        let (settled_id, settlement) =
            tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
                .await
                .expect("the link settles within the window")
                .expect("the initiator reactor is alive");
        assert_eq!(settled_id, CommandId(7));
        let Settlement::EstablishLink(Ok(established)) = settlement else {
            panic!("the command must settle established, got {settlement:?}");
        };

        let responder_side = tokio::time::timeout(Duration::from_secs(5), b_established_rx.recv())
            .await
            .expect("the responder journals the link up")
            .expect("the responder reactor is alive");
        assert_eq!(
            responder_side.link_id, established.link_id,
            "one link, two ends",
        );

        a_command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(8),
                command: EngineCommand::SendLink(SendLink {
                    link_id: established.link_id,
                    payload: SendLinkPayload::from_slice(b"ping over real tcp").unwrap(),
                }),
            }))
            .unwrap();
        let delivered = tokio::time::timeout(Duration::from_secs(5), b_delivered_rx.recv())
            .await
            .expect("the responder journals the delivery")
            .expect("the responder reactor is alive");
        assert_eq!(
            delivered,
            (established.link_id, b"ping over real tcp".to_vec())
        );
        let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
            .await
            .expect("the initiator's send settles")
            .expect("the initiator reactor is alive");
        assert_eq!(sent_id, CommandId(8));
        let Settlement::SendLink(Ok(_delivered_receipt)) = sent else {
            panic!("the ProveAll responder's proof settles the send Delivered, got {sent:?}");
        };

        b_command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(2),
                command: EngineCommand::SendLink(SendLink {
                    link_id: established.link_id,
                    payload: SendLinkPayload::from_slice(b"pong right back").unwrap(),
                }),
            }))
            .unwrap();
        let sent = loop {
            let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), b_settled_rx.recv())
                .await
                .expect("the responder's send settles")
                .expect("the responder reactor is alive");
            if sent_id == CommandId(2) {
                break sent;
            }
        };
        assert_eq!(
            sent,
            Settlement::SendLink(Err(SendLinkFailure::Timeout)),
            "the initiator's side never proves, so the responder's send times out — parity",
        );
        let delivered = tokio::time::timeout(Duration::from_secs(5), a_delivered_rx.recv())
            .await
            .expect("the initiator journals the delivery")
            .expect("the initiator reactor is alive");
        assert_eq!(
            delivered,
            (established.link_id, b"pong right back".to_vec())
        );
    }
}
