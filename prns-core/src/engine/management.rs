use crate::routing::RemovedRoute;
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::wire::{DestinationHash, TransportId};

use super::EngineState;

impl<S: StorageLayout> EngineState<S> {
    pub fn drop_route(&mut self, destination: &DestinationHash) -> Option<RemovedRoute> {
        let removed = self.routing_table.drop_route(destination)?;
        self.dirty_interfaces.mark(removed.receiving_interface);
        Some(removed)
    }

    pub fn drop_routes_via(
        &mut self,
        transport: TransportId,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        let dirty = &mut self.dirty_interfaces;
        self.routing_table
            .drop_routes_via(transport, &mut |removed| {
                dirty.mark(removed.receiving_interface);
                on_removed(removed);
            })
    }
}
