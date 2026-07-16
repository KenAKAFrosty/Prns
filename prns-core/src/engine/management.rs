use crate::routing::RemovedRoute;
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, TransportId};

use super::EngineState;

impl<S: StorageLayout> EngineState<S> {
    pub fn drop_route(&mut self, destination: &DestinationHash) -> Option<RemovedRoute> {
        self.routing_table.drop_route(destination)
    }

    pub fn drop_routes_via(
        &mut self,
        transport: TransportId,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        self.routing_table.drop_routes_via(transport, on_removed)
    }
}
