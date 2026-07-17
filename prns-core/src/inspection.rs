use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;

pub use crate::engine::{AnnounceRateSnapshot, RouteSnapshot};
use crate::interfaces::ifac::IfacSize;
use crate::interfaces::{
    ConnectionState, InterfaceId, InterfaceOriginKind, InterfaceSnapshot, Membership,
    PacketPhyStats, TransferRates,
};
use crate::routing::dedup::PacketHash;
use crate::wire::DestinationHash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceIfacSnapshot {
    pub signature: [u8; 64],
    pub size: IfacSize,
    pub network_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceInventoryEntry {
    pub name: Option<String>,
    pub origin: InterfaceOriginKind,
    pub snapshot: InterfaceSnapshot,
    pub ifac: Option<InterfaceIfacSnapshot>,
}

struct FoldedInterface {
    id: InterfaceId,
    name: Option<String>,
    origin: InterfaceOriginKind,
    root: Option<InterfaceSnapshot>,
    ifac: Option<InterfaceIfacSnapshot>,
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
    fn new(id: InterfaceId, origin: InterfaceOriginKind) -> Self {
        Self {
            id,
            name: None,
            origin,
            root: None,
            ifac: None,
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

    fn add(&mut self, entry: &InterfaceInventoryEntry) {
        let snapshot = entry.snapshot;
        self.destinations = self.destinations.saturating_add(snapshot.destinations);
        self.links = self.links.saturating_add(snapshot.links);
        self.transported_links = self
            .transported_links
            .saturating_add(snapshot.transported_links);
        match snapshot.membership {
            Membership::Independent => {
                self.root = Some(snapshot);
                self.origin = entry.origin;
                if entry.name.is_some() {
                    self.name = entry.name.clone();
                }
                if entry.ifac.is_some() {
                    self.ifac = entry.ifac.clone();
                }
            }
            Membership::FleetMember { .. } => {
                if self.root.is_none() && entry.origin == InterfaceOriginKind::Discovered {
                    self.origin = InterfaceOriginKind::Discovered;
                }
                self.has_members = true;
                self.member_connection =
                    preferred_connection(self.member_connection, snapshot.connection);
                self.member_failure_reason = self.member_failure_reason.or(snapshot.failure_reason);
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
                if self.name.is_none() {
                    self.name = entry.name.clone();
                }
                if self.ifac.is_none() {
                    self.ifac = entry.ifac.clone();
                }
            }
        }
    }

    fn finish(self) -> InterfaceInventoryEntry {
        let connection = self
            .root
            .map_or(self.member_connection, |snapshot| snapshot.connection);
        let failure_reason = self
            .root
            .and_then(|snapshot| snapshot.failure_reason)
            .or(self.member_failure_reason);
        let (rx_bytes, tx_bytes, transfer_rates) = if self.has_members {
            (
                self.member_rx_bytes,
                self.member_tx_bytes,
                self.member_rates,
            )
        } else {
            self.root.map_or((0, 0, None), |snapshot| {
                (
                    snapshot.rx_bytes,
                    snapshot.tx_bytes,
                    snapshot.transfer_rates,
                )
            })
        };
        InterfaceInventoryEntry {
            name: self.name,
            origin: self.origin,
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
            ifac: self.ifac,
        }
    }
}

#[must_use]
pub fn logical_interface_inventory(
    inventory: &[InterfaceInventoryEntry],
) -> Vec<InterfaceInventoryEntry> {
    let mut folded = BTreeMap::new();
    for entry in inventory {
        let id = match entry.snapshot.membership {
            Membership::Independent => entry.snapshot.id,
            Membership::FleetMember { supervisor_id } => supervisor_id,
        };
        folded
            .entry(id)
            .or_insert_with(|| FoldedInterface::new(id, entry.origin))
            .add(entry);
    }
    let mut interfaces = folded
        .into_values()
        .map(FoldedInterface::finish)
        .collect::<Vec<_>>();
    interfaces.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.snapshot.id.cmp(&right.snapshot.id))
    });
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

pub trait InspectionSource {
    fn interface_inventory(&self) -> Vec<InterfaceInventoryEntry>;

    fn link_count(&self) -> impl core::future::Future<Output = u32> + Send;

    fn packet_phy(&self, packet_hash: PacketHash) -> Option<PacketPhyStats>;

    fn announce_rates(
        &self,
    ) -> impl core::future::Future<Output = Vec<AnnounceRateSnapshot>> + Send;

    fn routes(&self) -> impl core::future::Future<Output = Vec<RouteSnapshot>> + Send;

    fn route(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<Output = Option<RouteSnapshot>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::InterfaceKind;

    fn snapshot(
        id: InterfaceId,
        membership: Membership,
        rx_bytes: u64,
        destinations: u32,
        links: u32,
        name: Option<&str>,
    ) -> InterfaceInventoryEntry {
        InterfaceInventoryEntry {
            name: name.map(String::from),
            origin: InterfaceOriginKind::Configured,
            snapshot: InterfaceSnapshot {
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
            },
            ifac: None,
        }
    }

    #[test]
    fn fleet_members_fold_into_the_named_supervisor() {
        let supervisor = InterfaceId::from_channel_tag(InterfaceKind::TcpServer, b"server");
        let first = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"first");
        let second = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"second");
        let membership = Membership::FleetMember {
            supervisor_id: supervisor,
        };
        let snapshots = [
            snapshot(
                supervisor,
                Membership::Independent,
                100,
                0,
                0,
                Some("Public server"),
            ),
            snapshot(first, membership, 40, 2, 1, None),
            snapshot(second, membership, 60, 3, 2, None),
        ];

        let logical = logical_interface_inventory(&snapshots);

        assert_eq!(logical.len(), 1);
        assert_eq!(logical[0].name.as_deref(), Some("Public server"));
        assert_eq!(logical[0].origin, InterfaceOriginKind::Configured);
        assert_eq!(logical[0].snapshot.id, supervisor);
        assert_eq!(logical[0].snapshot.rx_bytes, 100);
        assert_eq!(logical[0].snapshot.destinations, 5);
        assert_eq!(logical[0].snapshot.links, 3);
        assert_eq!(logical[0].snapshot.membership, Membership::Independent);
    }

    #[test]
    fn discovered_origin_survives_logical_inventory_folding() {
        let id = InterfaceId::from_channel_tag(InterfaceKind::BackboneClient, b"discovered");
        let mut discovered = snapshot(
            id,
            Membership::Independent,
            100,
            2,
            1,
            Some("Discovered backbone"),
        );
        discovered.origin = InterfaceOriginKind::Discovered;

        assert_eq!(
            logical_interface_inventory(core::slice::from_ref(&discovered)),
            vec![discovered]
        );
    }
}
