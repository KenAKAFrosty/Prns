#[cfg(feature = "websocket")]
use prns_config::{TcpListenPlan, WebSocketTargetPlan};

#[cfg(feature = "websocket")]
use crate::host_network::resolve_tcp_listener;
#[cfg(feature = "websocket")]
use crate::websocket::{WebSocketClientInterface, WebSocketServer};
#[cfg(feature = "websocket")]
use prns_core::interfaces::websocket::WebSocketWireFraming;

#[cfg(feature = "websocket")]
use super::{AttachmentResult, InterfaceConstruction, RECONNECT_POLICY};

#[cfg(feature = "websocket")]
pub(super) fn stand_up_client(
    construction: InterfaceConstruction<'_>,
    target: &WebSocketTargetPlan,
    framing: WebSocketWireFraming,
) -> AttachmentResult {
    let websocket = WebSocketClientInterface::with_policy(
        target.as_str().to_string(),
        construction.interface.policy,
        RECONNECT_POLICY,
        framing,
    );
    let attached = construction.attach(websocket);
    Ok(attached.id())
}

#[cfg(feature = "websocket")]
pub(super) async fn stand_up_server(
    construction: InterfaceConstruction<'_>,
    listener: &TcpListenPlan,
    framing: WebSocketWireFraming,
) -> AttachmentResult {
    let opened = match resolve_tcp_listener(listener).await {
        Ok(bind) => {
            WebSocketServer::bind_with_policy(bind, construction.interface.policy, framing).await
        }
        Err(error) => Err(error),
    };
    let server = opened?;
    let attached = construction.attach(server);
    Ok(attached.id())
}
