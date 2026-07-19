use super::RoutingTable;
use crate::engine::InstantMillis;
use crate::interfaces::AttachedInterfaces;
use crate::routing::announce::defaults::route_expiry_millis;
use crate::routing::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordTable};
use crate::routing::route_expiry::RouteExpiryIndex;
use crate::routing::routes::RouteTable;
use crate::routing::types::{RemovedRoute, RouteRemovalCause, RouteResponsiveness};
use crate::routing::warmth::RouteWarmth;

impl<R, A, H, D, I> RoutingTable<R, A, H, D, I>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
    I: RouteExpiryIndex,
{
    /// Intentional deviation from the reference's learn-fixed `IDX_PT_EXPIRES` gate clock: once a link activation or a returned proof marks the route `Responsive`, the gate keeps our slid clock instead, refusing to trade a route that demonstrably works for one with longer hops.
    pub(super) fn gate_expiry_of(
        &self,
        i: usize,
        interfaces: AttachedInterfaces<'_>,
    ) -> InstantMillis {
        match self.routes.responsiveness()[i] {
            RouteResponsiveness::Responsive => self.expiry_of(i, interfaces),
            RouteResponsiveness::Unknown | RouteResponsiveness::Unresponsive => {
                self.expiry_from_anchor(self.routes.learned_at()[i], i, interfaces, &())
            }
        }
    }

    /// RNS folds learn and relay into one path-table TIMESTAMP. We keep them apart and recombine here, so an actively-carried route never ages out mid-flow while its announces lull.
    fn last_active_at(&self, i: usize) -> InstantMillis {
        InstantMillis(
            self.routes.learned_at()[i]
                .0
                .max(self.routes.last_relayed_at()[i].0),
        )
    }

    fn expiry_of(&self, i: usize, interfaces: AttachedInterfaces<'_>) -> InstantMillis {
        self.expiry_of_with_warmth(i, interfaces, &())
    }

    pub(super) fn expiry_of_with_warmth(
        &self,
        i: usize,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
    ) -> InstantMillis {
        self.expiry_from_anchor(self.last_active_at(i), i, interfaces, warmth)
    }

    fn expiry_from_anchor(
        &self,
        anchor: InstantMillis,
        i: usize,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
    ) -> InstantMillis {
        let receiving_interface = self.routes.receiving_interfaces()[i];
        match interfaces.descriptor_for(receiving_interface) {
            Some(descriptor) => InstantMillis(
                anchor
                    .0
                    .saturating_add(route_expiry_millis(descriptor.mode)),
            ),
            None => warmth.warm_until(receiving_interface).unwrap_or(anchor),
        }
    }

    fn remove_expired_route(
        &mut self,
        row: usize,
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) {
        let receiving_interface = self.routes.receiving_interfaces()[row];
        let cause = if interfaces.descriptor_for(receiving_interface).is_some() {
            RouteRemovalCause::Expired
        } else {
            RouteRemovalCause::InterfaceGone
        };
        on_removed(RemovedRoute {
            destination: self.routes.destinations()[row],
            receiving_interface,
            cause,
        });
        self.remove_route(row);
    }

    /// Boundary-inclusive: a deadline must be actionable at its own instant or a reactor waking exactly at `expires` busy-spins. The reference culls on a 5s float-time poll, so the boundary is unobservable to parity.
    pub fn cull_expired_routes(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        self.cull_expired_routes_with_warmth(now, interfaces, &(), on_removed)
    }

    pub fn cull_expired_routes_with_warmth(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        let mut culled = 0;
        let mut i = 0;
        while i < self.routes.len() {
            if now >= self.expiry_of_with_warmth(i, interfaces, warmth) {
                self.remove_expired_route(i, interfaces, on_removed);
                culled += 1;
            } else {
                i += 1;
            }
        }
        culled
    }

    pub(crate) fn cull_expired_routes_indexed_with_warmth(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        if !I::INDEXED {
            return self.cull_expired_routes_with_warmth(now, interfaces, warmth, on_removed);
        }
        if self
            .route_expiries
            .prefers_linear_cull(self.routes.len(), now)
        {
            self.route_expiries.invalidate();
            return self.cull_expired_routes_with_warmth(now, interfaces, warmth, on_removed);
        }
        let mut culled = 0;
        while let Some(i) = self
            .route_expiries
            .first_expired(self.routes.len(), now, |row| {
                self.expiry_of_with_warmth(row, interfaces, warmth)
            })
        {
            self.remove_expired_route(i, interfaces, on_removed);
            culled += 1;
        }
        culled
    }

    pub fn soonest_route_expiry(
        &self,
        interfaces: AttachedInterfaces<'_>,
    ) -> Option<InstantMillis> {
        self.soonest_route_expiry_with_warmth(interfaces, &())
    }

    pub fn soonest_route_expiry_with_warmth(
        &self,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
    ) -> Option<InstantMillis> {
        (0..self.routes.len())
            .map(|i| self.expiry_of_with_warmth(i, interfaces, warmth))
            .min()
    }

    pub(crate) fn soonest_route_expiry_indexed_with_warmth(
        &self,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
    ) -> Option<InstantMillis> {
        self.route_expiries
            .earliest_exact(self.routes.len(), |row| {
                self.expiry_of_with_warmth(row, interfaces, warmth)
            })
    }

    pub(crate) fn invalidate_route_expiries(&self) {
        self.route_expiries.invalidate();
    }
}
