use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::{NextHop, RouteResponsiveness};
use crate::storage::ColumnsFull;
use crate::wire::DestinationHash;

mod impls;

pub use impls::FixedArrayRouteColumns;
pub use impls::FixedIndexedRouteColumns;
#[cfg(feature = "external-alloc")]
pub use impls::FixedHeapRouteColumns;
#[cfg(feature = "alloc")]
pub use impls::HeapRouteColumns;

pub const fn route_index_buckets(destinations: usize) -> usize {
    if destinations == 0 {
        return 1;
    }
    (destinations * 3).div_ceil(2)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteEntry {
    pub hops: u8,
    pub learned_at: InstantMillis,
    pub responsiveness: RouteResponsiveness,
    pub receiving_interface: InterfaceId,
    pub next_hop: NextHop,
}

pub trait RouteColumns {
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
    fn responsiveness(&self) -> &[RouteResponsiveness];
    fn receiving_interfaces(&self) -> &[InterfaceId];
    fn next_hops(&self) -> &[NextHop];

    fn set_row(&mut self, i: usize, row: RouteEntry);

    fn push(&mut self, destination: DestinationHash, row: RouteEntry)
        -> Result<usize, ColumnsFull>;

    fn swap_remove(&mut self, i: usize);
}
