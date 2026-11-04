use crate::engine::{
    settle, EngineReaction, EngineState, EstablishLinkFailure, EstablishLinkRejection,
    InstantMillis, LinkClosedReason, Settlement, WakeSchedules,
};
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::routing::announce::schedule::ScheduledAnnounceQueue;
use crate::routing::links::table::LinkPhase;
use crate::routing::warmth::Departure;
use crate::storage::{DirtyInterfaceSet, StorageLayout};

impl<S: StorageLayout> EngineState<S> {
    /// Retires the volatile network state of an administratively disabled interface.
    ///
    /// A supervisor also retires stored child lanes absent from the current roster.
    /// Unlike a temporary departure, this bypasses tunnel and departure warmth. Call once for
    /// each observed disable generation, even when the lane has already been enabled again.
    /// The caller fences queued/deferred I/O independently and supplies the current live view.
    /// No close frame is sent over the disabled interface, and durable application intent is
    /// not owned or removed by this operation.
    pub fn interface_unavailable(
        &mut self,
        interface: InterfaceId,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        self.interface_departed(interface, Departure::Forgotten, now);
        self.recursive_path_requests
            .retire_administrative_scope(interface);
        self.departed_interfaces
            .forget_administrative_scope(interface);
        loop {
            let next = self
                .held_announces
                .interfaces()
                .find(|id| interface.administrative_scope_contains(*id));
            let Some(id) = next else {
                break;
            };
            self.held_announces.drop_interface(id);
        }
        self.scheduled_announces.cancel_interface(interface);
        let scheduled = &mut self.scheduled_announces;
        let dirty = &mut self.dirty_interfaces;
        self.routing_table
            .drop_routes_for_interface(interface, &mut |removed| {
                let _ = scheduled.cancel(&removed.destination);
                dirty.mark(removed.receiving_interface);
                sink(EngineReaction::Journaled(
                    crate::engine::node_ingress::journal_route_removal(removed),
                ));
            });
        self.reverse_routes
            .cull_interface_orphans(|id| !interface.administrative_scope_contains(id));
        let dirty = &mut self.dirty_interfaces;
        self.transported_links.cull_interface_orphans(
            |id| !interface.administrative_scope_contains(id),
            |id| dirty.mark(id),
        );
        while let Some(link_id) = self.links.first_link_via(interface) {
            let pending_command = match self.links.phase_for(&link_id) {
                Some(LinkPhase::Pending { command_id, .. }) => Some(*command_id),
                _ => None,
            };
            self.retire_link(&link_id, LinkClosedReason::LocallyClosed, sink);
            if let Some(command_id) = pending_command {
                settle(
                    sink,
                    command_id,
                    Settlement::EstablishLink(Err(EstablishLinkFailure::Rejected(
                        EstablishLinkRejection::NoRouteToDestination,
                    ))),
                );
            }
        }
        // Restored path state must be rewritten even when the corresponding volatile table
        // was already culled. No later On transition may revive its persisted incarnation.
        self.dirty_interfaces.mark(interface);
        self.wake_schedules(interfaces)
    }
}

#[cfg(test)]
mod tests;
