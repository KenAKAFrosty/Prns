use super::removal::{RemovedRoute, RouteRemovalCause};
use super::RoutingTable;
use crate::engine::InstantMillis;
use crate::interfaces::AttachedInterfaces;
use crate::routing::announce::stored::{
    AnnounceAppData, AnnounceIdHistory, AnnounceRecord, AnnounceRecordTable,
};
use crate::routing::announce::AnnounceArrival;
use crate::routing::route_expiry::RouteExpiryIndex;
use crate::routing::routes::{
    RouteEntry, RouteEvidenceId, RouteResponsiveness, RouteRetention, RouteTable,
};
use crate::routing::warmth::RouteWarmth;
use crate::storage::TablePushError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eviction {
    Evicted,
    NothingToEvict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropCause {
    RoutingTableFull,
    PayloadArenaFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertRouteOutcome {
    Inserted,
    Updated,
    Dropped(DropCause),
}

impl<R, A, H, D, I> RoutingTable<R, A, H, D, I>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
    I: RouteExpiryIndex,
{
    pub fn upsert_route(
        &mut self,
        arrival: &AnnounceArrival<'_>,
        replacement_evidence_id: RouteEvidenceId,
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> UpsertRouteOutcome {
        self.upsert_route_with_warmth(
            arrival,
            replacement_evidence_id,
            interfaces,
            &(),
            on_removed,
        )
    }

    pub fn upsert_route_with_warmth(
        &mut self,
        arrival: &AnnounceArrival<'_>,
        replacement_evidence_id: RouteEvidenceId,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> UpsertRouteOutcome {
        self.upsert_route_with_retention_and_warmth(
            arrival,
            replacement_evidence_id,
            RouteRetention::Network,
            interfaces,
            warmth,
            on_removed,
        )
    }

    pub(crate) fn upsert_route_with_retention_and_warmth(
        &mut self,
        arrival: &AnnounceArrival<'_>,
        replacement_evidence_id: RouteEvidenceId,
        retention: RouteRetention,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> UpsertRouteOutcome {
        match self.index_of(&arrival.announce.destination) {
            None => {
                if self.routes.len() >= self.destination_capacity() {
                    self.cull_expired_routes_with_warmth(
                        arrival.arrived_at,
                        interfaces,
                        warmth,
                        on_removed,
                    );
                    if self.routes.len() >= self.destination_capacity() {
                        self.evict_route_nearest_expiry(interfaces, warmth, on_removed);
                    }
                }
                self.insert_new_route(
                    arrival,
                    replacement_evidence_id,
                    retention,
                    interfaces,
                    warmth,
                    on_removed,
                )
            }
            Some(i) => self.refresh_existing_route(
                i,
                arrival,
                replacement_evidence_id,
                retention,
                interfaces,
                warmth,
            ),
        }
    }

    /// The route and announce-record tables advance row-for-row, so the composite fills when the smaller backend does; one can never outgrow the other.
    pub(super) fn destination_capacity(&self) -> usize {
        self.routes.capacity().min(self.announce_records.capacity())
    }

    fn evict_route_nearest_expiry(
        &mut self,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> Eviction {
        let Some(i) = (0..self.routes.len())
            .min_by_key(|&i| self.expiry_of_with_warmth(i, interfaces, warmth))
        else {
            return Eviction::NothingToEvict;
        };
        on_removed(self.route_removal_at(i, RouteRemovalCause::Evicted));
        self.remove_route_at(i);
        Eviction::Evicted
    }

    fn insert_new_route(
        &mut self,
        arrival: &AnnounceArrival<'_>,
        evidence_id: RouteEvidenceId,
        retention: RouteRetention,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> UpsertRouteOutcome {
        let &AnnounceArrival {
            ref announce,
            hops,
            arrived_at,
            receiving_interface,
            next_hop,
            ..
        } = arrival;
        if self.routes.len() >= self.destination_capacity() {
            return UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull);
        }
        let handle = match self.announce_app_data.insert(announce.app_data) {
            Ok(handle) => handle,
            Err(_) => {
                if self.evict_route_nearest_expiry(interfaces, warmth, on_removed)
                    == Eviction::NothingToEvict
                {
                    return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
                }
                match self.announce_app_data.insert(announce.app_data) {
                    Ok(handle) => handle,
                    Err(_) => return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull),
                }
            }
        };
        let route_entry = RouteEntry {
            hops,
            learned_at: arrived_at,
            last_route_activity_at: InstantMillis(0),
            responsiveness: RouteResponsiveness::Unknown,
            receiving_interface,
            next_hop,
            retention,
        };
        let announce_entry = AnnounceRecord {
            public_keys: announce.public_keys,
            dotted_name_hash: announce.dotted_name_hash,
            announce_id: announce.announce_id,
            ratchet: announce.ratchet,
            signature: announce.signature,
            maybe_app_data_handle: Some(handle),
        };
        let routes_slot = match self
            .routes
            .push(announce.destination, evidence_id, route_entry)
        {
            Ok(i) => i,
            Err(TablePushError::TableFull) => {
                self.announce_app_data.free(handle);
                return UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull);
            }
        };
        if self.announce_records.push(announce_entry).is_err() {
            self.announce_app_data.free(handle);
            self.routes.swap_remove(routes_slot, self.routes.len() - 1);
            return UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull);
        }
        self.announce_id_history
            .remember(routes_slot, announce.announce_id);
        let expiry = self.expiry_of_with_warmth(routes_slot, interfaces, warmth);
        self.route_expiries.insert(routes_slot, expiry);
        UpsertRouteOutcome::Inserted
    }

    fn refresh_existing_route(
        &mut self,
        i: usize,
        arrival: &AnnounceArrival<'_>,
        replacement_evidence_id: RouteEvidenceId,
        retention: RouteRetention,
        interfaces: AttachedInterfaces<'_>,
        warmth: &dyn RouteWarmth,
    ) -> UpsertRouteOutcome {
        let &AnnounceArrival {
            ref announce,
            hops,
            arrived_at,
            receiving_interface,
            next_hop,
            ..
        } = arrival;
        let path_changed = self.routes.receiving_interfaces()[i] != receiving_interface
            || self.routes.next_hops()[i] != next_hop;
        let Some(handle) = self.announce_records.app_data_handles()[i] else {
            debug_assert!(false, "existing destination missing app_data handle");
            return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
        };
        if self
            .announce_app_data
            .replace(handle, announce.app_data)
            .is_err()
        {
            return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
        }

        self.routes.set_row(
            i,
            RouteEntry {
                hops,
                learned_at: arrived_at,
                last_route_activity_at: InstantMillis(0),
                responsiveness: RouteResponsiveness::Unknown,
                receiving_interface,
                next_hop,
                retention,
            },
        );
        if path_changed {
            self.routes.set_evidence_id(i, replacement_evidence_id);
        }
        self.announce_records.set_row(
            i,
            AnnounceRecord {
                public_keys: announce.public_keys,
                dotted_name_hash: announce.dotted_name_hash,
                announce_id: announce.announce_id,
                ratchet: announce.ratchet,
                signature: announce.signature,
                maybe_app_data_handle: Some(handle),
            },
        );
        self.announce_id_history.remember(i, announce.announce_id);
        let expiry = self.expiry_of_with_warmth(i, interfaces, warmth);
        self.route_expiries.update(i, expiry);
        UpsertRouteOutcome::Updated
    }
}
