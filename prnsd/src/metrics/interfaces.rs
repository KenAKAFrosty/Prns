use std::collections::{BTreeMap, HashMap};

use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceSnapshot, Membership, TransferRates,
};

pub(super) struct LogicalInterface {
    pub name: String,
    pub snapshot: InterfaceSnapshot,
}

struct FoldedInterface {
    id: InterfaceId,
    root: Option<InterfaceSnapshot>,
    member_connection: ConnectionState,
    member_failure_reason: Option<&'static str>,
    member_rx_bytes: u64,
    member_tx_bytes: u64,
    member_rates: Option<TransferRates>,
    has_members: bool,
    destinations: u32,
    links: u32,
    transported_links: u32,
}

impl FoldedInterface {
    fn new(id: InterfaceId) -> Self {
        Self {
            id,
            root: None,
            member_connection: ConnectionState::Unknown,
            member_failure_reason: None,
            member_rx_bytes: 0,
            member_tx_bytes: 0,
            member_rates: None,
            has_members: false,
            destinations: 0,
            links: 0,
            transported_links: 0,
        }
    }

    fn add(&mut self, snapshot: InterfaceSnapshot) {
        self.destinations = self.destinations.saturating_add(snapshot.destinations);
        self.links = self.links.saturating_add(snapshot.links);
        self.transported_links = self
            .transported_links
            .saturating_add(snapshot.transported_links);
        match snapshot.membership {
            Membership::Independent => self.root = Some(snapshot),
            Membership::FleetMember { .. } => {
                self.has_members = true;
                self.member_connection =
                    preferred_connection(self.member_connection, snapshot.connection);
                self.member_failure_reason = self
                    .member_failure_reason
                    .or(snapshot.failure_reason);
                self.member_rx_bytes = self.member_rx_bytes.saturating_add(snapshot.rx_bytes);
                self.member_tx_bytes = self.member_tx_bytes.saturating_add(snapshot.tx_bytes);
                if let Some(rates) = snapshot.transfer_rates {
                    let aggregate = self.member_rates.get_or_insert(TransferRates {
                        rx_bps: 0,
                        tx_bps: 0,
                    });
                    aggregate.rx_bps = aggregate.rx_bps.saturating_add(rates.rx_bps);
                    aggregate.tx_bps = aggregate.tx_bps.saturating_add(rates.tx_bps);
                }
            }
        }
    }

    fn finish(self, names: &HashMap<InterfaceId, String>) -> LogicalInterface {
        let root = self.root;
        let connection = root.map_or(self.member_connection, |snapshot| snapshot.connection);
        let failure_reason = root
            .and_then(|snapshot| snapshot.failure_reason)
            .or(self.member_failure_reason);
        let (rx_bytes, tx_bytes, transfer_rates) = if self.has_members {
            (
                self.member_rx_bytes,
                self.member_tx_bytes,
                self.member_rates,
            )
        } else {
            root.map_or((0, 0, None), |snapshot| {
                (
                    snapshot.rx_bytes,
                    snapshot.tx_bytes,
                    snapshot.transfer_rates,
                )
            })
        };
        LogicalInterface {
            name: names
                .get(&self.id)
                .cloned()
                .unwrap_or_else(|| fallback_name(self.id)),
            snapshot: InterfaceSnapshot {
                id: self.id,
                connection,
                failure_reason,
                rx_bytes,
                tx_bytes,
                transfer_rates,
                destinations: self.destinations,
                links: self.links,
                transported_links: self.transported_links,
                membership: Membership::Independent,
            },
        }
    }
}

pub(super) fn logical_interfaces(
    snapshots: &[InterfaceSnapshot],
    names: &HashMap<InterfaceId, String>,
) -> Vec<LogicalInterface> {
    let mut folded = BTreeMap::new();
    for snapshot in snapshots {
        let id = match snapshot.membership {
            Membership::Independent => snapshot.id,
            Membership::FleetMember { supervisor_id } => supervisor_id,
        };
        folded
            .entry(id)
            .or_insert_with(|| FoldedInterface::new(id))
            .add(*snapshot);
    }
    let mut interfaces = folded
        .into_values()
        .map(|interface| interface.finish(names))
        .collect::<Vec<_>>();
    interfaces.sort_by(|left, right| left.name.cmp(&right.name));
    interfaces
}

fn preferred_connection(left: ConnectionState, right: ConnectionState) -> ConnectionState {
    if connection_rank(left) <= connection_rank(right) {
        left
    } else {
        right
    }
}

fn connection_rank(state: ConnectionState) -> u8 {
    match state {
        ConnectionState::Connected => 0,
        ConnectionState::Degraded => 1,
        ConnectionState::Initializing => 2,
        ConnectionState::Reconnecting => 3,
        ConnectionState::Failed => 4,
        ConnectionState::Disconnected => 5,
        ConnectionState::Disabled => 6,
        ConnectionState::Unknown => 7,
    }
}

fn fallback_name(id: InterfaceId) -> String {
    match id.kind() {
        Some(InterfaceKind::LocalServer | InterfaceKind::LocalClient) => {
            String::from("Shared instance")
        }
        Some(kind) => String::from(super::interface_kind_name(kind)),
        None => String::from("unknown"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        id: InterfaceId,
        membership: Membership,
        rx_bytes: u64,
        destinations: u32,
        links: u32,
    ) -> InterfaceSnapshot {
        InterfaceSnapshot {
            id,
            connection: ConnectionState::Connected,
            failure_reason: None,
            rx_bytes,
            tx_bytes: rx_bytes / 2,
            transfer_rates: Some(TransferRates {
                rx_bps: rx_bytes as u32,
                tx_bps: (rx_bytes / 2) as u32,
            }),
            destinations,
            links,
            transported_links: 0,
            membership,
        }
    }

    #[test]
    fn fleet_members_supply_counts_without_double_counting_supervisor_traffic() {
        let supervisor = InterfaceId::from_channel_tag(InterfaceKind::TcpServer, b"server");
        let first = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"first");
        let second = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"second");
        let membership = Membership::FleetMember {
            supervisor_id: supervisor,
        };
        let snapshots = [
            snapshot(supervisor, Membership::Independent, 100, 0, 0),
            snapshot(first, membership, 40, 2, 1),
            snapshot(second, membership, 60, 3, 2),
        ];
        let names = HashMap::from([(supervisor, String::from("Public server"))]);

        let logical = logical_interfaces(&snapshots, &names);

        assert_eq!(logical.len(), 1);
        assert_eq!(logical[0].name, "Public server");
        assert_eq!(logical[0].snapshot.rx_bytes, 100);
        assert_eq!(logical[0].snapshot.destinations, 5);
        assert_eq!(logical[0].snapshot.links, 3);
    }
}
