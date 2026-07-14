use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::lemire_index::buckets_for_two_thirds_load;
use crate::routing::{NextHop, RouteResponsiveness};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

pub const fn route_index_buckets(destinations: usize) -> usize {
    buckets_for_two_thirds_load(destinations)
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
