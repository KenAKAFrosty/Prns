use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async_with_config, WebSocketStream};

use crate::websocket::framing;
use prns_core::interfaces::websocket;
use prns_core::interfaces::websocket::WebSocketFramingSelection;
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

const WEBSOCKET_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_PENDING_HANDSHAKES: usize = 64;

pub struct WebSocketServerConnection<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    socket: Option<WebSocketStream<S>>,
    policy: EffectiveInterfacePolicy,
    framing_selection: WebSocketFramingSelection,
    status: TokioInterfaceStatus,
}

impl<S> WebSocketServerConnection<S> {
    #[must_use]
    pub fn new(
        channel_tag: Vec<u8>,
        socket: WebSocketStream<S>,
        bitrate: BitrateBps,
        framing_selection: WebSocketFramingSelection,
    ) -> Self {
        Self::with_policy(
            channel_tag,
            socket,
            websocket::policy_for_bitrate(bitrate),
            framing_selection,
        )
    }

    #[must_use]
    pub fn with_policy(
        channel_tag: Vec<u8>,
        socket: WebSocketStream<S>,
        policy: EffectiveInterfacePolicy,
        framing_selection: WebSocketFramingSelection,
    ) -> Self {
        let channel_tag = framed_channel_tag(channel_tag, framing_selection);
        let id = InterfaceId::from_channel_tag(InterfaceKind::WebSocketServerPeer, &channel_tag);
        Self {
            id,
            channel_tag,
            socket: Some(socket),
            policy,
            framing_selection,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected),
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

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> Interface
    for WebSocketServerConnection<S>
{
    const HW_MTU: usize = websocket::WEBSOCKET_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::WebSocketServerPeer;

    fn descriptor(&self) -> InterfaceDescriptor {
        websocket::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(mut self, mut seam: Seam) {
        let Some(socket) = self.socket.take() else {
            return;
        };
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        framing::serve(
            socket,
            &mut seam,
            &self.status,
            &mut airtime,
            &mut throughput,
            framing::SessionConfig::new(self.policy.bitrate, started, self.framing_selection),
        )
        .await;
        self.status.set_connection(ConnectionState::Disconnected);
    }
}

pub struct WebSocketServer {
    listener: TcpListener,
    policy: EffectiveInterfacePolicy,
    channel_tag: Vec<u8>,
    status: WebSocketServerStatus,
    framing_selection: WebSocketFramingSelection,
}

impl WebSocketServer {
    pub async fn bind(
        addr: impl tokio::net::ToSocketAddrs,
        bitrate: BitrateBps,
        framing_selection: WebSocketFramingSelection,
    ) -> io::Result<Self> {
        Self::bind_with_policy(
            addr,
            websocket::policy_for_bitrate(bitrate),
            framing_selection,
        )
        .await
    }

    pub async fn bind_with_policy(
        addr: impl tokio::net::ToSocketAddrs,
        policy: EffectiveInterfacePolicy,
        framing_selection: WebSocketFramingSelection,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let channel_tag = framed_channel_tag(
            listener.local_addr()?.to_string().into_bytes(),
            framing_selection,
        );
        let id = InterfaceId::from_channel_tag(InterfaceKind::WebSocketServer, &channel_tag);
        Ok(Self {
            listener,
            policy,
            channel_tag,
            status: WebSocketServerStatus::new(id),
            framing_selection,
        })
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::WebSocketServer, &self.channel_tag)
    }

    #[must_use]
    pub fn status(&self) -> WebSocketServerStatus {
        self.status.clone()
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

impl InterfaceSupervisor for WebSocketServer {
    const KIND: InterfaceKind = InterfaceKind::WebSocketServer;

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    fn policy(&self) -> EffectiveInterfacePolicy {
        self.policy
    }

    async fn run(self, fleet: Fleet) {
        let mut handshakes = tokio::task::JoinSet::new();
        let framing_selection = self.framing_selection;
        loop {
            tokio::select! {
                accepted = self.listener.accept(), if handshakes.len() < MAX_PENDING_HANDSHAKES => {
                    match accepted {
                        Ok((stream, peer)) => {
                            handshakes.spawn(async move {
                                let result = tokio::time::timeout(
                                    WEBSOCKET_HANDSHAKE_TIMEOUT,
                                    accept_async_with_config(
                                        stream,
                                        Some(framing::config(framing_selection)),
                                    ),
                                )
                                .await;
                                (peer, result)
                            });
                        }
                        Err(_) => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
                    }
                }
                completed = handshakes.join_next(), if !handshakes.is_empty() => {
                    let Some(completed) = completed else {
                        continue;
                    };
                    match completed {
                        Ok((peer, Ok(Ok(socket)))) => {
                        let connection = WebSocketServerConnection::with_policy(
                            peer.to_string().into_bytes(),
                            socket,
                            self.policy,
                            framing_selection,
                        );
                        self.status.admit(connection.status());
                        let _ = fleet.add(connection);
                        }
                        Ok((peer, Ok(Err(_)) | Err(_))) => {
                            crate::diagnostic_log::debug!("websocket-server: handshake failed from {peer}");
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    }
}

fn framed_channel_tag(mut tag: Vec<u8>, selection: WebSocketFramingSelection) -> Vec<u8> {
    tag.extend_from_slice(selection.channel_tag_suffix());
    tag
}

#[derive(Clone)]
pub struct WebSocketServerStatus {
    shared: Arc<WebSocketServerShared>,
}

struct WebSocketServerShared {
    id: InterfaceId,
    members: Mutex<Vec<TokioInterfaceStatus>>,
}

impl WebSocketServerStatus {
    fn new(id: InterfaceId) -> Self {
        Self {
            shared: Arc::new(WebSocketServerShared {
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

impl InterfaceStatus for WebSocketServerStatus {
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

impl prns_core::interfaces::ReportsStatus for WebSocketServer {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl<S> prns_core::interfaces::ReportsStatus for WebSocketServerConnection<S> {
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
    use futures_util::{SinkExt, StreamExt};
    use prns_core::interfaces::websocket::WebSocketWireFraming;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::{self, UnboundedSender};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::protocol::Message;

    use prns_runtime::manifold::driver::{tokio_grant_lane, TokioGrantConsumer};

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

    async fn next_binary<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> std::vec::Vec<u8>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        loop {
            match socket.next().await {
                Some(Ok(Message::Binary(frame))) => return frame.to_vec(),
                Some(Ok(_)) => {}
                Some(Err(err)) => panic!("websocket read failed: {err}"),
                None => panic!("websocket closed before a binary frame arrived"),
            }
        }
    }

    fn member_id(tag: &[u8]) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::WebSocketServerPeer, tag)
    }

    #[test]
    fn the_member_id_is_a_websocket_server_peer_kind_from_the_tag() {
        assert_eq!(
            member_id(b"127.0.0.1:54321").kind(),
            Some(InterfaceKind::WebSocketServerPeer)
        );
        assert_eq!(member_id(b"peer"), member_id(b"peer"));
        assert_ne!(member_id(b"peer-a"), member_id(b"peer-b"));
    }

    #[test]
    fn the_listener_status_stays_connected_between_client_sessions() {
        let status = WebSocketServerStatus::new(InterfaceId::from_channel_tag(
            InterfaceKind::WebSocketServer,
            b"127.0.0.1:4242",
        ));
        assert_eq!(status.connection(), ConnectionState::Connected);
        assert_eq!(status.id().kind(), Some(InterfaceKind::WebSocketServer));

        let client_status = TokioInterfaceStatus::new_unaccounted(
            member_id(b"127.0.0.1:54321"),
            ConnectionState::Connected,
        );
        status.admit(client_status.clone());
        assert_eq!(status.connection(), ConnectionState::Connected);
        assert_eq!(status.members().len(), 1);

        client_status.set_connection(ConnectionState::Disconnected);
        assert_eq!(status.connection(), ConnectionState::Connected);
    }

    #[tokio::test]
    async fn the_member_inherits_the_servers_complete_effective_policy() {
        let policy =
            websocket::configured_policy(prns_core::interfaces::ConfiguredInterfacePolicy {
                mode: Some(prns_core::interfaces::InterfaceMode::Gateway),
                bitrate: Some(BitrateBps::guess(900_000_000)),
                mtu: Some(prns_core::interfaces::MtuPolicy::fixed(4_096)),
                ..prns_core::interfaces::ConfiguredInterfacePolicy::default()
            });
        let (near, _far) = tokio::io::duplex(64);
        let socket = WebSocketStream::from_raw_socket(
            near,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        let interface = WebSocketServerConnection::with_policy(
            b"peer".to_vec(),
            socket,
            policy,
            WebSocketFramingSelection::Fixed(WebSocketWireFraming::RawPacket),
        );

        assert_eq!(interface.descriptor(), policy.descriptor(interface.id()));
    }

    #[tokio::test]
    async fn a_member_carries_binary_frames_across_a_real_websocket() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral test port");
        let addr = listener.local_addr().expect("the bound address is known");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(websocket::FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            sink: std::vec::Vec::new(),
            outbound: out_rx,
        };

        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.expect("the listener accepts");
            let socket = accept_async_with_config(
                stream,
                Some(framing::config(WebSocketFramingSelection::Fixed(
                    WebSocketWireFraming::RawPacket,
                ))),
            )
            .await
            .expect("the websocket handshake completes");
            WebSocketServerConnection::new(
                peer.to_string().into_bytes(),
                socket,
                websocket::WEBSOCKET_BITRATE_ESTIMATE,
                WebSocketFramingSelection::Fixed(WebSocketWireFraming::RawPacket),
            )
            .run(seam)
            .await;
        });

        let (mut client, _response) = connect_async(std::format!("ws://{addr}/prns"))
            .await
            .expect("the websocket client connects");

        let payload = [0x01u8, 0x02, 0x7e, 0x7d, 0x03];
        client
            .send(Message::binary(payload.to_vec()))
            .await
            .expect("writes a binary message");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the member receives within the window")
            .expect("the member task is alive");
        assert_eq!(received, payload);

        let out_payload = [0xAAu8, 0x7e, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let decoded = tokio::time::timeout(Duration::from_secs(2), next_binary(&mut client))
            .await
            .expect("the frame leaves within the window");
        assert_eq!(decoded, out_payload);
    }
}
