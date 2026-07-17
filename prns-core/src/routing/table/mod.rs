use super::announce::defaults::route_expiry_millis;
use super::announce::stored::{
    AnnounceAppData, AnnounceIdHistory, AnnounceRecord, AnnounceRecordTable,
};
use super::announce::{Announce, AnnounceArrival};
use super::route_expiry::{LinearRouteExpiryIndex, RouteExpiryIndex};
use super::routes::{RouteEntry, RouteTable};
use super::types::{
    AnnounceIdRing, DropCause, ExistingRoute, ForwardingRoute, NextHop, PersistedRouteRow,
    RemovedRoute, RouteRemovalCause, RouteResponsiveness, SeedRouteOutcome, StoredAnnounce,
    UpsertRouteOutcome,
};
use super::warmth::RouteWarmth;
use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::storage::TablePushError;
use crate::wire::{DestinationHash, TransportId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eviction {
    Evicted,
    NothingToEvict,
}

/// RNS 1.3.5's `path_table`
///
/// NOTE: `PartialEq` compares backend representation byte-for-byte because the determinism tests rely on that. Do not use `==` and expect to compare the same set of routes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoutingTable<R, A, H, D, I = LinearRouteExpiryIndex>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
    I: RouteExpiryIndex,
{
    routes: R,
    route_expiries: I,
    announce_records: A,
    announce_id_history: H,
    announce_app_data: D,
}

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
        self.routes
            .receiving_interfaces()
            .iter()
            .filter(|&&learned_on| learned_on == interface)
            .count()
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

    /// RNS's `Transport.next_hop`.
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

    fn path_row_at(&self, i: usize) -> RouteEntry {
        RouteEntry {
            hops: self.routes.hops()[i],
            learned_at: self.routes.learned_at()[i],
            responsiveness: self.routes.responsiveness()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
            last_relayed_at: self.routes.last_relayed_at()[i],
        }
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
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

    /// Intentional deviation from the reference's learn-fixed `IDX_PT_EXPIRES` gate clock: once a link activation or a returned proof marks the route `Responsive`, the gate keeps our slid clock instead, refusing to trade a route that demonstrably works for one with longer hops.
    fn gate_expiry_of(&self, i: usize, interfaces: AttachedInterfaces<'_>) -> InstantMillis {
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

    fn expiry_of_with_warmth(
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

    pub fn forwarding_route_for(&self, destination: &DestinationHash) -> Option<ForwardingRoute> {
        let i = self.index_of(destination)?;
        Some(ForwardingRoute {
            hops: crate::units::HopCount(self.routes.hops()[i]),
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
        })
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

    pub fn upsert_route(
        &mut self,
        arrival: &AnnounceArrival<'_>,
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> UpsertRouteOutcome {
        self.upsert_route_with_warmth(arrival, interfaces, &(), on_removed)
    }

    pub fn upsert_route_with_warmth(
        &mut self,
        arrival: &AnnounceArrival<'_>,
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
                self.insert_new_route(arrival, interfaces, warmth, on_removed)
            }
            Some(i) => self.refresh_existing_route(i, arrival, interfaces, warmth),
        }
    }

    /// The route and announce-record tables advance row-for-row, so the composite fills when the smaller backend does; one can never outgrow the other.
    fn destination_capacity(&self) -> usize {
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
        on_removed(RemovedRoute {
            destination: self.routes.destinations()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            cause: RouteRemovalCause::Evicted,
        });
        self.remove_route(i);
        Eviction::Evicted
    }

    fn insert_new_route(
        &mut self,
        arrival: &AnnounceArrival<'_>,
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
            last_relayed_at: InstantMillis(0),
            responsiveness: RouteResponsiveness::Unknown,
            receiving_interface,
            next_hop,
        };
        let announce_entry = AnnounceRecord {
            public_keys: announce.public_keys,
            dotted_name_hash: announce.dotted_name_hash,
            announce_id: announce.announce_id,
            ratchet: announce.ratchet,
            signature: announce.signature,
            maybe_app_data_handle: Some(handle),
        };
        let routes_slot = match self.routes.push(announce.destination, route_entry) {
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
                last_relayed_at: InstantMillis(0),
                responsiveness: RouteResponsiveness::Unknown,
                receiving_interface,
                next_hop,
            },
        );
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

    pub fn remove_route(&mut self, i: usize) {
        let last = self.routes.len() - 1;
        let freed = self.announce_records.app_data_handles()[i];
        if let Some(handle) = freed {
            self.announce_app_data.free(handle);
        }
        self.routes.swap_remove(i, last);
        self.announce_records.swap_remove(i, last);
        self.announce_id_history.swap_remove(i, last);
        self.route_expiries.swap_remove(i, last);
    }

    pub fn drop_route(&mut self, destination: &DestinationHash) -> Option<RemovedRoute> {
        let i = self.index_of(destination)?;
        let removed = RemovedRoute {
            destination: self.routes.destinations()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            cause: RouteRemovalCause::Dropped,
        };
        self.remove_route(i);
        Some(removed)
    }

    pub fn drop_routes_via(
        &mut self,
        transport: TransportId,
        on_removed: &mut impl FnMut(RemovedRoute),
    ) -> usize {
        let mut dropped = 0;
        let mut i = 0;
        while i < self.routes.len() {
            if self.routes.next_hops()[i] != NextHop::Via(transport) {
                i += 1;
                continue;
            }
            let removed = RemovedRoute {
                destination: self.routes.destinations()[i],
                receiving_interface: self.routes.receiving_interfaces()[i],
                cause: RouteRemovalCause::Dropped,
            };
            self.remove_route(i);
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
        let mut i = 0;
        while i < self.routes.len() {
            if self.announce_records.public_keys()[i].identity_hash() != *identity {
                i += 1;
                continue;
            }
            let removed = RemovedRoute {
                destination: self.routes.destinations()[i],
                receiving_interface: self.routes.receiving_interfaces()[i],
                cause: RouteRemovalCause::Dropped,
            };
            self.remove_route(i);
            on_removed(removed);
            dropped += 1;
        }
        dropped
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
                let receiving_interface = self.routes.receiving_interfaces()[i];
                let cause = if interfaces
                    .iter()
                    .any(|descriptor| descriptor.id == receiving_interface)
                {
                    RouteRemovalCause::Expired
                } else {
                    RouteRemovalCause::InterfaceGone
                };
                on_removed(RemovedRoute {
                    destination: self.routes.destinations()[i],
                    receiving_interface,
                    cause,
                });
                self.remove_route(i);
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
            let receiving_interface = self.routes.receiving_interfaces()[i];
            let cause = if interfaces
                .iter()
                .any(|descriptor| descriptor.id == receiving_interface)
            {
                RouteRemovalCause::Expired
            } else {
                RouteRemovalCause::InterfaceGone
            };
            on_removed(RemovedRoute {
                destination: self.routes.destinations()[i],
                receiving_interface,
                cause,
            });
            self.remove_route(i);
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

    pub fn app_data_for(&self, destination: &DestinationHash) -> Option<&[u8]> {
        Some(self.stored_announce_for(destination)?.announce.app_data)
    }

    pub fn stored_announce_for(&self, destination: &DestinationHash) -> Option<StoredAnnounce<'_>> {
        let i = self.index_of(destination)?;
        let handle = self.announce_records.app_data_handles()[i]?;
        let app_data = self.announce_app_data.get(handle);
        Some(StoredAnnounce {
            hops: self.routes.hops()[i],
            receiving_interface: self.routes.receiving_interfaces()[i],
            next_hop: self.routes.next_hops()[i],
            announce: Announce {
                destination: self.routes.destinations()[i],
                public_keys: self.announce_records.public_keys()[i],
                dotted_name_hash: self.announce_records.dotted_name_hashes()[i],
                announce_id: self.announce_records.announce_ids()[i],
                ratchet: self.announce_records.ratchets()[i],
                signature: self.announce_records.signatures()[i],
                app_data,
            },
        })
    }

    /// Every row in the shape the persistence codec carries, serializable from the live table or from a cloned copy of it.
    pub fn persisted_rows(&self) -> impl Iterator<Item = PersistedRouteRow<'_>> + '_ {
        (0..self.routes.len()).map(move |i| PersistedRouteRow {
            destination: self.routes.destinations()[i],
            entry: RouteEntry {
                hops: self.routes.hops()[i],
                learned_at: self.routes.learned_at()[i],
                last_relayed_at: self.routes.last_relayed_at()[i],
                responsiveness: self.routes.responsiveness()[i],
                receiving_interface: self.routes.receiving_interfaces()[i],
                next_hop: self.routes.next_hops()[i],
            },
            public_keys: self.announce_records.public_keys()[i],
            dotted_name_hash: self.announce_records.dotted_name_hashes()[i],
            announce_id: self.announce_records.announce_ids()[i],
            ratchet: self.announce_records.ratchets()[i],
            signature: self.announce_records.signatures()[i],
            app_data: self.announce_records.app_data_handles()[i]
                .map_or(&[][..], |handle| self.announce_app_data.get(handle)),
            announce_id_ring: AnnounceIdRing::Table(self.announce_id_history.history(i)),
        })
    }

    /// Boot-restore twin of `insert_new_route`: the entry lands verbatim (hops, timestamps, responsiveness) and the replay ring replays oldest-first through `remember`. A full table or arena refuses instead of evicting — a seed never cannibalizes rows the live network already earned.
    pub fn seed_route(&mut self, row: &PersistedRouteRow<'_>) -> SeedRouteOutcome {
        if self.index_of(&row.destination).is_some() {
            return SeedRouteOutcome::AlreadyPresent;
        }
        if self.routes.len() >= self.destination_capacity() {
            return SeedRouteOutcome::TableFull;
        }
        let Ok(handle) = self.announce_app_data.insert(row.app_data) else {
            return SeedRouteOutcome::AppDataArenaFull;
        };
        let routes_slot = match self.routes.push(row.destination, row.entry) {
            Ok(i) => i,
            Err(TablePushError::TableFull) => {
                self.announce_app_data.free(handle);
                return SeedRouteOutcome::TableFull;
            }
        };
        let announce_entry = AnnounceRecord {
            public_keys: row.public_keys,
            dotted_name_hash: row.dotted_name_hash,
            announce_id: row.announce_id,
            ratchet: row.ratchet,
            signature: row.signature,
            maybe_app_data_handle: Some(handle),
        };
        if self.announce_records.push(announce_entry).is_err() {
            self.announce_app_data.free(handle);
            self.routes.swap_remove(routes_slot, self.routes.len() - 1);
            return SeedRouteOutcome::TableFull;
        }
        for id in row.announce_id_ring.ids() {
            self.announce_id_history.remember(routes_slot, id);
        }
        self.route_expiries.invalidate();
        SeedRouteOutcome::Seeded
    }
}

#[cfg(test)]
mod tests;
