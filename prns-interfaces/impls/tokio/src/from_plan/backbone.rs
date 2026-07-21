use prns_config::{TcpDialPlan, TcpListenPlan};

use crate::backbone::{BackboneClientInterface, BackboneServer};
use crate::host_network::{resolve_tcp_listener, tcp_target};

use super::{tcp, AttachmentResult, InterfaceConstruction};

pub(super) async fn stand_up_server(
    construction: InterfaceConstruction<'_>,
    listener: &TcpListenPlan,
) -> AttachmentResult {
    let opened = match resolve_tcp_listener(listener).await {
        Ok(bind) => BackboneServer::bind_with_policy(bind, construction.interface.policy).await,
        Err(error) => Err(error),
    };
    let server = opened?;
    let attached = construction.attach(server);
    Ok(attached.id())
}

pub(super) fn stand_up_client(
    construction: InterfaceConstruction<'_>,
    connection: &TcpDialPlan,
) -> AttachmentResult {
    let client = BackboneClientInterface::with_policy_and_connection_settings(
        tcp_target(connection),
        construction.interface.policy,
        tcp::connection_settings(connection),
    );
    let attached = construction.attach(client);
    Ok(attached.id())
}
