use std::string::String;
use std::time::Duration;

use tokio_tungstenite::connect_async;

use crate::websocket::tokio_wire;
use prns_core::interfaces::websocket::core;
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind};
use prns_core::reactor::airtime::AirtimeLedger;
use prns_core::reactor::interface_seam::{Interface, InterfaceSeam};
use prns_core::reactor::throughput::ThroughputLedger;
use prns_runtime::reactor::impls::tokio_reactor::TokioInterfaceStatus;

/// The initiating end of a Prns WebSocket pair. It dials `target` (`ws://host:port/path`),
/// upgrades to WebSocket, then carries one RNS wire frame per binary message. The target is kept as
/// the channel tag so reconnects preserve the same interface id.
pub struct WebSocketClientInterface {
    id: InterfaceId,
    target: String,
    bitrate: BitrateBps,
    reconnect: Duration,
    status: TokioInterfaceStatus,
}

impl WebSocketClientInterface {
    #[must_use]
    pub fn new(target: String, bitrate: BitrateBps, reconnect: Duration) -> Self {
        let id = InterfaceId::from_channel_tag(InterfaceKind::WebSocketClient, target.as_bytes());
        Self::new_with_id(id, target, bitrate, reconnect)
    }

    /// Build with a caller-chosen id instead of one derived from the dial target. Ordinary nodes
    /// should call [`new`](Self::new).
    #[must_use]
    pub fn new_with_id(
        id: InterfaceId,
        target: String,
        bitrate: BitrateBps,
        reconnect: Duration,
    ) -> Self {
        Self {
            id,
            target,
            bitrate,
            reconnect,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
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

impl Interface for WebSocketClientInterface {
    const HW_MTU: usize = core::WEBSOCKET_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::WebSocketClient;

    fn descriptor(&self) -> InterfaceDescriptor {
        core::descriptor(self.id, self.bitrate)
    }

    fn channel_tag(&self) -> &[u8] {
        self.target.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        loop {
            match connect_async(self.target.as_str()).await {
                Ok((socket, _response)) => {
                    log::info!("websocket-client: connected {}", self.target);
                    self.status.set_connection(ConnectionState::Connected);
                    seam.request_tunnel_synthesis().await;
                    tokio_wire::serve(
                        socket,
                        &mut seam,
                        &self.status,
                        &mut airtime,
                        &mut throughput,
                        self.bitrate,
                        started,
                    )
                    .await;
                    log::info!("websocket-client: dropped {}, retrying", self.target);
                    self.status.set_connection(ConnectionState::Disconnected);
                }
                Err(_) => {
                    log::debug!("websocket-client: connect failed {}, retrying", self.target);
                }
            }
            tokio::time::sleep(self.reconnect).await;
        }
    }
}

impl prns_core::interfaces::ReportsStatus for WebSocketClientInterface {
    fn status_view(&self) -> Option<prns_core::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![prns_core::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::{self, UnboundedSender};
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::protocol::Message;

    use prns_runtime::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};

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

    #[tokio::test]
    async fn the_client_carries_binary_frames_and_reconnects() {
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

        let interface = WebSocketClientInterface::new(
            std::format!("ws://{addr}/prns"),
            core::WEBSOCKET_BITRATE_GUESS_BPS,
            Duration::from_millis(10),
        );
        tokio::spawn(interface.run(seam));

        let (first_stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("the client connects within the window")
            .expect("the listener accepts");
        let mut first = accept_async(first_stream)
            .await
            .expect("the websocket handshake completes");

        let payload = [0x01u8, 0x02, 0x7e, 0x7d, 0x03];
        first
            .send(Message::binary(payload.to_vec()))
            .await
            .expect("writes a binary message");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface receives within the window")
            .expect("the interface task is alive");
        assert_eq!(received, payload);

        let out_payload = [0xAAu8, 0x7e, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let decoded = tokio::time::timeout(Duration::from_secs(2), next_binary(&mut first))
            .await
            .expect("the frame leaves within the window");
        assert_eq!(decoded, out_payload);

        drop(first);
        let (second_stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("the client reconnects within the window")
            .expect("the listener accepts again");
        let mut second = accept_async(second_stream)
            .await
            .expect("the second websocket handshake completes");
        second
            .send(Message::binary(b"after the reconnect".to_vec()))
            .await
            .expect("writes after reconnect");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the reconnected interface receives within the window")
            .expect("the interface task is alive");
        assert_eq!(received, b"after the reconnect");
    }
}
