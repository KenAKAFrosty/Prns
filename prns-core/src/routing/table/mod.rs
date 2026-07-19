mod learning;
mod lookup;
mod model;

pub use model::RoutingTable;

use super::announce::defaults::route_expiry_millis;
use super::announce::stored::{
    AnnounceAppData, AnnounceIdHistory, AnnounceRecord, AnnounceRecordTable,
};
use super::announce::Announce;
use super::route_expiry::RouteExpiryIndex;
use super::routes::{RouteEntry, RouteTable};
use super::types::{
    AnnounceIdRing, NextHop, PersistedRouteRow, RemovedRoute, RouteRemovalCause,
    RouteResponsiveness, SeedRouteOutcome, StoredAnnounce,
};
use super::warmth::RouteWarmth;
use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::storage::TablePushError;
use crate::wire::{DestinationHash, TransportId};

impl<R, A, H, D, I> RoutingTable<R, A, H, D, I>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
    I: RouteExpiryIndex,
{
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
