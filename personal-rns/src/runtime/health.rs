use core::time::Duration;

use crate::interfaces::{ConnectionState, InterfaceKind, InterfaceSnapshot};

/// A compact, host-facing health summary for a running runtime.
///
/// The source of truth is the runtime's live [`InterfaceSnapshot`] list. Hosts can expose this over
/// Android binders, daemon JSON, CLIs, or logs without each one re-learning how to fold interface
/// state into the same operational counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeHealth {
    pub uptime_millis: u64,
    pub interface_count: u32,
    pub online_interface_count: u32,
    pub local_client_count: u32,
    pub route_count: u32,
    pub link_count: u32,
    pub transported_link_count: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bps: u64,
    pub tx_bps: u64,
}

impl RuntimeHealth {
    /// Fold a runtime snapshot list into the health shape hosts expose externally.
    #[must_use]
    pub fn from_snapshots(uptime: Duration, snapshots: &[InterfaceSnapshot]) -> Self {
        let mut health = Self {
            uptime_millis: millis_u64(uptime),
            interface_count: snapshots.len() as u32,
            ..Self::default()
        };
        for snapshot in snapshots {
            if matches!(
                snapshot.connection,
                ConnectionState::Connected | ConnectionState::Degraded
            ) {
                health.online_interface_count = health.online_interface_count.saturating_add(1);
            }
            if snapshot.id.kind() == Some(InterfaceKind::LocalClient) {
                health.local_client_count = health.local_client_count.saturating_add(1);
            }
            health.route_count = health.route_count.saturating_add(snapshot.destinations);
            health.link_count = health.link_count.saturating_add(snapshot.links);
            health.transported_link_count = health
                .transported_link_count
                .saturating_add(snapshot.transported_links);
            health.rx_bytes = health.rx_bytes.saturating_add(snapshot.rx_bytes);
            health.tx_bytes = health.tx_bytes.saturating_add(snapshot.tx_bytes);
            if let Some(rates) = snapshot.transfer_rates {
                health.rx_bps = health.rx_bps.saturating_add(u64::from(rates.rx_bps));
                health.tx_bps = health.tx_bps.saturating_add(u64::from(rates.tx_bps));
            }
        }
        health
    }
}

fn millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{InterfaceId, Membership, TransferRates};

    #[test]
    fn runtime_health_aggregates_interface_snapshots() {
        let local_client = InterfaceSnapshot {
            id: InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"app"),
            connection: ConnectionState::Connected,
            failure_reason: None,
            rx_bytes: 10,
            tx_bytes: 20,
            transfer_rates: Some(TransferRates {
                rx_bps: 3,
                tx_bps: 4,
            }),
            destinations: 2,
            links: 1,
            transported_links: 0,
            membership: Membership::Independent,
        };
        let wifi_peer = InterfaceSnapshot {
            id: InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"peer"),
            connection: ConnectionState::Reconnecting,
            failure_reason: None,
            rx_bytes: 5,
            tx_bytes: 7,
            transfer_rates: None,
            destinations: 1,
            links: 0,
            transported_links: 2,
            membership: Membership::FleetMember {
                supervisor_id: InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"wifi"),
            },
        };

        let health =
            RuntimeHealth::from_snapshots(Duration::from_millis(123), &[local_client, wifi_peer]);

        assert_eq!(health.uptime_millis, 123);
        assert_eq!(health.interface_count, 2);
        assert_eq!(health.online_interface_count, 1);
        assert_eq!(health.local_client_count, 1);
        assert_eq!(health.route_count, 3);
        assert_eq!(health.link_count, 1);
        assert_eq!(health.transported_link_count, 2);
        assert_eq!(health.rx_bytes, 15);
        assert_eq!(health.tx_bytes, 27);
        assert_eq!(health.rx_bps, 3);
        assert_eq!(health.tx_bps, 4);
    }
}
