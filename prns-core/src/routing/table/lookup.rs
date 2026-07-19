use super::RoutingTable;
use crate::engine::InstantMillis;
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::routing::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordTable};
use crate::routing::route_expiry::RouteExpiryIndex;
use crate::routing::routes::{RouteEntry, RouteTable};
use crate::routing::types::{ExistingRoute, ForwardingRoute, RouteResponsiveness};
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
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    pub fn route_count_via(&self, interface: InterfaceId) -> usize {
        self.routes.route_count_via(interface)
    }

    pub fn hop_count_to(&self, destination: &DestinationHash) -> Option<u8> {
        self.index_of(destination).map(|i| self.routes.hops()[i])
    }

    pub fn has_route(&self, destination: &DestinationHash) -> bool {
        self.index_of(destination).is_some()
    }

    pub fn responsiveness_of(&self, destination: &DestinationHash) -> Option<RouteResponsiveness> {
        self.index_of(destination)
            .map(|i| self.routes.responsiveness()[i])
    }

    pub fn path_rows(&self) -> impl Iterator<Item = (DestinationHash, RouteEntry)> + '_ {
        let routes = &self.routes;
        (0..routes.len()).map(move |i| (routes.destinations()[i], self.path_row_at(i)))
    }

    pub(crate) fn path_rows_with_expiry<'a>(
        &'a self,
        interfaces: AttachedInterfaces<'a>,
        warmth: &'a dyn RouteWarmth,
    ) -> impl Iterator<Item = (DestinationHash, RouteEntry, InstantMillis)> + 'a {
        let routes = &self.routes;
        (0..routes.len()).map(move |i| {
            (
                routes.destinations()[i],
                self.path_row_at(i),
                self.expiry_of_with_warmth(i, interfaces, warmth),
            )
        })
    }

    pub fn path_row(&self, destination: &DestinationHash) -> Option<RouteEntry> {
        let i = self.index_of(destination)?;
        Some(self.path_row_at(i))
    }

    pub(crate) fn path_row_with_expiry(
        &self,
        destination: &DestinationHash,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
    ) -> Option<(RouteEntry, InstantMillis)> {
        let i = self.index_of(destination)?;
        Some((
            self.path_row_at(i),
            self.expiry_of_with_warmth(i, interfaces, warmth),
        ))
    }

    pub(super) fn path_row_at(&self, i: usize) -> RouteEntry {
        RouteEntry {
            hops: self.routes.hops()[i],
            learned_at: self.routes.learned_at()[i],
            responsiveness: self.routes.responsiveness()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
            last_relayed_at: self.routes.last_relayed_at()[i],
        }
    }

    pub(super) fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.routes.index_of(destination)
    }

    pub fn existing_route_for(
        &self,
        destination: &DestinationHash,
        interfaces: AttachedInterfaces<'_>,
    ) -> Option<ExistingRoute<'_>> {
        let i = self.index_of(destination)?;
        Some(ExistingRoute {
            hops: crate::units::HopCount(self.routes.hops()[i]),
            expires_at: self.gate_expiry_of(i, interfaces),
            announce_id_history: self.announce_id_history.history(i),
            responsiveness: self.routes.responsiveness()[i],
        })
    }

    pub fn forwarding_route_for(&self, destination: &DestinationHash) -> Option<ForwardingRoute> {
        let i = self.index_of(destination)?;
        Some(ForwardingRoute {
            hops: crate::units::HopCount(self.routes.hops()[i]),
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
        })
    }
}
