use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::server::ErrorResponse;
use tokio_tungstenite::tungstenite::http::{header, HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{
    accept_hdr_async_with_config, connect_async_with_config, MaybeTlsStream, WebSocketStream,
};

use prns_core::interfaces::browser_rendezvous as contract;
use prns_core::interfaces::browser_rendezvous::{
    BrowserRendezvousId, ClientHello, HelloDecodeError, ServerHello,
};
use prns_core::interfaces::websocket;
use prns_core::interfaces::websocket::{WebSocketFramingSelection, WebSocketWireFraming};
use prns_core::interfaces::{
    ConnectionState, EffectiveInterfacePolicy, InterfaceDescriptor, InterfaceId, InterfaceKind,
};
use prns_runtime::manifold::airtime::AirtimeLedger;
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_runtime::manifold::throughput::ThroughputLedger;

use crate::reconnect::ReconnectPolicy;
use crate::websocket::framing;

pub(super) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct BrowserRendezvousClient {
    id: InterfaceId,
    policy: EffectiveInterfacePolicy,
    status: TokioInterfaceStatus,
    core_id: Arc<Mutex<Option<BrowserRendezvousId>>>,
}

impl BrowserRendezvousClient {
    pub(super) fn new(policy: EffectiveInterfacePolicy) -> Self {
        let id = InterfaceId::from_channel_tag(
            InterfaceKind::WebSocketClient,
            b"browser-rendezvous-loopback-client",
        );
        Self {
            id,
            policy,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Disconnected),
            core_id: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }

    pub(super) fn core_id(&self) -> Arc<Mutex<Option<BrowserRendezvousId>>> {
        self.core_id.clone()
    }
}

impl Interface for BrowserRendezvousClient {
    const HW_MTU: usize = websocket::WEBSOCKET_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::WebSocketClient;

    fn descriptor(&self) -> InterfaceDescriptor {
        websocket::descriptor(self.id, self.policy)
    }

    fn channel_tag(&self) -> &[u8] {
        b"browser-rendezvous-loopback-client"
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let mut reconnect = ReconnectPolicy::STANDARD.schedule();
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        loop {
            if let Ok(Ok((socket, core_id))) =
                tokio::time::timeout(HANDSHAKE_TIMEOUT, connect()).await
            {
                if let Ok(mut current) = self.core_id.lock() {
                    *current = Some(core_id);
                }
                let connected_at = tokio::time::Instant::now();
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
                        WebSocketFramingSelection::Fixed(WebSocketWireFraming::RawPacket),
                    ),
                )
                .await;
                self.status.set_connection(ConnectionState::Disconnected);
                if let Ok(mut current) = self.core_id.lock() {
                    *current = None;
                }
                reconnect.record_connection_lifetime(connected_at.elapsed());
            }
            let delay = reconnect.next_delay(|bytes| seam.fill_entropy(bytes));
            tokio::time::sleep(delay).await;
        }
    }
}

impl prns_core::interfaces::ReportsStatus for BrowserRendezvousClient {
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

pub(super) async fn accept(
    stream: TcpStream,
    id: BrowserRendezvousId,
) -> Result<WebSocketStream<TcpStream>, BrowserTransportHandshakeError> {
    let mut socket = accept_hdr_async_with_config(
        stream,
        validate_upgrade,
        Some(framing::config(WebSocketFramingSelection::Fixed(
            WebSocketWireFraming::RawPacket,
        ))),
    )
    .await
    .map_err(BrowserTransportHandshakeError::WebSocket)?;
    let message = next_message(&mut socket).await?;
    let Message::Binary(bytes) = message else {
        return Err(BrowserTransportHandshakeError::UnexpectedFrame);
    };
    ClientHello::decode(&bytes).map_err(BrowserTransportHandshakeError::Hello)?;
    socket
        .send(Message::binary(ServerHello::new(id).encode().to_vec()))
        .await
        .map_err(BrowserTransportHandshakeError::WebSocket)?;
    Ok(socket)
}

#[expect(
    clippy::result_large_err,
    reason = "tungstenite requires its concrete HTTP rejection response in this callback"
)]
fn validate_upgrade(
    request: &tokio_tungstenite::tungstenite::handshake::server::Request,
    mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<tokio_tungstenite::tungstenite::handshake::server::Response, ErrorResponse> {
    let uri = request.uri();
    if uri.path() != contract::PATH
        || uri.query().is_some()
        || uri.scheme().is_some()
        || uri.authority().is_some()
    {
        return Err(rejection(StatusCode::NOT_FOUND));
    }
    let offered = request
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .any(|protocol| protocol.trim() == contract::SUBPROTOCOL)
        })
        .unwrap_or(false);
    if !offered {
        return Err(rejection(StatusCode::BAD_REQUEST));
    }
    response.headers_mut().insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(contract::SUBPROTOCOL),
    );
    Ok(response)
}

async fn connect() -> Result<
    (
        WebSocketStream<MaybeTlsStream<TcpStream>>,
        BrowserRendezvousId,
    ),
    BrowserTransportHandshakeError,
> {
    let target = format!("ws://127.0.0.1:{}{}", contract::PORT, contract::PATH);
    let mut request = target
        .into_client_request()
        .map_err(BrowserTransportHandshakeError::WebSocket)?;
    request.headers_mut().insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(contract::SUBPROTOCOL),
    );
    let (mut socket, response) = connect_async_with_config(
        request,
        Some(framing::config(WebSocketFramingSelection::Fixed(
            WebSocketWireFraming::RawPacket,
        ))),
        false,
    )
    .await
    .map_err(BrowserTransportHandshakeError::WebSocket)?;
    let selected = response
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok());
    if selected != Some(contract::SUBPROTOCOL) {
        return Err(BrowserTransportHandshakeError::Subprotocol);
    }
    socket
        .send(Message::binary(ClientHello::encode().to_vec()))
        .await
        .map_err(BrowserTransportHandshakeError::WebSocket)?;
    let message = next_message(&mut socket).await?;
    let Message::Binary(bytes) = message else {
        return Err(BrowserTransportHandshakeError::UnexpectedFrame);
    };
    let hello = ServerHello::decode(&bytes).map_err(BrowserTransportHandshakeError::Hello)?;
    Ok((socket, hello.id()))
}

async fn next_message<S>(
    socket: &mut WebSocketStream<S>,
) -> Result<Message, BrowserTransportHandshakeError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, socket.next()).await {
        Ok(Some(Ok(message))) => Ok(message),
        Ok(Some(Err(error))) => Err(BrowserTransportHandshakeError::WebSocket(error)),
        Ok(None) => Err(BrowserTransportHandshakeError::Closed),
        Err(_) => Err(BrowserTransportHandshakeError::TimedOut),
    }
}

fn rejection(status: StatusCode) -> ErrorResponse {
    let mut response = ErrorResponse::new(None);
    *response.status_mut() = status;
    response
}

#[derive(Debug)]
pub(super) enum BrowserTransportHandshakeError {
    WebSocket(tokio_tungstenite::tungstenite::Error),
    Hello(HelloDecodeError),
    Subprotocol,
    UnexpectedFrame,
    Closed,
    TimedOut,
}

impl std::fmt::Display for BrowserTransportHandshakeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WebSocket(error) => error.fmt(formatter),
            Self::Hello(error) => error.fmt(formatter),
            Self::Subprotocol => formatter.write_str("rendezvous subprotocol was not selected"),
            Self::UnexpectedFrame => {
                formatter.write_str("rendezvous hello arrived in an unsupported WebSocket frame")
            }
            Self::Closed => formatter.write_str("rendezvous closed during the hello"),
            Self::TimedOut => formatter.write_str("rendezvous hello timed out"),
        }
    }
}

impl std::error::Error for BrowserTransportHandshakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WebSocket(error) => Some(error),
            Self::Hello(error) => Some(error),
            Self::Subprotocol | Self::UnexpectedFrame | Self::Closed | Self::TimedOut => None,
        }
    }
}
