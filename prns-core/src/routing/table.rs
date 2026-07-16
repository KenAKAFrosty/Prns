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

    #[cfg(feature = "alloc")]
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

    #[cfg(feature = "alloc")]
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

    /// RNS folds learn and relay into one path-table TIMESTAMP.
    /// We keep them apart and recombine here, so an actively-carried route never ages out mid-flow while its announces lull.
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

    /// Boundary-inclusive: a deadline must be actionable at its own instant or a reactor waking exactly at `expires` busy-spins.
    /// The reference culls on a 5s float-time poll, so the boundary is unobservable to parity.
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

    /// Boot-restore twin of `insert_new_route`: the entry lands verbatim (hops, timestamps, responsiveness) and the replay ring replays oldest-first through `remember`.
    /// A full table or arena refuses instead of evicting — a seed never cannibalizes rows the live network already earned.
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
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
    use crate::engine::test_support::routable_descriptor;
    use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
    use crate::interfaces::InterfaceDescriptor;
    use crate::interfaces::InterfaceMode;
    use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
    use crate::routing::announce::stored::{
        FixedAnnounceIdHistory, FixedArrayAnnounceRecordTable, PackedAppDataArena,
    };
    use crate::routing::routes::FixedArrayRouteTable;
    use crate::routing::NextHop;

    type TestRoutingTable<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
    > = RoutingTable<
        FixedArrayRouteTable<MAX_TRACKED_DESTINATIONS>,
        FixedArrayAnnounceRecordTable<MAX_TRACKED_DESTINATIONS>,
        FixedAnnounceIdHistory<MAX_TRACKED_DESTINATIONS, MAX_ANNOUNCE_IDS_PER_DESTINATION>,
        PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>,
    >;
    type Rt = TestRoutingTable<64, 64, 4096>;
    const RT_HISTORY_CAP: usize = 64;
    use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    fn source() -> InterfaceId {
        iface(0xEE)
    }

    fn announce_id(nonce_byte: u8, timebase: u64) -> AnnounceId {
        let mut bytes = [0u8; 10];
        bytes[..5].copy_from_slice(&[nonce_byte; 5]);
        bytes[5..].copy_from_slice(&timebase.to_be_bytes()[3..]);
        AnnounceId::from_wire(bytes)
    }

    fn app_data(tag: u8) -> [u8; 16] {
        [tag; 16]
    }

    fn announce_for<'a>(
        destination: DestinationHash,
        announce_id: AnnounceId,
        ratchet: Option<RatchetKey>,
        app_data: &'a [u8],
    ) -> Announce<'a> {
        Announce {
            destination,
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0u8; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0u8; 32])),
            },
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            announce_id,
            ratchet,
            signature: Ed25519Signature([0u8; 64]),
            app_data,
        }
    }

    fn full_interfaces() -> [InterfaceDescriptor; 1] {
        [routable_descriptor(source())]
    }

    fn view_with(mode: InterfaceMode) -> [InterfaceDescriptor; 1] {
        [InterfaceDescriptor {
            mode,
            ..routable_descriptor(source())
        }]
    }

    fn record<const D: usize, const S: usize, const A: usize>(
        table: &mut TestRoutingTable<D, S, A>,
        destination: DestinationHash,
        hops: u8,
        arrival: InstantMillis,
        announce_id: AnnounceId,
        app_data: &[u8],
    ) -> UpsertRouteOutcome {
        record_with_next_hop(
            table,
            destination,
            hops,
            arrival,
            announce_id,
            app_data,
            NextHop::Direct,
        )
    }

    fn record_with_next_hop<const D: usize, const S: usize, const A: usize>(
        table: &mut TestRoutingTable<D, S, A>,
        destination: DestinationHash,
        hops: u8,
        arrival: InstantMillis,
        announce_id: AnnounceId,
        app_data: &[u8],
        next_hop: NextHop,
    ) -> UpsertRouteOutcome {
        table.upsert_route(
            &AnnounceArrival {
                announce: announce_for(destination, announce_id, None, app_data),
                hops,
                arrived_at: arrival,
                receiving_interface: source(),
                next_hop,
                is_path_response: false,
            },
            AttachedInterfaces::new(&full_interfaces()),
            &mut |_| {},
        )
    }

    #[test]
    fn route_expiry_is_derived_from_the_mode_the_view_carries_now() {
        use crate::routing::announce::defaults::{
            ACCESS_POINT_ROUTE_EXPIRY_MILLIS, ROAMING_ROUTE_EXPIRY_MILLIS,
        };
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(1_000),
            announce_id(1, 1),
            &app_data(1),
        );

        for (mode, lifetime) in [
            (InterfaceMode::Full, DEFAULT_ROUTE_EXPIRY_MILLIS),
            (InterfaceMode::AccessPoint, ACCESS_POINT_ROUTE_EXPIRY_MILLIS),
            (InterfaceMode::Roaming, ROAMING_ROUTE_EXPIRY_MILLIS),
        ] {
            assert_eq!(
                table
                    .existing_route_for(&dest(1), AttachedInterfaces::new(&view_with(mode)))
                    .unwrap()
                    .expires_at,
                InstantMillis(1_000 + lifetime),
                "the same stored route re-keys to {mode:?} the moment the attached interfaces say so",
            );
        }
    }

    #[test]
    fn a_refresh_restarts_the_lifetime_from_its_own_arrival() {
        use crate::routing::announce::defaults::ROAMING_ROUTE_EXPIRY_MILLIS;
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(1_000),
            announce_id(1, 1),
            &app_data(1),
        );
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(2_000),
            announce_id(1, 2),
            &app_data(1),
        );
        assert_eq!(
            table
                .existing_route_for(&dest(1), AttachedInterfaces::new(&full_interfaces()))
                .unwrap()
                .expires_at,
            InstantMillis(2_000 + DEFAULT_ROUTE_EXPIRY_MILLIS),
            "the refresh restarts the clock",
        );
        assert_eq!(
            table
                .existing_route_for(
                    &dest(1),
                    AttachedInterfaces::new(&view_with(InterfaceMode::Roaming))
                )
                .unwrap()
                .expires_at,
            InstantMillis(2_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            "and the lifetime still follows whatever mode the attached interface carries",
        );
    }

    #[test]
    fn a_relay_slides_a_routes_expiry_forward_so_it_survives_mid_flow() {
        use crate::routing::announce::defaults::ROAMING_ROUTE_EXPIRY_MILLIS;
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(1_000),
            announce_id(1, 1),
            &app_data(1),
        );
        let roaming = view_with(InterfaceMode::Roaming);

        assert_eq!(
            table
                .existing_route_for(&dest(1), AttachedInterfaces::new(&roaming))
                .unwrap()
                .expires_at,
            InstantMillis(1_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            "the announce sets the baseline expiry clock",
        );

        table.note_relayed(&dest(1), InstantMillis(1_000_000));
        assert_eq!(
            table
                .existing_route_for(&dest(1), AttachedInterfaces::new(&roaming))
                .unwrap()
                .expires_at,
            InstantMillis(1_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            "the announce gate keeps the reference's learn-anchored clock while the route is unproven",
        );

        table.mark_responsiveness(&dest(1), RouteResponsiveness::Responsive);
        assert_eq!(
            table
                .existing_route_for(&dest(1), AttachedInterfaces::new(&roaming))
                .unwrap()
                .expires_at,
            InstantMillis(1_000_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            "once the route has proven itself, the gate follows the slid clock: relaying restarts it from the last carried packet",
        );

        table.mark_responsiveness(&dest(1), RouteResponsiveness::Unresponsive);
        assert_eq!(
            table
                .existing_route_for(&dest(1), AttachedInterfaces::new(&roaming))
                .unwrap()
                .expires_at,
            InstantMillis(1_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            "an unresponsive incumbent reads learn-anchored again, so longer-hop alternatives can clear the gate",
        );

        let mut removed = std::vec::Vec::new();
        table.cull_expired_routes(
            InstantMillis(1_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            AttachedInterfaces::new(&roaming),
            &mut |r| removed.push(r.destination),
        );
        assert!(
            removed.is_empty(),
            "the CULL clock slides regardless of responsiveness: the route the announce alone would have culled survives, because traffic still flows across it",
        );

        table.cull_expired_routes(
            InstantMillis(1_000_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            AttachedInterfaces::new(&roaming),
            &mut |r| removed.push(r.destination),
        );
        assert_eq!(
            removed,
            std::vec![dest(1)],
            "and it still ages out a full lifetime after its last relay",
        );
    }

    #[test]
    fn an_announce_refresh_clears_prior_relay_activity() {
        use crate::routing::announce::defaults::ROAMING_ROUTE_EXPIRY_MILLIS;
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(1_000),
            announce_id(1, 1),
            &app_data(1),
        );
        let roaming = view_with(InterfaceMode::Roaming);

        table.note_relayed(&dest(1), InstantMillis(500_000));
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(2_000),
            announce_id(1, 2),
            &app_data(1),
        );
        assert_eq!(
            table.existing_route_for(&dest(1), AttachedInterfaces::new(&roaming)).unwrap().expires_at,
            InstantMillis(2_000 + ROAMING_ROUTE_EXPIRY_MILLIS),
            "a fresh announce supersedes prior relay activity, exactly as RNS overwrites the path TIMESTAMP",
        );
    }

    #[test]
    fn eviction_prefers_a_newer_roaming_route_over_an_older_full_one() {
        const MAX: usize = 2;
        let mut table: TestRoutingTable<MAX, 8, 256> = TestRoutingTable::default();
        let full_interface = iface(0xA1);
        let roaming_interface = iface(0xB2);
        let two_mode_interfaces = [
            routable_descriptor(full_interface),
            InterfaceDescriptor {
                mode: InterfaceMode::Roaming,
                ..routable_descriptor(roaming_interface)
            },
        ];
        for (dest_byte, arrival, learned_on) in
            [(1u8, 0u64, full_interface), (2, 1_000, roaming_interface)]
        {
            assert_eq!(
                table.upsert_route(
                    &AnnounceArrival {
                        announce: announce_for(
                            dest(dest_byte),
                            announce_id(dest_byte, 1),
                            None,
                            &app_data(dest_byte)
                        ),
                        hops: 1,
                        arrived_at: InstantMillis(arrival),
                        receiving_interface: learned_on,
                        next_hop: NextHop::Direct,
                        is_path_response: false,
                    },
                    AttachedInterfaces::new(&two_mode_interfaces),
                    &mut |_| {},
                ),
                UpsertRouteOutcome::Inserted
            );
        }

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(3), announce_id(3, 1), None, &app_data(3)),
                    hops: 1,
                    arrived_at: InstantMillis(2_000),
                    receiving_interface: full_interface,
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                AttachedInterfaces::new(&two_mode_interfaces),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(2),
                receiving_interface: roaming_interface,
                cause: RouteRemovalCause::Evicted,
            }],
            "the roaming route expires in six hours, nearer death than the full one with a week to live",
        );
        assert_eq!(table.hop_count_to(&dest(1)), Some(1));
        assert_eq!(table.hop_count_to(&dest(3)), Some(1));
    }

    #[test]
    fn first_record_creates_a_path() {
        let mut table: Rt = Rt::default();
        assert_eq!(
            record(
                &mut table,
                dest(1),
                2,
                InstantMillis(100),
                announce_id(0xAA, 1),
                &app_data(0xAA)
            ),
            UpsertRouteOutcome::Inserted
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.hop_count_to(&dest(1)), Some(2));
        assert_eq!(table.hop_count_to(&dest(2)), None);
    }

    #[test]
    fn route_count_via_attributes_destinations_to_the_receiving_interface() {
        let mut table: Rt = Rt::default();
        let wifi = iface(0x01);
        let usb = iface(0x02);
        let silent = iface(0x03);

        for (dest_byte, id_byte, learned_on) in
            [(1u8, 0xA1u8, wifi), (2, 0xA2, wifi), (3, 0xA3, usb)]
        {
            assert_eq!(
                table.upsert_route(
                    &AnnounceArrival {
                        announce: announce_for(
                            dest(dest_byte),
                            announce_id(id_byte, 1),
                            None,
                            &app_data(id_byte)
                        ),
                        hops: 1,
                        arrived_at: InstantMillis(100),
                        receiving_interface: learned_on,
                        next_hop: NextHop::Direct,
                        is_path_response: false,
                    },
                    AttachedInterfaces::new(&full_interfaces()),
                    &mut |_| {},
                ),
                UpsertRouteOutcome::Inserted
            );
        }

        assert_eq!(table.route_count(), 3);
        assert_eq!(table.route_count_via(wifi), 2);
        assert_eq!(table.route_count_via(usb), 1);
        assert_eq!(table.route_count_via(silent), 0);

        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(1), announce_id(0xB1, 2), None, &app_data(0xB1)),
                    hops: 1,
                    arrived_at: InstantMillis(200),
                    receiving_interface: usb,
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                AttachedInterfaces::new(&full_interfaces()),
                &mut |_| {},
            ),
            UpsertRouteOutcome::Updated
        );
        assert_eq!(table.route_count(), 3);
        assert_eq!(table.route_count_via(wifi), 1);
        assert_eq!(table.route_count_via(usb), 2);
    }

    #[test]
    fn refresh_updates_in_place_and_remembers_distinct_ids() {
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            4,
            InstantMillis(100),
            announce_id(0xAA, 1),
            &app_data(0xAA),
        );
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(200),
            announce_id(0xBB, 2),
            &app_data(0xBB),
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.hop_count_to(&dest(1)), Some(2));

        let route = table
            .existing_route_for(&dest(1), AttachedInterfaces::new(&full_interfaces()))
            .unwrap();
        assert_eq!(route.announce_id_history.len(), 2);
    }

    #[test]
    fn re_recording_the_same_id_does_not_duplicate_it() {
        let mut table: Rt = Rt::default();
        let id = announce_id(0xAA, 1);
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(100),
            id,
            &app_data(0xAA),
        );
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(150),
            id,
            &app_data(0xAA),
        );
        assert_eq!(
            table
                .existing_route_for(&dest(1), AttachedInterfaces::new(&full_interfaces()))
                .unwrap()
                .announce_id_history
                .len(),
            1
        );
    }

    #[test]
    fn seen_set_evicts_oldest_when_full() {
        let mut table: Rt = Rt::default();
        for n in 0..(RT_HISTORY_CAP as u64 + 3) {
            record(
                &mut table,
                dest(1),
                1,
                InstantMillis(n),
                announce_id(0, n),
                &app_data(0),
            );
        }
        let route = table
            .existing_route_for(&dest(1), AttachedInterfaces::new(&full_interfaces()))
            .unwrap();
        assert_eq!(route.announce_id_history.len(), RT_HISTORY_CAP);
        assert!(!route.announce_id_history.contains(&announce_id(0, 0)));
        assert!(route
            .announce_id_history
            .contains(&announce_id(0, RT_HISTORY_CAP as u64 + 2)));
    }

    #[test]
    fn a_full_table_of_fresh_routes_evicts_the_one_nearest_expiry_for_a_newcomer() {
        const MAX: usize = 8;
        let mut table: TestRoutingTable<MAX, 8, 256> = TestRoutingTable::default();
        for n in 1..=MAX {
            assert_eq!(
                record(
                    &mut table,
                    dest(n as u8),
                    1,
                    InstantMillis(n as u64 * 10),
                    announce_id(0, n as u64),
                    &app_data(n as u8)
                ),
                UpsertRouteOutcome::Inserted
            );
        }
        assert_eq!(table.route_count(), MAX);

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(0xFF), announce_id(0, 999), None, &app_data(0xFF)),
                    hops: 1,
                    arrived_at: InstantMillis(100),
                    receiving_interface: source(),
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                AttachedInterfaces::new(&full_interfaces()),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
            "a full table of fresh routes admits the newcomer by eviction",
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(1),
                receiving_interface: source(),
                cause: RouteRemovalCause::Evicted,
            }],
            "the victim is the earliest arrival — the route nearest its expiry",
        );
        assert_eq!(table.route_count(), MAX);
        assert_eq!(table.hop_count_to(&dest(1)), None);
        assert_eq!(table.hop_count_to(&dest(0xFF)), Some(1));
        assert_eq!(
            record(
                &mut table,
                dest(2),
                1,
                InstantMillis(101),
                announce_id(1, 1),
                &app_data(2)
            ),
            UpsertRouteOutcome::Updated,
            "refreshing a survivor needs no slot",
        );
    }

    #[test]
    fn record_retains_the_payload_and_refresh_replaces_it() {
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(100),
            announce_id(0xAA, 1),
            &[1, 2, 3],
        );
        assert_eq!(table.app_data_for(&dest(1)), Some(&[1, 2, 3][..]));
        assert_eq!(table.app_data_for(&dest(2)), None);

        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(200),
            announce_id(0xBB, 2),
            &[9, 9, 9, 9, 9],
        );
        assert_eq!(table.app_data_for(&dest(1)), Some(&[9, 9, 9, 9, 9][..]));
    }

    #[test]
    fn distinct_destinations_retain_independent_payloads() {
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(0),
            announce_id(1, 1),
            &[0xA1; 4],
        );
        record(
            &mut table,
            dest(2),
            1,
            InstantMillis(0),
            announce_id(2, 1),
            &[0xB2; 7],
        );
        record(
            &mut table,
            dest(3),
            1,
            InstantMillis(0),
            announce_id(3, 1),
            &[0xC3; 2],
        );
        assert_eq!(table.app_data_for(&dest(1)), Some(&[0xA1; 4][..]));
        assert_eq!(table.app_data_for(&dest(2)), Some(&[0xB2; 7][..]));
        assert_eq!(table.app_data_for(&dest(3)), Some(&[0xC3; 2][..]));
    }

    #[test]
    fn a_new_path_that_overflows_the_arena_evicts_the_route_nearest_expiry() {
        let mut table: TestRoutingTable<4, 8, 8> = TestRoutingTable::default();
        assert_eq!(
            record(
                &mut table,
                dest(1),
                1,
                InstantMillis(0),
                announce_id(1, 1),
                &[0xAA; 8]
            ),
            UpsertRouteOutcome::Inserted
        );

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(2), announce_id(2, 1), None, &[0xBB; 1]),
                    hops: 1,
                    arrived_at: InstantMillis(10),
                    receiving_interface: source(),
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                AttachedInterfaces::new(&full_interfaces()),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
            "arena pressure evicts to admit the newcomer",
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(1),
                receiving_interface: source(),
                cause: RouteRemovalCause::Evicted,
            }],
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.hop_count_to(&dest(1)), None);
        assert_eq!(table.app_data_for(&dest(2)), Some(&[0xBB; 1][..]));
    }

    #[test]
    fn an_oversized_newcomer_takes_one_eviction_per_attempt_until_it_fits() {
        let mut table: TestRoutingTable<4, 8, 8> = TestRoutingTable::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(10),
            announce_id(1, 1),
            &[0xA1; 3],
        );
        record(
            &mut table,
            dest(2),
            1,
            InstantMillis(20),
            announce_id(2, 1),
            &[0xB2; 3],
        );

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(3), announce_id(3, 1), None, &[0xC3; 8]),
                    hops: 1,
                    arrived_at: InstantMillis(30),
                    receiving_interface: source(),
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                AttachedInterfaces::new(&full_interfaces()),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull),
            "one eviction was not enough, so this attempt drops",
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(1),
                receiving_interface: source(),
                cause: RouteRemovalCause::Evicted,
            }],
            "each attempt evicts at most one victim",
        );

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(3), announce_id(3, 2), None, &[0xC3; 8]),
                    hops: 1,
                    arrived_at: InstantMillis(40),
                    receiving_interface: source(),
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                AttachedInterfaces::new(&full_interfaces()),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
            "the retransmitted announce finds the room the first attempt made",
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(2),
                receiving_interface: source(),
                cause: RouteRemovalCause::Evicted,
            }],
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.app_data_for(&dest(3)), Some(&[0xC3; 8][..]));
    }

    #[test]
    fn refresh_that_cannot_retain_a_better_announce_leaves_the_table_untouched() {
        let mut table: TestRoutingTable<4, 8, 8> = TestRoutingTable::default();
        record(
            &mut table,
            dest(1),
            5,
            InstantMillis(0),
            announce_id(1, 1),
            &[0xAA; 6],
        );
        assert_eq!(table.app_data_for(&dest(1)), Some(&[0xAA; 6][..]));

        let outcome = record(
            &mut table,
            dest(1),
            2,
            InstantMillis(1),
            announce_id(2, 2),
            &[0xBB; 9],
        );
        assert_eq!(
            outcome,
            UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull)
        );
        assert_eq!(table.hop_count_to(&dest(1)), Some(5));
        assert_eq!(table.app_data_for(&dest(1)), Some(&[0xAA; 6][..]));
    }

    #[test]
    fn ratchet_is_retained_for_faithful_rebroadcast() {
        let mut table: Rt = Rt::default();
        let ratchet = Some(RatchetKey::new([0xFE; 32]));
        let body = app_data(0xAA);
        table.upsert_route(
            &AnnounceArrival {
                announce: announce_for(dest(1), announce_id(0xAA, 1), ratchet, &body),
                hops: 3,
                arrived_at: InstantMillis(0),
                receiving_interface: source(),
                next_hop: NextHop::Direct,
                is_path_response: false,
            },
            AttachedInterfaces::new(&full_interfaces()),
            &mut |_| {},
        );
        let stored = table.stored_announce_for(&dest(1)).unwrap();
        assert_eq!(stored.announce.ratchet, ratchet);
        assert_eq!(stored.hops, 3);
        assert_eq!(stored.announce.app_data, &body[..]);

        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(1),
            announce_id(0xBB, 2),
            &app_data(0xBB),
        );
        let stored = table.stored_announce_for(&dest(1)).unwrap();
        assert_eq!(stored.announce.ratchet, None);
        assert_eq!(stored.hops, 2);

        assert!(table.stored_announce_for(&dest(2)).is_none());
    }

    #[test]
    fn remove_route_drops_a_destination_and_keeps_the_rest_aligned() {
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(100),
            announce_id(0xA1, 1),
            &app_data(0x11),
        );
        record(
            &mut table,
            dest(2),
            2,
            InstantMillis(100),
            announce_id(0xA2, 1),
            &app_data(0x22),
        );
        record(
            &mut table,
            dest(3),
            3,
            InstantMillis(100),
            announce_id(0xA3, 1),
            &app_data(0x33),
        );
        assert_eq!(table.route_count(), 3);

        let slot = table.index_of(&dest(1)).unwrap();
        table.remove_route(slot);

        assert_eq!(table.route_count(), 2);
        assert_eq!(table.hop_count_to(&dest(1)), None);
        assert!(table.stored_announce_for(&dest(1)).is_none());

        assert_eq!(table.hop_count_to(&dest(2)), Some(2));
        assert_eq!(table.hop_count_to(&dest(3)), Some(3));
        assert_eq!(table.app_data_for(&dest(2)), Some(&app_data(0x22)[..]));
        assert_eq!(
            table.app_data_for(&dest(3)),
            Some(&app_data(0x33)[..]),
            "the moved row's app-data handle survives the free of the removed row's",
        );
        assert!(
            table
                .existing_route_for(&dest(3), AttachedInterfaces::new(&full_interfaces()))
                .unwrap()
                .announce_id_history
                .contains(&announce_id(0xA3, 1)),
            "dest 3's announce-id history moved into the hole intact",
        );
        assert!(table
            .existing_route_for(&dest(2), AttachedInterfaces::new(&full_interfaces()))
            .unwrap()
            .announce_id_history
            .contains(&announce_id(0xA2, 1)));
    }

    #[test]
    fn explicit_drops_target_one_destination_or_every_route_via_one_transport() {
        let mut table: Rt = Rt::default();
        let via_a = TransportId::new([0xA0; 16]);
        let via_b = TransportId::new([0xB0; 16]);
        for (destination, next_hop) in [
            (dest(1), NextHop::Direct),
            (dest(2), NextHop::Via(via_a)),
            (dest(3), NextHop::Via(via_a)),
            (dest(4), NextHop::Via(via_b)),
        ] {
            record_with_next_hop(
                &mut table,
                destination,
                1,
                InstantMillis(100),
                announce_id(destination.as_bytes()[0], 1),
                &app_data(destination.as_bytes()[0]),
                next_hop,
            );
        }

        assert_eq!(table.drop_route(&dest(9)), None);
        assert_eq!(
            table.drop_route(&dest(1)),
            Some(RemovedRoute {
                destination: dest(1),
                receiving_interface: source(),
                cause: RouteRemovalCause::Dropped,
            })
        );

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.drop_routes_via(via_a, &mut |route| removed.push(route)),
            2
        );
        removed.sort_by_key(|route| *route.destination.as_bytes());
        assert_eq!(
            removed,
            std::vec![
                RemovedRoute {
                    destination: dest(2),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Dropped,
                },
                RemovedRoute {
                    destination: dest(3),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Dropped,
                },
            ]
        );
        assert_eq!(
            table.path_rows().collect::<std::vec::Vec<_>>(),
            std::vec![(
                dest(4),
                RouteEntry {
                    hops: 1,
                    learned_at: InstantMillis(100),
                    last_relayed_at: InstantMillis(0),
                    responsiveness: RouteResponsiveness::Unknown,
                    receiving_interface: source(),
                    next_hop: NextHop::Via(via_b),
                }
            )]
        );
    }

    fn cull_a_mixed_table<R, A, H, D>(table: &mut RoutingTable<R, A, H, D>)
    where
        R: RouteTable,
        A: AnnounceRecordTable,
        H: AnnounceIdHistory,
        D: AnnounceAppData,
    {
        let stale_arrival = InstantMillis(0);
        let fresh_arrival = InstantMillis(1);
        for (dest_byte, arrival) in [
            (1u8, stale_arrival),
            (2, stale_arrival),
            (3, fresh_arrival),
            (4, stale_arrival),
            (5, fresh_arrival),
        ] {
            assert_eq!(
                table.upsert_route(
                    &AnnounceArrival {
                        announce: announce_for(
                            dest(dest_byte),
                            announce_id(dest_byte, 1),
                            None,
                            &[dest_byte; 4]
                        ),
                        hops: dest_byte,
                        arrived_at: arrival,
                        receiving_interface: source(),
                        next_hop: NextHop::Direct,
                        is_path_response: false,
                    },
                    AttachedInterfaces::new(&full_interfaces()),
                    &mut |_| {},
                ),
                UpsertRouteOutcome::Inserted
            );
        }
        assert_eq!(
            table.cull_expired_routes(
                fresh_arrival,
                AttachedInterfaces::new(&full_interfaces()),
                &mut |_| {}
            ),
            0,
            "nothing has expired yet"
        );
        assert_eq!(table.route_count(), 5);

        let mut culled_destinations = std::vec::Vec::new();
        let culled = table.cull_expired_routes(
            InstantMillis(DEFAULT_ROUTE_EXPIRY_MILLIS),
            AttachedInterfaces::new(&full_interfaces()),
            &mut |removed| culled_destinations.push(removed),
        );
        assert_eq!(
            culled, 3,
            "exactly the stale arrivals, expiry boundary inclusive"
        );
        assert_eq!(
            culled_destinations,
            std::vec![
                RemovedRoute {
                    destination: dest(1),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Expired,
                },
                RemovedRoute {
                    destination: dest(2),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Expired,
                },
                RemovedRoute {
                    destination: dest(4),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Expired,
                },
            ],
            "each removal reports the destination it dropped and why",
        );
        assert_eq!(table.route_count(), 2);
        for gone in [1u8, 2, 4] {
            assert_eq!(table.hop_count_to(&dest(gone)), None);
        }
        for kept in [3u8, 5] {
            assert_eq!(table.hop_count_to(&dest(kept)), Some(kept));
            assert_eq!(table.app_data_for(&dest(kept)), Some(&[kept; 4][..]));
            assert!(table
                .existing_route_for(&dest(kept), AttachedInterfaces::new(&full_interfaces()))
                .unwrap()
                .announce_id_history
                .contains(&announce_id(kept, 1)));
        }
    }

    #[test]
    fn cull_expired_routes_removes_every_expired_route_and_keeps_survivors_intact() {
        cull_a_mixed_table(&mut Rt::default());
    }

    #[test]
    fn cull_expired_routes_behaves_identically_on_the_heap_backend() {
        use crate::routing::announce::stored::{
            HeapAnnounceAppData, HeapAnnounceIdHistory, HeapAnnounceRecordTable,
        };
        use crate::routing::routes::HeapRouteTable;
        let mut table: RoutingTable<
            HeapRouteTable,
            HeapAnnounceRecordTable,
            HeapAnnounceIdHistory,
            HeapAnnounceAppData,
        > = RoutingTable::default();
        cull_a_mixed_table(&mut table);
    }

    #[cfg(feature = "std")]
    #[test]
    fn roaring_route_expiries_match_linear_route_semantics_across_mutations() {
        use crate::routing::announce::stored::{
            HeapAnnounceAppData, HeapAnnounceIdHistory, HeapAnnounceRecordTable,
        };
        use crate::routing::route_expiry::RoaringRouteExpiryIndex;
        use crate::routing::routes::HeapRouteTable;

        type IndexedTable = RoutingTable<
            HeapRouteTable,
            HeapAnnounceRecordTable,
            HeapAnnounceIdHistory,
            HeapAnnounceAppData,
            RoaringRouteExpiryIndex,
        >;

        let descriptors = full_interfaces();
        let interfaces = AttachedInterfaces::new(&descriptors);
        let mut indexed = IndexedTable::default();
        let mut linear = IndexedTable::default();
        for row in 0..80u8 {
            let destination = dest(row + 1);
            let payload = [row; 4];
            for table in [&mut indexed, &mut linear] {
                assert_eq!(
                    table.upsert_route(
                        &AnnounceArrival {
                            announce: announce_for(
                                destination,
                                announce_id(row + 1, u64::from(row) + 1),
                                None,
                                &payload,
                            ),
                            hops: row,
                            arrived_at: InstantMillis(u64::from(row) * 197_003),
                            receiving_interface: source(),
                            next_hop: NextHop::Direct,
                            is_path_response: false,
                        },
                        interfaces,
                        &mut |_| {},
                    ),
                    UpsertRouteOutcome::Inserted
                );
            }
        }

        assert_eq!(
            indexed.soonest_route_expiry_indexed_with_warmth(interfaces, &()),
            indexed.soonest_route_expiry_with_warmth(interfaces, &())
        );

        for table in [&mut indexed, &mut linear] {
            table.note_relayed_with_warmth(&dest(1), InstantMillis(20_000_000), interfaces, &());
            table.remove_route(17);
        }
        assert_eq!(
            indexed.soonest_route_expiry_indexed_with_warmth(interfaces, &()),
            indexed.soonest_route_expiry_with_warmth(interfaces, &())
        );

        let cutoff = InstantMillis(DEFAULT_ROUTE_EXPIRY_MILLIS + 2_000_000);
        let indexed_count =
            indexed.cull_expired_routes_indexed_with_warmth(cutoff, interfaces, &(), &mut |_| {});
        let linear_count =
            linear.cull_expired_routes_with_warmth(cutoff, interfaces, &(), &mut |_| {});
        assert_eq!(indexed_count, linear_count);

        let mut indexed_destinations = indexed
            .path_rows()
            .map(|(destination, _)| *destination.as_bytes())
            .collect::<std::vec::Vec<_>>();
        let mut linear_destinations = linear
            .path_rows()
            .map(|(destination, _)| *destination.as_bytes())
            .collect::<std::vec::Vec<_>>();
        indexed_destinations.sort_unstable();
        linear_destinations.sort_unstable();
        assert_eq!(indexed_destinations, linear_destinations);

        let roaming = view_with(InterfaceMode::Roaming);
        let roaming = AttachedInterfaces::new(&roaming);
        indexed.invalidate_route_expiries();
        assert_eq!(
            indexed.soonest_route_expiry_indexed_with_warmth(roaming, &()),
            indexed.soonest_route_expiry_with_warmth(roaming, &())
        );
    }

    #[test]
    fn a_full_table_culls_expired_routes_to_admit_a_new_destination() {
        const MAX: usize = 4;
        let mut table: TestRoutingTable<MAX, 8, 256> = TestRoutingTable::default();
        for (dest_byte, arrival) in [(1u8, 0u64), (2, 0), (3, 1), (4, 1)] {
            assert_eq!(
                record(
                    &mut table,
                    dest(dest_byte),
                    1,
                    InstantMillis(arrival),
                    announce_id(dest_byte, 1),
                    &app_data(dest_byte)
                ),
                UpsertRouteOutcome::Inserted
            );
        }
        assert_eq!(table.route_count(), MAX);

        let now = InstantMillis(DEFAULT_ROUTE_EXPIRY_MILLIS);
        assert_eq!(
            record(&mut table, dest(5), 1, now, announce_id(5, 1), &app_data(5)),
            UpsertRouteOutcome::Inserted
        );
        assert_eq!(
            table.route_count(),
            3,
            "both expired occupants culled, the newcomer admitted"
        );
        assert_eq!(table.hop_count_to(&dest(1)), None);
        assert_eq!(table.hop_count_to(&dest(2)), None);
        assert_eq!(table.hop_count_to(&dest(3)), Some(1));
        assert_eq!(table.hop_count_to(&dest(4)), Some(1));
        assert_eq!(table.app_data_for(&dest(5)), Some(&app_data(5)[..]));
    }

    #[test]
    fn a_route_whose_interface_left_the_view_is_culled_as_interface_gone() {
        let mut table: Rt = Rt::default();
        let surviving_interface = iface(0xA1);
        let vanishing_interface = iface(0xB2);
        let both = [
            routable_descriptor(surviving_interface),
            routable_descriptor(vanishing_interface),
        ];
        for (dest_byte, learned_on) in [(1u8, surviving_interface), (2, vanishing_interface)] {
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(
                        dest(dest_byte),
                        announce_id(dest_byte, 1),
                        None,
                        &app_data(dest_byte),
                    ),
                    hops: 1,
                    arrived_at: InstantMillis(1_000),
                    receiving_interface: learned_on,
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                AttachedInterfaces::new(&both),
                &mut |_| {},
            );
        }

        let shrunk = [routable_descriptor(surviving_interface)];
        assert_eq!(
            table.soonest_route_expiry(AttachedInterfaces::new(&shrunk)),
            Some(InstantMillis(1_000)),
            "the orphan earns no lifetime, so the lane is due the moment the attached interfaces shrink",
        );

        let mut removed = std::vec::Vec::new();
        let culled = table.cull_expired_routes(
            InstantMillis(2_000),
            AttachedInterfaces::new(&shrunk),
            &mut |removal| {
                removed.push(removal);
            },
        );
        assert_eq!(culled, 1);
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(2),
                receiving_interface: vanishing_interface,
                cause: RouteRemovalCause::InterfaceGone,
            }],
        );
        assert_eq!(table.hop_count_to(&dest(2)), None);
        assert_eq!(
            table.hop_count_to(&dest(1)),
            Some(1),
            "the route on the surviving interface is untouched",
        );
    }

    #[test]
    fn at_capacity_an_orphan_goes_as_interface_gone_before_any_fresh_eviction() {
        const MAX: usize = 2;
        let mut table: TestRoutingTable<MAX, 8, 256> = TestRoutingTable::default();
        let surviving_interface = iface(0xA1);
        let vanishing_interface = iface(0xB2);
        let both = [
            routable_descriptor(surviving_interface),
            routable_descriptor(vanishing_interface),
        ];
        for (dest_byte, learned_on) in [(1u8, surviving_interface), (2, vanishing_interface)] {
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(
                        dest(dest_byte),
                        announce_id(dest_byte, 1),
                        None,
                        &app_data(dest_byte),
                    ),
                    hops: 1,
                    arrived_at: InstantMillis(1_000),
                    receiving_interface: learned_on,
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                AttachedInterfaces::new(&both),
                &mut |_| {},
            );
        }

        let shrunk = [routable_descriptor(surviving_interface)];
        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(3), announce_id(3, 1), None, &app_data(3)),
                    hops: 1,
                    arrived_at: InstantMillis(2_000),
                    receiving_interface: surviving_interface,
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                AttachedInterfaces::new(&shrunk),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
        );
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(2),
                receiving_interface: vanishing_interface,
                cause: RouteRemovalCause::InterfaceGone,
            }],
            "the orphan is already due, so the inline cull takes it before eviction is consulted",
        );
        assert_eq!(table.hop_count_to(&dest(1)), Some(1));
        assert_eq!(table.hop_count_to(&dest(3)), Some(1));
    }

    #[test]
    fn expired_occupants_are_culled_before_any_fresh_route_is_evicted() {
        const MAX: usize = 4;
        let mut table: TestRoutingTable<MAX, 8, 256> = TestRoutingTable::default();
        for (dest_byte, arrival) in [(1u8, 0u64), (2, 0), (3, 1_000), (4, 1_000)] {
            record(
                &mut table,
                dest(dest_byte),
                1,
                InstantMillis(arrival),
                announce_id(dest_byte, 1),
                &app_data(dest_byte),
            );
        }

        let mut removed = std::vec::Vec::new();
        assert_eq!(
            table.upsert_route(
                &AnnounceArrival {
                    announce: announce_for(dest(5), announce_id(5, 1), None, &app_data(5)),
                    hops: 1,
                    arrived_at: InstantMillis(DEFAULT_ROUTE_EXPIRY_MILLIS),
                    receiving_interface: source(),
                    next_hop: NextHop::Direct,
                    is_path_response: false,
                },
                AttachedInterfaces::new(&full_interfaces()),
                &mut |removal| removed.push(removal),
            ),
            UpsertRouteOutcome::Inserted,
        );
        assert_eq!(
            removed,
            std::vec![
                RemovedRoute {
                    destination: dest(1),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Expired,
                },
                RemovedRoute {
                    destination: dest(2),
                    receiving_interface: source(),
                    cause: RouteRemovalCause::Expired,
                },
            ],
            "the expired occupants go as expirations; no fresh route is evicted",
        );
        assert_eq!(table.route_count(), 3);
        assert_eq!(table.hop_count_to(&dest(3)), Some(1));
        assert_eq!(table.hop_count_to(&dest(4)), Some(1));
        assert_eq!(table.hop_count_to(&dest(5)), Some(1));
    }

    #[test]
    fn mismatched_backend_capacities_fill_at_the_smaller_and_stay_aligned() {
        let mut table: RoutingTable<
            FixedArrayRouteTable<4>,
            FixedArrayAnnounceRecordTable<2>,
            FixedAnnounceIdHistory<4, 8>,
            PackedAppDataArena<256, 4>,
        > = RoutingTable::default();
        let mut removed = std::vec::Vec::new();
        for (dest_byte, arrival) in [(1u8, 10u64), (2, 20), (3, 30)] {
            assert_eq!(
                table.upsert_route(
                    &AnnounceArrival {
                        announce: announce_for(
                            dest(dest_byte),
                            announce_id(dest_byte, 1),
                            None,
                            &app_data(dest_byte),
                        ),
                        hops: 1,
                        arrived_at: InstantMillis(arrival),
                        receiving_interface: source(),
                        next_hop: NextHop::Direct,
                        is_path_response: false,
                    },
                    AttachedInterfaces::new(&full_interfaces()),
                    &mut |removal| removed.push(removal),
                ),
                UpsertRouteOutcome::Inserted,
            );
        }
        assert_eq!(
            removed,
            std::vec![RemovedRoute {
                destination: dest(1),
                receiving_interface: source(),
                cause: RouteRemovalCause::Evicted,
            }],
            "the composite fills at the smaller backend's capacity, so the third insert evicts instead of desyncing",
        );
        assert_eq!(table.route_count(), 2);
        assert_eq!(table.hop_count_to(&dest(1)), None);
        for kept in [2u8, 3] {
            assert!(
                table.stored_announce_for(&dest(kept)).is_some(),
                "every surviving route still reads its own announce record row",
            );
        }
    }

    #[test]
    fn remove_route_of_the_only_route_empties_the_table() {
        let mut table: Rt = Rt::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(100),
            announce_id(0xA1, 1),
            &app_data(0x11),
        );
        assert_eq!(table.route_count(), 1);

        table.remove_route(0);

        assert_eq!(table.route_count(), 0);
        assert_eq!(table.hop_count_to(&dest(1)), None);

        record(
            &mut table,
            dest(2),
            2,
            InstantMillis(200),
            announce_id(0xA2, 1),
            &app_data(0x22),
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.app_data_for(&dest(2)), Some(&app_data(0x22)[..]));
    }

    fn seedable_row<'a>(
        destination: DestinationHash,
        app_data: &'a [u8],
        ring: &'a [AnnounceId],
    ) -> PersistedRouteRow<'a> {
        PersistedRouteRow {
            destination,
            entry: RouteEntry {
                hops: 7,
                learned_at: InstantMillis(3_000),
                last_relayed_at: InstantMillis(5_000),
                responsiveness: RouteResponsiveness::Responsive,
                receiving_interface: source(),
                next_hop: NextHop::Direct,
            },
            public_keys: announce_for(destination, announce_id(1, 1), None, b"").public_keys,
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            announce_id: announce_id(9, 9),
            ratchet: None,
            signature: Ed25519Signature([0u8; 64]),
            app_data,
            announce_id_ring: AnnounceIdRing::Table(ring),
        }
    }

    #[test]
    fn a_seeded_row_carries_its_entry_verbatim_where_an_upsert_would_default_it() {
        let ring = [announce_id(1, 1), announce_id(2, 2), announce_id(3, 3)];
        let payload = app_data(0x5D);
        let row = seedable_row(dest(9), &payload, &ring);

        let mut table: Rt = Rt::default();
        assert_eq!(table.seed_route(&row), SeedRouteOutcome::Seeded);

        let seeded = table.path_row(&dest(9)).unwrap();
        assert_eq!(
            seeded, row.entry,
            "hops, timestamps, and responsiveness land untouched"
        );
        let seeded_ring: std::vec::Vec<_> = table
            .persisted_rows()
            .next()
            .unwrap()
            .announce_id_ring
            .ids()
            .collect();
        assert_eq!(seeded_ring, ring, "the replay ring replays in stored order");
        assert_eq!(table.app_data_for(&dest(9)), Some(&payload[..]));

        assert_eq!(table.seed_route(&row), SeedRouteOutcome::AlreadyPresent);
    }

    #[test]
    fn path_rows_carry_the_expiry_owned_by_routing_policy() {
        let ring = [announce_id(1, 1)];
        let payload = app_data(0x5D);
        let row = seedable_row(dest(9), &payload, &ring);
        let mut table: Rt = Rt::default();
        assert_eq!(table.seed_route(&row), SeedRouteOutcome::Seeded);
        let interfaces = full_interfaces();

        assert_eq!(
            table
                .path_rows_with_expiry(AttachedInterfaces::new(&interfaces), &())
                .collect::<std::vec::Vec<_>>(),
            std::vec![(
                dest(9),
                row.entry,
                InstantMillis(5_000 + DEFAULT_ROUTE_EXPIRY_MILLIS),
            )]
        );
    }

    #[test]
    fn a_seed_never_evicts_a_row_the_live_network_earned() {
        let mut table: TestRoutingTable<1, 4, 4096> = Default::default();
        record(
            &mut table,
            dest(1),
            1,
            InstantMillis(1_000),
            announce_id(1, 1),
            &app_data(0x11),
        );

        let ring = [announce_id(2, 2)];
        let payload = app_data(0x22);
        let refused = seedable_row(dest(2), &payload, &ring);
        assert_eq!(table.seed_route(&refused), SeedRouteOutcome::TableFull);
        assert!(
            table.has_route(&dest(1)),
            "the live row survives the refused seed"
        );
        assert_eq!(table.route_count(), 1);
    }

    #[test]
    fn a_flushed_table_seeds_back_to_the_same_persisted_rows() {
        use crate::persistence::{
            read_routing_table_snapshot, routing_table_snapshot_len, write_routing_table_snapshot,
        };

        let mut table: Rt = Rt::default();
        for n in 1..=3u8 {
            record(
                &mut table,
                dest(n),
                n,
                InstantMillis(1_000 * u64::from(n)),
                announce_id(n, u64::from(n)),
                &app_data(n),
            );
        }
        table.note_relayed(&dest(2), InstantMillis(9_000));

        let mut out = std::vec![0u8; routing_table_snapshot_len(table.persisted_rows())];
        let len = write_routing_table_snapshot(table.persisted_rows(), &mut out).unwrap();

        let mut reborn: Rt = Rt::default();
        for row in read_routing_table_snapshot(&out[..len]).unwrap() {
            assert_eq!(reborn.seed_route(&row.unwrap()), SeedRouteOutcome::Seeded);
        }

        for n in 1..=3u8 {
            assert_eq!(
                reborn.path_row(&dest(n)),
                table.path_row(&dest(n)),
                "row {n} survives the flush-seed round trip whole",
            );
            assert_eq!(reborn.app_data_for(&dest(n)), table.app_data_for(&dest(n)));
        }
    }
}
