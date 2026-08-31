use super::RoutingTable;
use crate::engine::InstantMillis;
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::routing::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordTable};
use crate::routing::route_expiry::RouteExpiryIndex;
use crate::routing::routes::{RouteEntry, RouteEvidenceHandle, RouteResponsiveness, RouteTable};
use crate::routing::warmth::RouteWarmth;
use crate::wire::DestinationHash;

impl<R, A, H, D, I> RoutingTable<R, A, H, D, I>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
    I: RouteExpiryIndex,
{
    /// Applies authenticated traffic to the exact route incarnation that carried it.
    ///
    /// The route activity clock is separate from announce age: relayed traffic and authenticated
    /// return traffic both advance it without making the route appear newly announced.
    pub(crate) fn apply_route_evidence(
        &mut self,
        handle: &mut RouteEvidenceHandle,
        observed_at: InstantMillis,
    ) -> bool {
        let Some(i) = self.resolve_route_evidence(handle) else {
            return false;
        };
        let last_route_activity = self.routes.last_route_activity_at()[i].max(observed_at);
        let changed = last_route_activity != self.routes.last_route_activity_at()[i]
            || self.routes.responsiveness()[i] != RouteResponsiveness::Responsive;
        if !changed {
            return false;
        }
        self.routes.set_row(
            i,
            RouteEntry {
                hops: self.routes.hops()[i],
                learned_at: self.routes.learned_at()[i],
                last_route_activity_at: last_route_activity,
                responsiveness: RouteResponsiveness::Responsive,
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
                retention: self.routes.retentions()[i],
            },
        );
        self.route_expiries.invalidate();
        true
    }

    /// Marks only the route incarnation used by a failed attempt, unless newer route activity has
    /// already disproved that negative observation.
    pub(crate) fn mark_unresponsive_if_not_active_since(
        &mut self,
        handle: &mut RouteEvidenceHandle,
        attempt_started_at: InstantMillis,
    ) -> bool {
        let Some(i) = self.resolve_route_evidence(handle) else {
            return false;
        };
        let last_active = self.routes.learned_at()[i].max(self.routes.last_route_activity_at()[i]);
        if last_active > attempt_started_at
            || self.routes.responsiveness()[i] == RouteResponsiveness::Unresponsive
        {
            return false;
        }
        self.routes.set_row(
            i,
            RouteEntry {
                hops: self.routes.hops()[i],
                learned_at: self.routes.learned_at()[i],
                last_route_activity_at: self.routes.last_route_activity_at()[i],
                responsiveness: RouteResponsiveness::Unresponsive,
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
                retention: self.routes.retentions()[i],
            },
        );
        true
    }

    pub fn mark_responsiveness(
        &mut self,
        destination: &DestinationHash,
        responsiveness: RouteResponsiveness,
    ) {
        let Some(i) = self.index_of(destination) else {
            return;
        };
        self.routes.set_row(
            i,
            RouteEntry {
                hops: self.routes.hops()[i],
                learned_at: self.routes.learned_at()[i],
                last_route_activity_at: self.routes.last_route_activity_at()[i],
                responsiveness,
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
                retention: self.routes.retentions()[i],
            },
        );
    }

    pub(crate) fn rebalance_hops(&mut self, destination: &DestinationHash, hops: u8) {
        let Some(i) = self.index_of(destination) else {
            return;
        };
        self.routes.set_row(
            i,
            RouteEntry {
                hops,
                learned_at: self.routes.learned_at()[i],
                last_route_activity_at: self.routes.last_route_activity_at()[i],
                responsiveness: self.routes.responsiveness()[i],
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
                retention: self.routes.retentions()[i],
            },
        );
    }

    pub fn note_relayed(&mut self, destination: &DestinationHash, now: InstantMillis) {
        let Some(i) = self.index_of(destination) else {
            return;
        };
        self.routes.set_row(
            i,
            RouteEntry {
                hops: self.routes.hops()[i],
                learned_at: self.routes.learned_at()[i],
                last_route_activity_at: now,
                responsiveness: self.routes.responsiveness()[i],
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
                retention: self.routes.retentions()[i],
            },
        );
        self.route_expiries.invalidate();
    }

    pub(crate) fn note_relayed_with_warmth(
        &mut self,
        destination: &DestinationHash,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
    ) {
        let Some(i) = self.index_of(destination) else {
            return;
        };
        self.routes.set_row(
            i,
            RouteEntry {
                hops: self.routes.hops()[i],
                learned_at: self.routes.learned_at()[i],
                last_route_activity_at: now,
                responsiveness: self.routes.responsiveness()[i],
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
                retention: self.routes.retentions()[i],
            },
        );
        let expiry = self.expiry_of_with_warmth(i, interfaces, warmth);
        self.route_expiries.update(i, expiry);
    }

    pub fn repoint_routes(
        &mut self,
        previous: InterfaceId,
        current: InterfaceId,
        now: InstantMillis,
    ) -> usize {
        let moved = self
            .routes
            .repoint_receiving_interface(previous, current, now);
        if moved != 0 {
            self.route_expiries.invalidate();
        }
        moved
    }
}
