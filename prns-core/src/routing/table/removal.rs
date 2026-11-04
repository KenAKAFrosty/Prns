use super::RoutingTable;
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::announce::stored::{AnnounceAppData, AnnounceIdHistory, AnnounceRecordTable};
use crate::routing::route_expiry::RouteExpiryIndex;
use crate::routing::routes::{NextHop, RouteEvidenceHandle, RouteTable};
use crate::units::InstantMillis;
use crate::wire::{DestinationHash, TransportId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// repr(C): crosses the dual-core channel inside `Journaled`; see the layout note on `PrnsCommand`.
#[repr(C)]
pub enum RouteRemovalCause {
    Expired,
    Evicted,
    InterfaceGone,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemovedRoute {
    pub destination: DestinationHash,
    pub receiving_interface: InterfaceId,
    pub cause: RouteRemovalCause,
}

impl<R, A, H, D, I> RoutingTable<R, A, H, D, I>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
    I: RouteExpiryIndex,
{
    pub(super) fn route_removal_at(&self, row: usize, cause: RouteRemovalCause) -> RemovedRoute {
        RemovedRoute {
            destination: self.routes.destinations()[row],
            receiving_interface: self.routes.receiving_interfaces()[row],
            cause,
        }
    }

    pub(super) fn remove_route_at(&mut self, row: usize) {
        let last = self.routes.len() - 1;
        let freed = self.announce_records.app_data_handles()[row];
        if let Some(handle) = freed {
            self.announce_app_data.free(handle);
        }
        self.routes.swap_remove(row, last);
        self.announce_records.swap_remove(row, last);
        self.announce_id_history.swap_remove(row, last);
        self.route_expiries.swap_remove(row, last);
    }

    pub fn drop_route(&mut self, destination: &DestinationHash) -> Option<RemovedRoute> {
        let row = self.index_of(destination)?;
        let removed = self.route_removal_at(row, RouteRemovalCause::Dropped);
        self.remove_route_at(row);
        Some(removed)
    }

    /// A failed link may retire only its own path incarnation, and only if no newer announce or
    /// authenticated traffic has contradicted that attempt. Unlike marking responsiveness, this
    /// also retires the remembered announce history so a cached reply can discover another path.
    pub(crate) fn expire_failed_route(
        &mut self,
        handle: &mut RouteEvidenceHandle,
        attempt_started_at: InstantMillis,
    ) -> Option<RemovedRoute> {
        let row = self.resolve_route_evidence(handle)?;
        let last_active =
            self.routes.learned_at()[row].max(self.routes.last_route_activity_at()[row]);
        if last_active > attempt_started_at {
            return None;
        }
        let removed = self.route_removal_at(row, RouteRemovalCause::Expired);
        self.remove_route_at(row);
        Some(removed)
    }

    /// Administrative retirement bypasses network expiry and tunnel/departure warmth.
    pub fn drop_routes_for_interface(
        &mut self,
        interface: InterfaceId,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        let mut dropped = 0;
        let mut row = 0;
        while row < self.routes.len() {
            if !interface.administrative_scope_contains(self.routes.receiving_interfaces()[row]) {
                row += 1;
                continue;
            }
            let removed = self.route_removal_at(row, RouteRemovalCause::InterfaceGone);
            self.remove_route_at(row);
            on_removed(removed);
            dropped += 1;
        }
        dropped
    }

    pub fn drop_routes_via(
        &mut self,
        transport: TransportId,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        let mut dropped = 0;
        let mut row = 0;
        while row < self.routes.len() {
            if self.routes.next_hops()[row] != NextHop::Via(transport) {
                row += 1;
                continue;
            }
            let removed = self.route_removal_at(row, RouteRemovalCause::Dropped);
            self.remove_route_at(row);
            on_removed(removed);
            dropped += 1;
        }
        dropped
    }

    pub fn drop_routes_for_identity(
        &mut self,
        identity: &IdentityHash,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        let mut dropped = 0;
        let mut row = 0;
        while row < self.routes.len() {
            if self.announce_records.public_keys()[row].identity_hash() != *identity {
                row += 1;
                continue;
            }
            let removed = self.route_removal_at(row, RouteRemovalCause::Dropped);
            self.remove_route_at(row);
            on_removed(removed);
            dropped += 1;
        }
        dropped
    }
}
