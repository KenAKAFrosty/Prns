use crate::engine::EngineState;
#[cfg(feature = "alloc")]
use crate::interfaces::InterfaceId;
#[cfg(feature = "alloc")]
use crate::routing::types::NextHop;
use crate::storage::StorageLayout;
#[cfg(feature = "alloc")]
use crate::units::InstantMillis;
#[cfg(feature = "alloc")]
use crate::wire::DestinationHash;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectionQuery {
    LinkCount,
    #[cfg(feature = "alloc")]
    Routes,
    #[cfg(feature = "alloc")]
    Route(DestinationHash),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectionResult {
    LinkCount(u32),
    #[cfg(feature = "alloc")]
    Routes(Vec<RouteSnapshot>),
    #[cfg(feature = "alloc")]
    Route(Option<RouteSnapshot>),
}

#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSnapshot {
    pub destination: DestinationHash,
    pub hops: u8,
    pub via: NextHop,
    pub learned_at: InstantMillis,
    pub interface: InterfaceId,
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn run_inspection_query(&self, query: InspectionQuery) -> InspectionResult {
        match query {
            InspectionQuery::LinkCount => InspectionResult::LinkCount(self.links.len() as u32),
            #[cfg(feature = "alloc")]
            InspectionQuery::Routes => InspectionResult::Routes(
                self.routing_table
                    .path_rows()
                    .map(|(destination, entry)| RouteSnapshot {
                        destination,
                        hops: entry.hops,
                        via: entry.next_hop,
                        learned_at: entry.learned_at,
                        interface: entry.receiving_interface,
                    })
                    .collect(),
            ),
            #[cfg(feature = "alloc")]
            InspectionQuery::Route(destination) => {
                InspectionResult::Route(self.routing_table.path_row(&destination).map(|entry| {
                    RouteSnapshot {
                        destination,
                        hops: entry.hops,
                        via: entry.next_hop,
                        learned_at: entry.learned_at,
                        interface: entry.receiving_interface,
                    }
                }))
            }
        }
    }
}
