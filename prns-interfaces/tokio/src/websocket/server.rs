use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, WebSocketStream};

use personal_rns::interfaces::websocket::core;
use crate::websocket::tokio_wire;
use personal_rns::interfaces::{
    ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind, InterfaceStatus, TransferRates,
};
use personal_rns::reactor::airtime::AirtimeLedger;
use personal_rns::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use personal_rns::reactor::interface_seam::{Interface, InterfaceSeam};
use personal_rns::reactor::throughput::ThroughputLedger;
use personal_rns::runtime::{Fleet, InterfaceSupervisor};

/// One peer accepted by a [`WebSocketServer`]. A peer is a distinct engine interface over an
/// already-upgraded WebSocket stream, and it carries one RNS wire frame per binary message.
pub struct WebSocketServerConnection<S> {
    id: InterfaceId,
    channel_tag: Vec<u8>,
    socket: Option<WebSocketStream<S>>,
    bitrate_bps: u32,
    status: TokioInterfaceStatus,
}

impl<S> WebSocketServerConnection<S> {
    #[must_use]
    pub fn new(channel_tag: Vec<u8>, socket: WebSocketStream<S>, bitrate_bps: u32) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::WebSocketServerPeer, &channel_tag);
        Self {
            id,
            channel_tag,
            socket: Some(socket),
            bitrate_bps,
            status: TokioInterfaceStatus::new(id, ConnectionState::Connected),
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
    const HW_MTU: usize = core::WEBSOCKET_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::WebSocketServerPeer;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, self.bitrate_bps)
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
        tokio_wire::serve(
            socket,
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

/// A plain `ws://` listener that upgrades accepted TCP connections and stands up one
/// [`WebSocketServerConnection`] per peer. Use an HTTP/TLS reverse proxy when the public edge needs
/// `wss://`; the Prns wire inside each WebSocket message stays the same.
pub struct WebSocketServer {
    listener: TcpListener,
    bitrate_bps: u32,
    channel_tag: Vec<u8>,
    status: WebSocketServerStatus,
}

impl WebSocketServer {
    pub async fn bind(addr: impl tokio::net::ToSocketAddrs, bitrate_bps: u32) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let channel_tag = listener.local_addr()?.to_string().into_bytes();
        let id = InterfaceId::from_channel_tag(InterfaceKind::WebSocketServer, &channel_tag);
        Ok(Self {
            listener,
            bitrate_bps,
            channel_tag,
            status: WebSocketServerStatus::new(id),
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

    async fn run(self, fleet: Fleet) {
        loop {
            match self.listener.accept().await {
                Ok((stream, peer)) => match accept_async(stream).await {
                    Ok(socket) => {
                        let connection = WebSocketServerConnection::new(
                            peer.to_string().into_bytes(),
                            socket,
                            self.bitrate_bps,
                        );
                        self.status.admit(connection.status());
                        let _ = fleet.add(connection);
                    }
                    Err(_) => {
                        log::debug!("websocket-server: handshake failed from {peer}");
                    }
                },
                Err(_) => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
            }
        }
    }
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

impl personal_rns::interfaces::ReportsStatus for WebSocketServer {
    fn status_view(&self) -> Option<personal_rns::interfaces::StatusView> {
        let status = self.status.clone();
        Some(std::sync::Arc::new(move || {
            std::vec![personal_rns::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

impl<S> personal_rns::interfaces::ReportsStatus for WebSocketServerConnection<S> {
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
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::{self, UnboundedSender};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::protocol::Message;

    use personal_rns::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};

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
    fn the_aggregate_status_is_dormant_until_a_client_connects() {
        let status = WebSocketServerStatus::new(InterfaceId::from_channel_tag(
            InterfaceKind::WebSocketServer,
            b"127.0.0.1:4242",
        ));
        assert_eq!(status.connection(), ConnectionState::Disconnected);
        assert_eq!(status.id().kind(), Some(InterfaceKind::WebSocketServer));

        let client_status =
            TokioInterfaceStatus::new(member_id(b"127.0.0.1:54321"), ConnectionState::Connected);
        status.admit(client_status.clone());
        assert_eq!(status.connection(), ConnectionState::Connected);
        assert_eq!(status.members().len(), 1);

        client_status.set_connection(ConnectionState::Disconnected);
        assert_eq!(status.connection(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn a_member_carries_binary_frames_across_a_real_websocket() {
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

        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.expect("the listener accepts");
            let socket = accept_async(stream)
                .await
                .expect("the websocket handshake completes");
            WebSocketServerConnection::new(
                peer.to_string().into_bytes(),
                socket,
                core::WEBSOCKET_BITRATE_GUESS_BPS,
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
