use ::core::net::Ipv6Addr;

use prns_core::interfaces::{InterfaceId, InterfaceKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SegmentRole {
    Primary,
    Secondary,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct WifiPeer {
    address: Ipv6Addr,
    id: InterfaceId,
    segment: SegmentRole,
}

impl WifiPeer {
    pub(super) fn new(address: Ipv6Addr, segment: SegmentRole) -> Self {
        Self {
            address,
            id: InterfaceId::from_channel_tag(InterfaceKind::WifiPeer, &address.octets()),
            segment,
        }
    }

    pub(super) fn address(&self) -> Ipv6Addr {
        self.address
    }

    pub(super) fn id(&self) -> InterfaceId {
        self.id
    }

    pub(super) fn segment(&self) -> SegmentRole {
        self.segment
    }
}

enum WifiPeerSlot {
    Vacant,
    Occupied(WifiPeer),
    Reserved,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WifiPeerLookup {
    Present { slot: usize, id: InterfaceId },
    Vacant { slot: usize },
    Full,
}

pub(super) enum WifiPeerSlotLookup<'a> {
    Occupied(&'a WifiPeer),
    Empty,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WifiPeerInsertion {
    Inserted { slot: usize },
    Occupied { slot: usize },
    Reserved { slot: usize },
    OutOfBounds,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WifiPeerReplacement {
    Replaced { slot: usize, previous: SegmentRole },
    Unchanged { slot: usize },
    Missing,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WifiPeerRetirement {
    Retired { slot: usize, peer: WifiPeer },
    Missing,
}

pub(super) enum WifiPeerClear {
    AlreadyEmpty,
    Cleared,
}

pub(super) struct WifiPeerTable<const MEMBERS: usize> {
    slots: [WifiPeerSlot; MEMBERS],
}

impl<const MEMBERS: usize> WifiPeerTable<MEMBERS> {
    pub(super) fn new() -> Self {
        Self {
            slots: ::core::array::from_fn(|_| WifiPeerSlot::Vacant),
        }
    }

    pub(super) fn reserving_last_slots(count: usize) -> Self {
        let mut table = Self::new();
        let first_reserved = MEMBERS.saturating_sub(count.min(MEMBERS));
        for slot in &mut table.slots[first_reserved..] {
            *slot = WifiPeerSlot::Reserved;
        }
        table
    }

    pub(super) fn reserved_slot(&self, index: usize) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, state)| matches!(state, WifiPeerSlot::Reserved).then_some(slot))
            .nth(index)
    }

    pub(super) fn lookup(&self, address: Ipv6Addr) -> WifiPeerLookup {
        let mut vacant_slot = None;
        for (slot, state) in self.slots.iter().enumerate() {
            match state {
                WifiPeerSlot::Occupied(peer) if peer.address() == address => {
                    return WifiPeerLookup::Present {
                        slot,
                        id: peer.id(),
                    };
                }
                WifiPeerSlot::Vacant if vacant_slot.is_none() => vacant_slot = Some(slot),
                WifiPeerSlot::Vacant | WifiPeerSlot::Occupied(_) | WifiPeerSlot::Reserved => {}
            }
        }
        match vacant_slot {
            Some(slot) => WifiPeerLookup::Vacant { slot },
            None => WifiPeerLookup::Full,
        }
    }

    pub(super) fn lookup_slot(&self, slot: usize) -> WifiPeerSlotLookup<'_> {
        match self.slots.get(slot) {
            Some(WifiPeerSlot::Occupied(peer)) => WifiPeerSlotLookup::Occupied(peer),
            Some(WifiPeerSlot::Vacant | WifiPeerSlot::Reserved) | None => WifiPeerSlotLookup::Empty,
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (usize, &WifiPeer)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, state)| match state {
                WifiPeerSlot::Occupied(peer) => Some((slot, peer)),
                WifiPeerSlot::Vacant | WifiPeerSlot::Reserved => None,
            })
    }

    pub(super) fn insert(&mut self, slot: usize, peer: WifiPeer) -> WifiPeerInsertion {
        let Some(state) = self.slots.get_mut(slot) else {
            return WifiPeerInsertion::OutOfBounds;
        };
        match state {
            WifiPeerSlot::Vacant => {
                *state = WifiPeerSlot::Occupied(peer);
                WifiPeerInsertion::Inserted { slot }
            }
            WifiPeerSlot::Occupied(_) => WifiPeerInsertion::Occupied { slot },
            WifiPeerSlot::Reserved => WifiPeerInsertion::Reserved { slot },
        }
    }

    pub(super) fn replace_segment(
        &mut self,
        slot: usize,
        segment: SegmentRole,
    ) -> WifiPeerReplacement {
        let Some(WifiPeerSlot::Occupied(peer)) = self.slots.get_mut(slot) else {
            return WifiPeerReplacement::Missing;
        };
        if peer.segment == segment {
            return WifiPeerReplacement::Unchanged { slot };
        }
        let previous = peer.segment;
        peer.segment = segment;
        WifiPeerReplacement::Replaced { slot, previous }
    }

    pub(super) fn retire(&mut self, slot: usize) -> WifiPeerRetirement {
        let Some(state) = self.slots.get_mut(slot) else {
            return WifiPeerRetirement::Missing;
        };
        match ::core::mem::replace(state, WifiPeerSlot::Vacant) {
            WifiPeerSlot::Occupied(peer) => WifiPeerRetirement::Retired { slot, peer },
            WifiPeerSlot::Vacant => WifiPeerRetirement::Missing,
            WifiPeerSlot::Reserved => {
                *state = WifiPeerSlot::Reserved;
                WifiPeerRetirement::Missing
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(suffix: u16) -> Ipv6Addr {
        Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, suffix)
    }

    #[test]
    fn peer_table_operations_preserve_whole_values_and_reservations() {
        let mut peers = WifiPeerTable::<3>::reserving_last_slots(1);
        let first = WifiPeer::new(address(1), SegmentRole::Primary);
        let first_id = first.id();

        assert_eq!(peers.reserved_slot(0), Some(2));
        assert_eq!(peers.lookup(address(1)), WifiPeerLookup::Vacant { slot: 0 });
        assert_eq!(
            peers.insert(0, first),
            WifiPeerInsertion::Inserted { slot: 0 }
        );
        assert_eq!(
            peers.lookup(address(1)),
            WifiPeerLookup::Present {
                slot: 0,
                id: first_id,
            }
        );
        assert_eq!(
            peers.replace_segment(0, SegmentRole::Secondary),
            WifiPeerReplacement::Replaced {
                slot: 0,
                previous: SegmentRole::Primary,
            }
        );
        assert_eq!(
            peers.insert(0, WifiPeer::new(address(2), SegmentRole::Primary)),
            WifiPeerInsertion::Occupied { slot: 0 }
        );
        assert_eq!(
            peers.insert(2, WifiPeer::new(address(2), SegmentRole::Primary)),
            WifiPeerInsertion::Reserved { slot: 2 }
        );
        assert_eq!(
            peers.retire(0),
            WifiPeerRetirement::Retired {
                slot: 0,
                peer: WifiPeer::new(address(1), SegmentRole::Secondary),
            }
        );
        assert!(matches!(peers.lookup_slot(0), WifiPeerSlotLookup::Empty));
    }

    #[test]
    fn zero_capacity_table_has_no_reservable_or_available_slot() {
        let peers = WifiPeerTable::<0>::reserving_last_slots(1);

        assert_eq!(peers.reserved_slot(0), None);
        assert_eq!(peers.lookup(address(1)), WifiPeerLookup::Full);
    }

    #[test]
    fn peer_table_reserves_a_tail_range_for_rendezvous_clients() {
        let peers = WifiPeerTable::<6>::reserving_last_slots(4);

        assert_eq!(peers.reserved_slot(0), Some(2));
        assert_eq!(peers.reserved_slot(1), Some(3));
        assert_eq!(peers.reserved_slot(2), Some(4));
        assert_eq!(peers.reserved_slot(3), Some(5));
        assert_eq!(peers.reserved_slot(4), None);
        assert_eq!(peers.lookup(address(9)), WifiPeerLookup::Vacant { slot: 0 });
    }
}
