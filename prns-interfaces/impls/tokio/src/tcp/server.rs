use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

use crate::byte_stream::framing;
use crate::reconnect::ReconnectPolicy;
use crate::tcp::{tune_for_tunnel, TcpTunnelMode};
use prns_core::interfaces::tcp::{self, TcpWireFraming};
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
    InterfaceStatus, TransferRates,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;
use prns_runtime::runtime::{Fleet, InterfaceSupervisor};

pub struct TcpServerConnection<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    stream: Option<S>,
    policy: EffectiveInterfacePolicy,
    framing: TcpWireFraming,
    status: TokioInterfaceStatus,
}

impl<S> TcpServerConnection<S> {
    #[must_use]
    pub fn new(channel_tag: Vec<u8>, stream: S, bitrate: BitrateBps) -> Self {
        Self::with_policy(channel_tag, stream, tcp::policy_for_bitrate(bitrate))
    }

    #[must_use]
    pub fn with_policy(channel_tag: Vec<u8>, stream: S, policy: EffectiveInterfacePolicy) -> Self {
        Self::with_policy_and_framing(channel_tag, stream, policy, TcpWireFraming::Hdlc)
    }

    #[must_use]
    pub fn with_policy_and_framing(
        channel_tag: Vec<u8>,
        stream: S,
        policy: EffectiveInterfacePolicy,
        framing: TcpWireFraming,
    ) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, &channel_tag);
        Self {
            id,
            channel_tag,
            stream: Some(stream),
            policy,
            framing,
            status: TokioInterfaceStatus::new_accounted(id, ConnectionState::Connected),
        }
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Interface for TcpServerConnection<S> {
    const HW_MTU: usize = tcp::TCP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::TcpServerPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        tcp::descriptor(self.id, self.policy)
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
        let mut meters = framing::WireMeters {
            status: &self.status,
            airtime: &mut airtime,
            throughput: &mut throughput,
            bitrate: self.policy.bitrate,
            started,
        };
        match self.framing {
            TcpWireFraming::Hdlc => {
                let mut buffers = framing::FramedBuffers::<
                    framing::HdlcFraming,
                    { tcp::READ_BUF_LEN },
                    { tcp::FRAMED_LEN },
                >::new();
                framing::serve::<
                    framing::HdlcFraming,
                    { tcp::READ_BUF_LEN },
                    { tcp::FRAMED_LEN },
                    _,
                    _,
                >(stream, &mut buffers, &mut seam, &mut meters)
                .await;
            }
            TcpWireFraming::Kiss => {
                let mut buffers = framing::FramedBuffers::<
                    framing::KissFraming,
                    { tcp::READ_BUF_LEN },
                    { tcp::KISS_FRAMED_LEN },
                >::new();
                framing::serve::<
                    framing::KissFraming,
                    { tcp::READ_BUF_LEN },
                    { tcp::KISS_FRAMED_LEN },
                    _,
                    _,
                >(stream, &mut buffers, &mut seam, &mut meters)
                .await;
            }
        }
        self.status.set_connection(ConnectionState::Disconnected);
    }
}

pub struct TcpServer {
    listener: TcpListener,
    policy: EffectiveInterfacePolicy,
    tunnel: TcpTunnelMode,
    framing: TcpWireFraming,
    channel_tag: Vec<u8>,
    status: TcpServerStatus,
}

impl TcpServer {
    pub async fn bind(addr: impl tokio::net::ToSocketAddrs) -> io::Result<Self> {
        Self::bind_with_bitrate(addr, tcp::TCP_BITRATE_ESTIMATE).await
    }

    pub async fn bind_with_bitrate(
        addr: impl tokio::net::ToSocketAddrs,
        bitrate: BitrateBps,
    ) -> io::Result<Self> {
        Self::bind_with_policy(addr, tcp::policy_for_bitrate(bitrate)).await
    }

    pub async fn bind_with_policy(
        addr: impl tokio::net::ToSocketAddrs,
        policy: EffectiveInterfacePolicy,
    ) -> io::Result<Self> {
        Self::bind_with_policy_and_tunnel(addr, policy, TcpTunnelMode::Direct).await
    }

    pub async fn bind_with_policy_and_tunnel(
        addr: impl tokio::net::ToSocketAddrs,
        policy: EffectiveInterfacePolicy,
        tunnel: TcpTunnelMode,
    ) -> io::Result<Self> {
        Self::bind_with_policy_and_tunnel_and_framing(addr, policy, tunnel, TcpWireFraming::Hdlc)
            .await
    }

    pub async fn bind_with_policy_and_tunnel_and_framing(
        addr: impl tokio::net::ToSocketAddrs,
        policy: EffectiveInterfacePolicy,
        tunnel: TcpTunnelMode,
        framing: TcpWireFraming,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let channel_tag = listener.local_addr()?.to_string().into_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::TcpServer, &channel_tag);
        Ok(Self {
            listener,
            policy,
            tunnel,
            framing,
            channel_tag,
            status: TcpServerStatus::new(id),
        })
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::TcpServer, &self.channel_tag)
    }

    #[must_use]
    pub fn status(&self) -> TcpServerStatus {
        self.status.clone()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

impl InterfaceSupervisor for TcpServer {
    const KIND: InterfaceKind = InterfaceKind::TcpServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(self, fleet: Fleet) {
        let policy = ReconnectPolicy::STANDARD;
        let mut schedule = policy.schedule();
        loop {
            match self.listener.accept().await {
                Ok((stream, peer)) => {
                    schedule = policy.schedule();
                    tune_for_tunnel(&stream, self.tunnel);
                    let connection = TcpServerConnection::with_policy_and_framing(
                        peer.to_string().into_bytes(),
                        stream,
                        self.policy,
                        self.framing,
                    );
                    self.status.admit(connection.status());
                    let _ = fleet.add(connection);
                }
                Err(_) => {
                    let delay = schedule.next_delay(|bytes| fleet.fill_entropy(bytes));
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

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
        ConnectionState::Connected
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

impl prns_core::interfaces::ReportsStatus for TcpServer {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl<S> prns_core::interfaces::ReportsStatus for TcpServerConnection<S> {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }

    fn connection_view(&self) -> Option<prns_core::interfaces::ConnectionView> {
        Some(prns_core::interfaces::ConnectionView::of(self.status()))
    }

    fn frame_accounting_recorder(&self) -> Option<prns_core::interfaces::FrameAccountingRecorder> {
        prns_core::interfaces::FrameAccountingRecorder::of(self.status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::kiss_framing::{self, KissDecoder, FEND, FESC};
    use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder, ESC, FLAG};

    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::mpsc::{self, UnboundedSender};

    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        sink: std::vec::Vec<u8>,
        outbound: TokioGrantConsumer,
    }

    use prns_core::interfaces::FrameSink;

    impl InterfaceSeam for MockSeam {
        fn fill_entropy(&mut self, bytes: &mut [u8]) {
            bytes.fill(0);
        }

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
        let mut decoder = std::boxed::Box::new(RnsSerialDecoder::<{ tcp::FRAME_CAP }>::new());
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
        let mut decoder = std::boxed::Box::new(KissDecoder::<{ tcp::FRAME_CAP }>::new());
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

    fn duplex_member(
        tag: &[u8],
        bitrate: BitrateBps,
    ) -> TcpServerConnection<tokio::io::DuplexStream> {
        let (near, _far) = tokio::io::duplex(64);
        TcpServerConnection::new(tag.to_vec(), near, bitrate)
    }

    #[test]
    fn the_member_id_is_a_tcp_server_peer_kind_from_the_tag() {
        let iface = duplex_member(b"127.0.0.1:54321", tcp::TCP_BITRATE_ESTIMATE);
        assert_eq!(iface.id().kind(), Some(InterfaceKind::TcpServerPeer));
        let same = duplex_member(b"127.0.0.1:54321", tcp::TCP_BITRATE_ESTIMATE);
        assert_eq!(iface.id(), same.id(), "the same peer addr is the same id");
        let other = duplex_member(b"127.0.0.1:54322", tcp::TCP_BITRATE_ESTIMATE);
        assert_ne!(iface.id(), other.id(), "a different peer is a different id");
    }

    #[test]
    fn the_member_descriptor_declares_the_servers_bitrate() {
        let iface = duplex_member(b"peer", BitrateBps::guess(12_345_678));
        let descriptor = iface.descriptor();
        assert_eq!(descriptor.id, iface.id());
        assert_eq!(
            descriptor.bitrate,
            BitrateBps::guess(12_345_678),
            "the server's pipe claim rides into the member's descriptor",
        );
    }

    #[test]
    fn the_member_inherits_the_servers_complete_effective_policy() {
        let policy = tcp::configured_policy(prns_core::interfaces::ConfiguredInterfacePolicy {
            mode: Some(prns_core::interfaces::InterfaceMode::Gateway),
            bitrate: Some(BitrateBps::guess(900_000_000)),
            mtu: Some(prns_core::interfaces::MtuPolicy::fixed(4_096)),
            ..prns_core::interfaces::ConfiguredInterfacePolicy::default()
        });
        let (near, _far) = tokio::io::duplex(64);
        let iface = TcpServerConnection::with_policy(b"peer".to_vec(), near, policy);

        assert_eq!(iface.descriptor(), policy.descriptor(iface.id()));
    }

    #[test]
    fn the_listener_status_stays_connected_between_client_sessions() {
        let status = TcpServerStatus::new(InterfaceId::from_channel_tag(
            InterfaceKind::TcpServer,
            b"127.0.0.1:4242",
        ));
        assert_eq!(
            status.connection(),
            ConnectionState::Connected,
            "a bound listener is ready before a client arrives",
        );
        assert_eq!(status.id().kind(), Some(InterfaceKind::TcpServer));

        let client = duplex_member(b"127.0.0.1:54321", tcp::TCP_BITRATE_ESTIMATE);
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
            ConnectionState::Connected,
            "the listener stays ready when its last client drops",
        );
    }

    #[tokio::test]
    async fn a_member_frames_and_deframes_across_a_real_socket() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral test port");
        let addr = listener.local_addr().expect("the bound address is known");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(tcp::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.expect("the listener accepts");
            TcpServerConnection::new(
                peer.to_string().into_bytes(),
                stream,
                tcp::TCP_BITRATE_ESTIMATE,
            )
            .run(seam)
            .await;
        });

        let mut client = TcpStream::connect(addr).await.expect("the client connects");

        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        write_framed(&mut client, &payload).await;
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the member deframes within the window")
            .expect("the member task is alive");
        assert_eq!(received, payload);

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

    #[tokio::test]
    async fn a_member_honors_kiss_framing_in_both_directions() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral test port");
        let addr = listener.local_addr().expect("the bound address is known");
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(tcp::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.expect("the listener accepts");
            TcpServerConnection::with_policy_and_framing(
                peer.to_string().into_bytes(),
                stream,
                tcp::DEFAULTS.configured(Default::default()),
                TcpWireFraming::Kiss,
            )
            .run(seam)
            .await;
        });
        let mut client = TcpStream::connect(addr).await.expect("the client connects");
        let inbound = [0x01u8, FEND, FESC, 0x02];
        let mut framed = [0u8; 32];
        let framed_len =
            kiss_framing::encode(&inbound, &mut framed).expect("encodes the inbound payload");
        client
            .write_all(&framed[..framed_len])
            .await
            .expect("writes the inbound frame");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the member deframes within the window")
            .expect("the member task is alive");
        assert_eq!(received, inbound);
        let outbound = [0xAAu8, FEND, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&outbound);
        out_tx.commit();
        let received =
            tokio::time::timeout(Duration::from_secs(2), read_kiss_deframed(&mut client))
                .await
                .expect("the frame leaves within the window");
        assert_eq!(received, outbound);
    }
}
