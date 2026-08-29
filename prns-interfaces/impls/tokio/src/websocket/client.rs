use std::string::String;
use std::time::Duration;

use tokio_tungstenite::connect_async_with_config;

use crate::reconnect::ReconnectPolicy;
use crate::websocket::framing;
use prns_core::interfaces::websocket;
use prns_core::interfaces::websocket::WebSocketFramingSelection;
use prns_core::interfaces::BitrateBps;
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

const WEBSOCKET_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub struct WebSocketClientInterface {
    id: InterfaceId,
    target: String,
    channel_tag: Vec<u8>,
    policy: EffectiveInterfacePolicy,
    reconnect_policy: ReconnectPolicy,
    framing_selection: WebSocketFramingSelection,
    status: TokioInterfaceStatus,
}

impl WebSocketClientInterface {
    #[must_use]
    pub fn new(
        target: String,
        bitrate: BitrateBps,
        reconnect_policy: ReconnectPolicy,
        framing_selection: WebSocketFramingSelection,
    ) -> Self {
        let channel_tag = channel_tag(&target, framing_selection);
        let id = InterfaceId::from_channel_tag(InterfaceKind::WebSocketClient, &channel_tag);
        Self::with_id_and_policy(
            id,
            target,
            websocket::policy_for_bitrate(bitrate),
            reconnect_policy,
            framing_selection,
        )
    }

    #[must_use]
    pub fn with_policy(
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect_policy: ReconnectPolicy,
        framing_selection: WebSocketFramingSelection,
    ) -> Self {
        let channel_tag = channel_tag(&target, framing_selection);
        let id = InterfaceId::from_channel_tag(InterfaceKind::WebSocketClient, &channel_tag);
        Self::with_id_and_policy(id, target, policy, reconnect_policy, framing_selection)
    }

    #[must_use]
    pub fn new_with_id(
        id: InterfaceId,
        target: String,
        bitrate: BitrateBps,
        reconnect_policy: ReconnectPolicy,
        framing_selection: WebSocketFramingSelection,
    ) -> Self {
        Self::with_id_and_policy(
            id,
            target,
            websocket::policy_for_bitrate(bitrate),
            reconnect_policy,
            framing_selection,
        )
    }

    #[must_use]
    pub fn with_id_and_policy(
        id: InterfaceId,
        target: String,
        policy: EffectiveInterfacePolicy,
        reconnect_policy: ReconnectPolicy,
        framing_selection: WebSocketFramingSelection,
    ) -> Self {
        let channel_tag = channel_tag(&target, framing_selection);
        Self {
            id,
            target,
            channel_tag,
            policy,
            reconnect_policy,
            framing_selection,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Initializing),
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
    const HW_MTU: usize = websocket::WEBSOCKET_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::WebSocketClient;

    fn descriptor(&self) -> InterfaceDescriptor {
        websocket::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let interface_origin = seam.interface_origin().as_str();
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut reconnect = self.reconnect_policy.schedule();
        loop {
            let connect = tokio::time::timeout(
                WEBSOCKET_HANDSHAKE_TIMEOUT,
                connect_async_with_config(
                    self.target.as_str(),
                    Some(framing::config(self.framing_selection)),
                    false,
                ),
            );
            #[cfg(feature = "tracing")]
            let connected = tracing::Instrument::instrument(
                connect,
                tracing::debug_span!(
                    target: "prns.interface",
                    "prns.interface.connect",
                    interface_kind = "websocket_client",
                    interface_origin,
                    peer = %self.target,
                ),
            )
            .await;
            #[cfg(not(feature = "tracing"))]
            let connected = connect.await;
            match connected {
                Ok(Ok((socket, _response))) => {
                    let connected_at = tokio::time::Instant::now();
                    crate::diagnostic_log::debug!(
                        "websocket-client [{interface_origin}]: connected {}",
                        self.target
                    );
                    self.status.set_connection(ConnectionState::Connected);
                    seam.request_tunnel_synthesis().await;
                    framing::serve(
                        socket,
                        &mut seam,
                        &self.status,
                        &mut airtime,
                        &mut throughput,
                        framing::SessionConfig::new(
                            self.policy.bitrate,
                            started,
                            self.framing_selection,
                        ),
                    )
                    .await;
                    crate::diagnostic_log::debug!(
                        "websocket-client [{interface_origin}]: dropped {}, retrying",
                        self.target
                    );
                    self.status.set_connection(ConnectionState::Disconnected);
                    reconnect.record_connection_lifetime(connected_at.elapsed());
                }
                Ok(Err(_)) | Err(_) => {
                    crate::diagnostic_log::debug!(
                        "websocket-client [{interface_origin}]: connect failed {}, retrying",
                        self.target
                    );
                }
            }
            let reconnect_delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
            tokio::time::sleep(reconnect_delay).await;
        }
    }
}

fn channel_tag(target: &str, selection: WebSocketFramingSelection) -> Vec<u8> {
    let mut tag = target.as_bytes().to_vec();
    tag.extend_from_slice(selection.channel_tag_suffix());
    tag
}

impl prns_core::interfaces::ReportsStatus for WebSocketClientInterface {
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
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::{self, UnboundedSender};
    use tokio_tungstenite::accept_async;
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

    #[tokio::test]
    async fn the_client_carries_binary_frames_and_reconnects() {
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

        let interface = WebSocketClientInterface::new(
            std::format!("ws://{addr}/prns"),
            websocket::WEBSOCKET_BITRATE_ESTIMATE,
            ReconnectPolicy::STANDARD,
            WebSocketFramingSelection::Fixed(WebSocketWireFraming::RawPacket),
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

    #[tokio::test]
    async fn the_secure_client_initializes_tls_and_rejects_an_invalid_peer() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds an ephemeral test port");
        let addr = listener.local_addr().expect("the bound address is known");
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accepts the TLS client");
            drop(stream);
        });

        let outcome = tokio::time::timeout(
            Duration::from_secs(if cfg!(windows) { 20 } else { 2 }),
            connect_async_with_config(
                std::format!("wss://localhost:{}/prns", addr.port()),
                Some(framing::config(WebSocketFramingSelection::Fixed(
                    WebSocketWireFraming::RawPacket,
                ))),
                false,
            ),
        )
        .await
        .expect("the invalid TLS peer is rejected within the window");
        assert!(outcome.is_err());
        peer.await.expect("the test peer exits cleanly");
    }
}
