mod learning;
mod lifetime;
mod lookup;
mod model;
mod updates;

pub use model::RoutingTable;

use super::announce::stored::{
    AnnounceAppData, AnnounceIdHistory, AnnounceRecord, AnnounceRecordTable,
};
use super::announce::Announce;
use super::route_expiry::RouteExpiryIndex;
use super::routes::{RouteEntry, RouteTable};
use super::types::{
    AnnounceIdRing, NextHop, PersistedRouteRow, RemovedRoute, RouteRemovalCause, SeedRouteOutcome,
    StoredAnnounce,
};
use crate::identity::IdentityHash;
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
