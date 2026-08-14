use prns_config::{TcpListenPlan, WebSocketTargetPlan};

use crate::host_network::resolve_tcp_listener;
use crate::websocket::{WebSocketClientInterface, WebSocketServer};
use prns_core::interfaces::websocket::{WebSocketFramingSelection, WebSocketWireFraming};

use super::{AttachmentResult, InterfaceConstruction, RECONNECT_POLICY};

pub(super) fn stand_up_client(
    construction: InterfaceConstruction<'_>,
    target: &WebSocketTargetPlan,
    framing: WebSocketWireFraming,
) -> AttachmentResult {
    let websocket = WebSocketClientInterface::with_policy(
        target.as_str().to_string(),
        construction.interface.policy,
        RECONNECT_POLICY,
        WebSocketFramingSelection::Fixed(framing),
    );
    let attached = construction.attach(websocket);
    Ok(attached.id())
}

pub(super) async fn stand_up_server(
    construction: InterfaceConstruction<'_>,
    listener: &TcpListenPlan,
    framing: WebSocketWireFraming,
) -> AttachmentResult {
    let opened = match resolve_tcp_listener(listener).await {
        Ok(bind) => {
            WebSocketServer::bind_with_policy(
                bind,
                construction.interface.policy,
                WebSocketFramingSelection::Fixed(framing),
            )
            .await
        }
        Err(error) => Err(error),
    };
    let server = opened?;
    let attached = construction.attach(server);
    Ok(attached.id())
}
