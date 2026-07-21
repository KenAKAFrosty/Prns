use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::lemire_index::buckets_for_two_thirds_load;
use crate::storage::TablePushError;
use crate::wire::{DestinationHash, TransportId};

pub const fn route_index_buckets(destinations: usize) -> usize {
    buckets_for_two_thirds_load(destinations)
}

/// RNS 1.4.0 `Transport.path_table`'s `received_from` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextHop {
    Direct,
    Via(TransportId),
}

/// RNS 1.4.0 `Transport.path_is_unresponsive`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteResponsiveness {
    Unknown,
    Responsive,
    Unresponsive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteEntry {
    pub hops: u8,
    pub learned_at: InstantMillis,
    pub last_relayed_at: InstantMillis,
    pub responsiveness: RouteResponsiveness,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
}

pub trait RouteTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.destinations()
            .iter()
            .position(|candidate| candidate == destination)
    }

    fn route_count_via(&self, interface: InterfaceId) -> usize {
        self.receiving_interfaces()
            .iter()
            .filter(|&&candidate| candidate == interface)
            .count()
    }

    fn repoint_receiving_interface(
        &mut self,
        previous: InterfaceId,
        current: InterfaceId,
        now: InstantMillis,
    ) -> usize {
        let mut moved = 0;
        for row in 0..self.len() {
            if self.receiving_interfaces()[row] != previous {
                continue;
            }
            self.set_row(
                row,
                RouteEntry {
                    hops: self.hops()[row],
                    learned_at: self.learned_at()[row],
                    last_relayed_at: now,
                    responsiveness: self.responsiveness()[row],
                    receiving_interface: current,
                    next_hop: self.next_hops()[row],
                },
            );
            moved += 1;
        }
        moved
    }

    fn destinations(&self) -> &[DestinationHash];
    fn hops(&self) -> &[u8];
    fn learned_at(&self) -> &[InstantMillis];
    fn last_relayed_at(&self) -> &[InstantMillis];
    fn responsiveness(&self) -> &[RouteResponsiveness];
    fn receiving_interfaces(&self) -> &[InterfaceId];
    fn next_hops(&self) -> &[NextHop];

    fn set_row(&mut self, i: usize, row: RouteEntry);

    fn push(
        &mut self,
        destination: DestinationHash,
        row: RouteEntry,
    ) -> Result<usize, TablePushError>;

    fn swap_remove(&mut self, i: usize, last: usize);
}
