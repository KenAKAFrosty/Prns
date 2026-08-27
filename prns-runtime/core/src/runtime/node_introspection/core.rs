use prns_core::interfaces::IfacSize;
use prns_core::interfaces::{
    BitrateBps, ConnectionState, FrameAccounting, InterfaceCapabilities, InterfaceGravity,
    InterfaceId, InterfaceMode, InterfaceOriginKind, InterfaceSnapshot, Membership, TransferRates,
};


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceTimingSnapshot {
    pub id: InterfaceId,
    pub bitrate: BitrateBps,
    pub capabilities: InterfaceCapabilities,
    pub connection: ConnectionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameAccountingCoverage {
    Unavailable,
    Incomplete,
    Complete(FrameAccounting),
}

impl FrameAccountingCoverage {
    #[must_use]
    pub const fn complete(self) -> Option<FrameAccounting> {
        match self {
            Self::Complete(accounting) => Some(accounting),
            Self::Unavailable | Self::Incomplete => None,
        }
    }
}

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

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
    pub attachment_epoch: u64,
    pub frame_accounting: FrameAccountingCoverage,
    pub snapshot: InterfaceSnapshot,
    pub ifac: Option<InterfaceIfacSnapshot<Label>>,
    /// Link-up RSSI in dBm when the interface recorded one (Bluetooth Auto peers).
    pub rssi: Option<i8>,
    /// Fleet members nested under a folded supervisor. Always empty on leaf entries.
    #[cfg(feature = "alloc")]
    pub members: Vec<InterfaceInventoryEntry<Label>>,
}

struct FoldedInterface<Label> {
    id: InterfaceId,
    name: Option<Label>,
    origin: InterfaceOriginKind,
    root: Option<InterfaceSnapshot>,
    root_attachment_epoch: Option<u64>,
    root_frame_accounting: FrameAccountingCoverage,
    ifac: Option<InterfaceIfacSnapshot<Label>>,
    rssi: Option<i8>,
    #[cfg(feature = "alloc")]
    members: Vec<InterfaceInventoryEntry<Label>>,
    member_connection: ConnectionState,
    member_mode: Option<InterfaceMode>,
    member_gravity: Option<InterfaceGravity>,
    member_failure_reason: Option<&'static str>,
    member_rx_bytes: u64,
    member_tx_bytes: u64,
    member_rates: Option<TransferRates>,
    member_attachment_epoch: u64,
    member_frame_accounting: FrameAccounting,
    member_frame_accounting_complete: bool,
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
            root_attachment_epoch: None,
            root_frame_accounting: FrameAccountingCoverage::Unavailable,
            ifac: None,
            rssi: None,
            #[cfg(feature = "alloc")]
            members: Vec::new(),
            member_connection: ConnectionState::Unknown,
            member_mode: None,
            member_gravity: None,
            member_failure_reason: None,
            member_rx_bytes: 0,
            member_tx_bytes: 0,
            member_rates: None,
            member_attachment_epoch: 0,
            member_frame_accounting: FrameAccounting::default(),
            member_frame_accounting_complete: true,
            has_members: false,
            destinations: 0,
            links: 0,
            transported_links: 0,
        }
    }

    fn add(&mut self, entry: &mut InterfaceInventoryEntry<Label>)
    where
        Label: Clone,
    {
        let snapshot = entry.snapshot;
        self.destinations = self.destinations.saturating_add(snapshot.destinations);
        self.links = self.links.saturating_add(snapshot.links);
        self.transported_links = self
            .transported_links
            .saturating_add(snapshot.transported_links);
        match snapshot.membership {
            Membership::Independent => {
                self.root = Some(snapshot);
                self.root_attachment_epoch = Some(entry.attachment_epoch);
                self.root_frame_accounting = entry.frame_accounting;
                self.origin = entry.origin;
                if entry.name.is_some() {
                    self.name = entry.name.take();
                }
                if entry.ifac.is_some() {
                    self.ifac = entry.ifac.take();
                }
                self.rssi = entry.rssi.or(self.rssi);
            }
            Membership::FleetMember { .. } => {
                if self.root.is_none() && entry.origin == InterfaceOriginKind::Discovered {
                    self.origin = InterfaceOriginKind::Discovered;
                }
                self.has_members = true;
                self.member_attachment_epoch =
                    self.member_attachment_epoch.max(entry.attachment_epoch);
                match entry.frame_accounting {
                    FrameAccountingCoverage::Complete(accounting) => {
                        self.member_frame_accounting =
                            self.member_frame_accounting.saturating_add(accounting);
                    }
                    FrameAccountingCoverage::Unavailable | FrameAccountingCoverage::Incomplete => {
                        self.member_frame_accounting_complete = false;
                    }
                }
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
                #[cfg(feature = "alloc")]
                {
                    if self.ifac.is_none() {
                        self.ifac = entry.ifac.clone();
                    }
                    self.members.push(InterfaceInventoryEntry {
                        name: entry.name.take(),
                        origin: entry.origin,
                        attachment_epoch: entry.attachment_epoch,
                        frame_accounting: entry.frame_accounting,
                        snapshot,
                        ifac: entry.ifac.take(),
                        rssi: entry.rssi,
                        members: Vec::new(),
                    });
                }
                #[cfg(not(feature = "alloc"))]
                {
                    if self.name.is_none() {
                        self.name = entry.name.take();
                    }
                    if self.ifac.is_none() {
                        self.ifac = entry.ifac.take();
                    }
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
        let frame_accounting = if self.has_members {
            match (
                self.root_frame_accounting,
                self.member_frame_accounting_complete,
            ) {
                (FrameAccountingCoverage::Incomplete, _) | (_, false) => {
                    FrameAccountingCoverage::Incomplete
                }
                (FrameAccountingCoverage::Complete(retired), true) => {
                    FrameAccountingCoverage::Complete(
                        retired.saturating_add(self.member_frame_accounting),
                    )
                }
                (FrameAccountingCoverage::Unavailable, true) => {
                    FrameAccountingCoverage::Complete(self.member_frame_accounting)
                }
            }
        } else {
            self.root_frame_accounting
        };
        InterfaceInventoryEntry {
            name: self.name,
            origin: self.origin,
            attachment_epoch: self
                .root_attachment_epoch
                .unwrap_or(self.member_attachment_epoch),
            frame_accounting,
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
            rssi: self.rssi,
            #[cfg(feature = "alloc")]
            members: self.members,
        }
    }
}

#[must_use]
pub fn fold_logical_interface_inventory<Label: Clone + Ord>(
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
            attachment_epoch: 1,
            frame_accounting: FrameAccountingCoverage::Unavailable,
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
            rssi: None,
            #[cfg(feature = "alloc")]
            members: Vec::new(),
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
            snapshot(second, membership, 60, 3, 2, Some("second-peer")),
            snapshot(
                supervisor,
                Membership::Independent,
                10,
                0,
                0,
                Some("Public server"),
            ),
            {
                let mut peer = snapshot(first, membership, 40, 2, 1, Some("first-peer"));
                peer.rssi = Some(-61);
                peer
            },
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
        #[cfg(feature = "alloc")]
        {
            assert_eq!(logical[0].members.len(), 2);
            let member_names: alloc::vec::Vec<_> =
                logical[0].members.iter().map(|peer| peer.name).collect();
            assert!(member_names.contains(&Some("first-peer")));
            assert!(member_names.contains(&Some("second-peer")));
            let first_peer = logical[0]
                .members
                .iter()
                .find(|peer| peer.snapshot.id == first)
                .expect("first peer retained");
            assert_eq!(first_peer.snapshot.rx_bytes, 40);
            assert_eq!(first_peer.rssi, Some(-61));
            let second_peer = logical[0]
                .members
                .iter()
                .find(|peer| peer.snapshot.id == second)
                .expect("second peer retained");
            assert_eq!(second_peer.snapshot.rx_bytes, 60);
        }
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

    #[test]
    fn fleet_frame_accounting_requires_complete_member_coverage() {
        let supervisor = InterfaceId::from_channel_tag(InterfaceKind::TcpServer, b"server");
        let first = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"first");
        let second = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"second");
        let membership = Membership::FleetMember {
            supervisor_id: supervisor,
        };
        let mut root = snapshot(
            supervisor,
            Membership::Independent,
            0,
            0,
            0,
            Some("Public server"),
        );
        root.attachment_epoch = 7;
        root.frame_accounting = FrameAccountingCoverage::Complete(FrameAccounting {
            frames_in: 5,
            malformed: 1,
            protocol_violations: 1,
            undecodable: 0,
            delivered: 4,
        });
        let mut first_member = snapshot(first, membership, 0, 0, 0, None);
        first_member.frame_accounting = FrameAccountingCoverage::Complete(FrameAccounting {
            frames_in: 3,
            malformed: 0,
            protocol_violations: 1,
            undecodable: 1,
            delivered: 2,
        });
        let mut second_member = snapshot(second, membership, 0, 0, 0, None);
        second_member.frame_accounting = FrameAccountingCoverage::Unavailable;
        let mut partial = [root.clone(), first_member.clone(), second_member];

        let logical = fold_logical_interface_inventory(&mut partial);

        assert_eq!(logical[0].attachment_epoch, 7);
        assert_eq!(
            logical[0].frame_accounting,
            FrameAccountingCoverage::Incomplete
        );

        let mut complete = [root, first_member];
        let logical = fold_logical_interface_inventory(&mut complete);
        assert_eq!(
            logical[0].frame_accounting,
            FrameAccountingCoverage::Complete(FrameAccounting {
                frames_in: 8,
                malformed: 1,
                protocol_violations: 2,
                undecodable: 1,
                delivered: 6,
            })
        );
    }
}
