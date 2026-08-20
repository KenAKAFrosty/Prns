use prns_core::interfaces::IfacSize;
use prns_core::interfaces::{
    ConnectionState, InterfaceGravity, InterfaceId, InterfaceMode, InterfaceOriginKind,
    InterfaceSnapshot, Membership, TransferRates,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceIfacSnapshot<Label> {
    pub signature: [u8; 64],
    pub size: IfacSize,
    pub network_name: Option<Label>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceInventoryEntry<Label> {
    pub name: Option<Label>,
    pub origin: InterfaceOriginKind,
    pub snapshot: InterfaceSnapshot,
    pub ifac: Option<InterfaceIfacSnapshot<Label>>,
}

struct FoldedInterface<Label> {
    id: InterfaceId,
    name: Option<Label>,
    origin: InterfaceOriginKind,
    root: Option<InterfaceSnapshot>,
    ifac: Option<InterfaceIfacSnapshot<Label>>,
    member_connection: ConnectionState,
    member_mode: Option<InterfaceMode>,
    member_gravity: Option<InterfaceGravity>,
    member_failure_reason: Option<&'static str>,
    member_rx_bytes: u64,
    member_tx_bytes: u64,
    member_rates: Option<TransferRates>,
    has_members: bool,
    destinations: u32,
    links: u32,
    transported_links: u32,
}

impl<Label> FoldedInterface<Label> {
    fn new(id: InterfaceId, origin: InterfaceOriginKind) -> Self {
        Self {
            id,
            name: None,
            origin,
            root: None,
            ifac: None,
            member_connection: ConnectionState::Unknown,
            member_mode: None,
            member_gravity: None,
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

    fn add(&mut self, entry: &mut InterfaceInventoryEntry<Label>) {
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
                    self.name = entry.name.take();
                }
                if entry.ifac.is_some() {
                    self.ifac = entry.ifac.take();
                }
            }
            Membership::FleetMember { .. } => {
                if self.root.is_none() && entry.origin == InterfaceOriginKind::Discovered {
                    self.origin = InterfaceOriginKind::Discovered;
                }
                self.has_members = true;
                self.member_mode.get_or_insert(snapshot.mode);
                self.member_gravity.get_or_insert(snapshot.gravity);
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
                    self.name = entry.name.take();
                }
                if self.ifac.is_none() {
                    self.ifac = entry.ifac.take();
                }
            }
        }
    }

    fn finish(self) -> InterfaceInventoryEntry<Label> {
        let connection = self
            .root
            .map_or(self.member_connection, |snapshot| snapshot.connection);
        let failure_reason = self
            .root
            .and_then(|snapshot| snapshot.failure_reason)
            .or(self.member_failure_reason);
        let mode = self
            .root
            .map(|snapshot| snapshot.mode)
            .or(self.member_mode)
            .unwrap_or(InterfaceMode::Full);
        let gravity = self
            .root
            .map(|snapshot| snapshot.gravity)
            .or(self.member_gravity)
            .unwrap_or(InterfaceGravity::ZERO);
        let root_rx_bytes = self.root.map_or(0, |snapshot| snapshot.rx_bytes);
        let root_tx_bytes = self.root.map_or(0, |snapshot| snapshot.tx_bytes);
        let rx_bytes = root_rx_bytes.saturating_add(self.member_rx_bytes);
        let tx_bytes = root_tx_bytes.saturating_add(self.member_tx_bytes);
        let transfer_rates = if self.has_members {
            self.member_rates
        } else {
            self.root.and_then(|snapshot| snapshot.transfer_rates)
        };
        InterfaceInventoryEntry {
            name: self.name,
            origin: self.origin,
            snapshot: InterfaceSnapshot {
                id: self.id,
                mode,
                gravity,
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
pub fn fold_logical_interface_inventory<Label: Ord>(
    inventory: &mut [InterfaceInventoryEntry<Label>],
) -> &mut [InterfaceInventoryEntry<Label>] {
    inventory.sort_unstable_by(|left, right| {
        logical_interface_id(left)
            .cmp(&logical_interface_id(right))
            .then_with(|| membership_rank(left).cmp(&membership_rank(right)))
            .then_with(|| left.snapshot.id.cmp(&right.snapshot.id))
    });

    let mut read = 0;
    let mut write = 0;
    while read < inventory.len() {
        let logical_id = logical_interface_id(&inventory[read]);
        let mut end = read + 1;
        while end < inventory.len() && logical_interface_id(&inventory[end]) == logical_id {
            end += 1;
        }
        let mut folded = FoldedInterface::new(logical_id, inventory[read].origin);
        for entry in &mut inventory[read..end] {
            folded.add(entry);
        }
        inventory[write] = folded.finish();
        write += 1;
        read = end;
    }

    let logical = &mut inventory[..write];
    logical.sort_unstable_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.snapshot.id.cmp(&right.snapshot.id))
    });
    logical
}

fn logical_interface_id<Label>(entry: &InterfaceInventoryEntry<Label>) -> InterfaceId {
    match entry.snapshot.membership {
        Membership::Independent => entry.snapshot.id,
        Membership::FleetMember { supervisor_id } => supervisor_id,
    }
}

fn membership_rank<Label>(entry: &InterfaceInventoryEntry<Label>) -> u8 {
    match entry.snapshot.membership {
        Membership::Independent => 0,
        Membership::FleetMember { .. } => 1,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::InterfaceKind;

    fn snapshot(
        id: InterfaceId,
        membership: Membership,
        rx_bytes: u64,
        destinations: u32,
        links: u32,
        name: Option<&'static str>,
    ) -> InterfaceInventoryEntry<&'static str> {
        InterfaceInventoryEntry {
            name,
            origin: InterfaceOriginKind::Configured,
            snapshot: InterfaceSnapshot {
                id,
                mode: InterfaceMode::Full,
                gravity: InterfaceGravity::ZERO,
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
        let mut snapshots = [
            snapshot(second, membership, 60, 3, 2, None),
            snapshot(
                supervisor,
                Membership::Independent,
                10,
                0,
                0,
                Some("Public server"),
            ),
            snapshot(first, membership, 40, 2, 1, None),
        ];

        let logical = fold_logical_interface_inventory(&mut snapshots);

        assert_eq!(logical.len(), 1);
        assert_eq!(logical[0].name, Some("Public server"));
        assert_eq!(logical[0].origin, InterfaceOriginKind::Configured);
        assert_eq!(logical[0].snapshot.id, supervisor);
        assert_eq!(logical[0].snapshot.rx_bytes, 110);
        assert_eq!(logical[0].snapshot.destinations, 5);
        assert_eq!(logical[0].snapshot.links, 3);
        assert_eq!(logical[0].snapshot.membership, Membership::Independent);
    }

    #[test]
    fn the_folded_byte_odometer_holds_across_a_member_departure() {
        let supervisor = InterfaceId::from_channel_tag(InterfaceKind::AutoWifi, b"default");
        let peer = InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, b"peer");
        let membership = Membership::FleetMember {
            supervisor_id: supervisor,
        };
        let mut with_live_member = [
            snapshot(
                supervisor,
                Membership::Independent,
                90,
                0,
                0,
                Some("Default Interface"),
            ),
            snapshot(peer, membership, 30, 0, 0, None),
        ];

        let logical = fold_logical_interface_inventory(&mut with_live_member);

        assert_eq!(logical.len(), 1);
        assert_eq!(logical[0].snapshot.rx_bytes, 120);
        assert_eq!(logical[0].snapshot.tx_bytes, 60);

        let mut after_departure = [snapshot(
            supervisor,
            Membership::Independent,
            120,
            0,
            0,
            Some("Default Interface"),
        )];

        let logical = fold_logical_interface_inventory(&mut after_departure);

        assert_eq!(logical.len(), 1);
        assert_eq!(logical[0].snapshot.rx_bytes, 120);
        assert_eq!(logical[0].snapshot.tx_bytes, 60);
    }

    #[test]
    fn discovered_origin_survives_logical_inventory_folding() {
        let id = InterfaceId::from_channel_tag(InterfaceKind::BackboneClient, b"discovered");
        let mut snapshots = [snapshot(
            id,
            Membership::Independent,
            100,
            2,
            1,
            Some("Discovered backbone"),
        )];
        snapshots[0].origin = InterfaceOriginKind::Discovered;
        let expected = snapshots[0].clone();

        let logical = fold_logical_interface_inventory(&mut snapshots);

        assert_eq!(logical, core::slice::from_ref(&expected));
    }
}
