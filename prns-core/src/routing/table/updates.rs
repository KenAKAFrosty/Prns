use super::RoutingTable;
use crate::engine::InstantMillis;
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::routing::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordTable};
use crate::routing::route_expiry::RouteExpiryIndex;
use crate::routing::routes::{RouteEntry, RouteTable};
use crate::routing::types::RouteResponsiveness;
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
                last_relayed_at: self.routes.last_relayed_at()[i],
                responsiveness,
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
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
                last_relayed_at: now,
                responsiveness: self.routes.responsiveness()[i],
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
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
                last_relayed_at: now,
                responsiveness: self.routes.responsiveness()[i],
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
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
        let mut moved = 0;
        for i in 0..self.routes.len() {
            if self.routes.receiving_interfaces()[i] != previous {
                continue;
            }
            self.routes.set_row(
                i,
                RouteEntry {
                    hops: self.routes.hops()[i],
                    learned_at: self.routes.learned_at()[i],
                    last_relayed_at: now,
                    responsiveness: self.routes.responsiveness()[i],
                    receiving_interface: current,
                    next_hop: self.routes.next_hops()[i],
                },
            );
            moved += 1;
        }
        if moved != 0 {
            self.route_expiries.invalidate();
        }
        moved
    }
}
