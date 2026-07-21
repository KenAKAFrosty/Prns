use core::time::Duration;

use prns_config::{
    AddressFamilyPreference as PlannedAddressFamilyPreference,
    ReconnectLimit as PlannedReconnectLimit, TcpDialPlan, TcpListenPlan,
    TcpTunnelMode as PlannedTcpTunnelMode,
};
use prns_core::interfaces::tcp::TcpWireFraming;

use crate::host_network::{resolve_tcp_listener, tcp_target};
use crate::tcp::{
    AddressFamilyPreference, ReconnectLimit, TcpClientInterface, TcpConnectionSettings, TcpServer,
    TcpTunnelMode,
};

use super::{AttachmentResult, InterfaceConstruction, RECONNECT_POLICY};

pub(super) fn stand_up_client(
    construction: InterfaceConstruction<'_>,
    connection: &TcpDialPlan,
    framing: TcpWireFraming,
) -> AttachmentResult {
    let client = TcpClientInterface::with_policy_and_connection_settings(
        tcp_target(connection),
        construction.interface.policy,
        framing,
        connection_settings(connection),
    );
    let attached = construction.attach(client);
    Ok(attached.id())
}

pub(super) async fn stand_up_server(
    construction: InterfaceConstruction<'_>,
    listener: &TcpListenPlan,
    framing: TcpWireFraming,
) -> AttachmentResult {
    let opened = match resolve_tcp_listener(listener).await {
        Ok(bind) => {
            TcpServer::bind_with_policy_and_tunnel_and_framing(
                bind,
                construction.interface.policy,
                tunnel_mode(listener.tunnel),
                framing,
            )
            .await
        }
        Err(error) => Err(error),
    };
    let server = opened?;
    let attached = construction.attach(server);
    Ok(attached.id())
}

pub(super) fn connection_settings(plan: &TcpDialPlan) -> TcpConnectionSettings {
    TcpConnectionSettings {
        connect_timeout: Duration::from_secs(plan.connect_timeout.get()),
        reconnect_policy: RECONNECT_POLICY,
        reconnect_limit: match plan.reconnect_limit {
            PlannedReconnectLimit::Unlimited => ReconnectLimit::Unlimited,
            PlannedReconnectLimit::Attempts(attempts) => ReconnectLimit::Attempts(attempts),
        },
        address_family: match plan.address_family {
            PlannedAddressFamilyPreference::System => AddressFamilyPreference::System,
            PlannedAddressFamilyPreference::Ipv4 => AddressFamilyPreference::Ipv4,
            PlannedAddressFamilyPreference::Ipv6 => AddressFamilyPreference::Ipv6,
        },
        tunnel: tunnel_mode(plan.tunnel),
    }
}

const fn tunnel_mode(mode: PlannedTcpTunnelMode) -> TcpTunnelMode {
    match mode {
        PlannedTcpTunnelMode::Direct => TcpTunnelMode::Direct,
        PlannedTcpTunnelMode::I2p => TcpTunnelMode::I2p,
    }
}
