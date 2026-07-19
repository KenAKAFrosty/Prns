use super::RoutingTable;
use crate::crypto::Ed25519Signature;
use crate::routing::announce::stored::{
    AnnounceAppData, AnnounceIdHistory, AnnounceRecord, AnnounceRecordTable,
};
use crate::routing::announce::{
    AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey, ANNOUNCE_ID_WIRE_LEN,
};
use crate::routing::route_expiry::RouteExpiryIndex;
use crate::routing::routes::{RouteEntry, RouteTable};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

#[derive(Debug, Clone)]
pub struct PersistedRouteRow<'a> {
    pub destination: DestinationHash,
    pub entry: RouteEntry,
    pub public_keys: IdentityPublicKeys,
    pub dotted_name_hash: DottedNameHash,
    pub announce_id: AnnounceId,
    pub ratchet: Option<RatchetKey>,
    pub signature: Ed25519Signature,
    pub app_data: &'a [u8],
    pub announce_id_ring: AnnounceIdRing<'a>,
}

#[derive(Debug, Clone, Copy)]
pub enum AnnounceIdRing<'a> {
    Table(&'a [AnnounceId]),
    Wire(&'a [u8]),
}

impl AnnounceIdRing<'_> {
    pub fn len(&self) -> usize {
        match self {
            AnnounceIdRing::Table(ids) => ids.len(),
            AnnounceIdRing::Wire(bytes) => bytes.len() / ANNOUNCE_ID_WIRE_LEN,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Oldest first, matching the order `remember` replays them in.
    pub fn ids(&self) -> impl Iterator<Item = AnnounceId> + '_ {
        let (table, wire) = match self {
            AnnounceIdRing::Table(ids) => (Some(ids.iter().copied()), None),
            AnnounceIdRing::Wire(bytes) => (
                None,
                Some(bytes.chunks_exact(ANNOUNCE_ID_WIRE_LEN).map(|chunk| {
                    let mut bytes = [0u8; ANNOUNCE_ID_WIRE_LEN];
                    bytes.copy_from_slice(chunk);
                    AnnounceId::from_wire(bytes)
                })),
            ),
        };
        table
            .into_iter()
            .flatten()
            .chain(wire.into_iter().flatten())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedRouteOutcome {
    Seeded,
    AlreadyPresent,
    TableFull,
    AppDataArenaFull,
}

impl<R, A, H, D, I> RoutingTable<R, A, H, D, I>
where
    R: RouteTable,
    A: AnnounceRecordTable,
    H: AnnounceIdHistory,
    D: AnnounceAppData,
    I: RouteExpiryIndex,
{
    pub fn persisted_rows(&self) -> impl Iterator<Item = PersistedRouteRow<'_>> + '_ {
        (0..self.routes.len()).map(move |i| PersistedRouteRow {
            destination: self.routes.destinations()[i],
            entry: self.path_row_at(i),
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
