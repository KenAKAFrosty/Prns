//! The routing layer: announces, the routing table, the rebroadcast schedule,
//! and the storage backends that hold each routing concern.
//!
//! `RoutingTable` is generic over three substitutable storage backends —
//! see [`storage`] for the trait catalogue. The default type parameters
//! resolve to the no_std stack-resident backends (`FixedArrayRouteColumns`,
//! `TieredAnnounceIdHistory`, `PackedAppDataArena`), so bare `RoutingTable`
//! is the embedded-friendly default and `DefaultRoutingTable<...>`
//! re-introduces the const generics that size them. A capable host
//! substitutes alternate backends (heap-resident, mmap-backed, etc.) at the
//! type parameters.

pub mod announce;
pub mod defaults;
pub mod schedule;
pub mod storage;
pub mod types;

use crate::engine::InstantMillis;
use crate::wire::DestinationHash;
use announce::Announce;
use defaults::DEFAULT_PATH_EXPIRY_MILLIS;
pub use defaults::{
    DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES, DEFAULT_HISTORY_FLOOR_PER_DESTINATION,
    DEFAULT_HISTORY_OVERFLOW_CAPACITY, DEFAULT_MAX_ANNOUNCE_IDS_PER_DESTINATION,
    DEFAULT_MAX_TRACKED_DESTINATIONS,
};
pub use storage::AnnounceIdHistoryView;
use storage::{
    AnnounceIdHistory, ColumnsFull, FixedArrayRouteColumns, PackedAppDataArena, RetainedAppData,
    RouteColumns, RouteEntry, TieredAnnounceIdHistory,
};
pub use types::{
    DropCause, ExistingRoute, RetainedAnnounce, RouteResponsiveness, UpsertRouteOutcome,
};

/// Routing table composed of three substitutable storage backends:
///
/// - `C: RouteColumns` — the SoA per-destination columns.
/// - `S: AnnounceIdHistory` — the per-destination set of recently-seen announce ids.
/// - `P: RetainedAppData` — the variable-length app_data arena.
///
/// Defaults resolve to the no_std stack-resident backends. See
/// [`DefaultRoutingTable`] for the const-generic-sized convenience alias
/// and [`crate::routing::storage`] for the trait catalogue.
///
/// `PartialEq` is structural — it compares every backend's representation
/// byte-for-byte. The engine's determinism tests rely on this; do not use it
/// to ask "do two tables built by different routes hold the same paths."
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoutingTable<
    C: RouteColumns = FixedArrayRouteColumns<DEFAULT_MAX_TRACKED_DESTINATIONS>,
    S: AnnounceIdHistory = TieredAnnounceIdHistory<
        DEFAULT_HISTORY_FLOOR_PER_DESTINATION,
        DEFAULT_HISTORY_OVERFLOW_CAPACITY,
        DEFAULT_MAX_TRACKED_DESTINATIONS,
        DEFAULT_MAX_ANNOUNCE_IDS_PER_DESTINATION,
    >,
    P: RetainedAppData = PackedAppDataArena<
        DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES,
        DEFAULT_MAX_TRACKED_DESTINATIONS,
    >,
> {
    columns: C,
    announce_id_history: S,
    retained_app_data: P,
}


/// Convenience alias for the no_std default backends parameterized by the
/// existing const-generic knobs. Lets call sites that want to tune the
/// no_std backends' sizes do so with a familiar const-generic shape rather
/// than spelling out the full `RoutingTable<C, S, P>`.
pub type DefaultRoutingTable<
    const MAX_TRACKED_DESTINATIONS: usize = DEFAULT_MAX_TRACKED_DESTINATIONS,
    const MAX_ANNOUNCE_IDS_PER_DESTINATION: usize = DEFAULT_MAX_ANNOUNCE_IDS_PER_DESTINATION,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize = DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES,
    const HISTORY_FLOOR_PER_DESTINATION: usize = DEFAULT_HISTORY_FLOOR_PER_DESTINATION,
    const HISTORY_OVERFLOW_CAPACITY: usize = DEFAULT_HISTORY_OVERFLOW_CAPACITY,
> = RoutingTable<
    FixedArrayRouteColumns<MAX_TRACKED_DESTINATIONS>,
    TieredAnnounceIdHistory<
        HISTORY_FLOOR_PER_DESTINATION,
        HISTORY_OVERFLOW_CAPACITY,
        MAX_TRACKED_DESTINATIONS,
        MAX_ANNOUNCE_IDS_PER_DESTINATION,
    >,
    PackedAppDataArena<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>,
>;

impl<C, S, P> RoutingTable<C, S, P>
where
    C: RouteColumns,
    S: AnnounceIdHistory,
    P: RetainedAppData,
{
    pub fn route_count(&self) -> usize {
        self.columns.len()
    }

    pub fn hop_count_to(&self, destination: &DestinationHash) -> Option<u8> {
        self.index_of(destination).map(|i| self.columns.hops()[i])
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.columns
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
    }

    pub fn existing_route_for(&self, destination: &DestinationHash) -> Option<ExistingRoute<'_>> {
        let i = self.index_of(destination)?;
        Some(ExistingRoute {
            hops: self.columns.hops()[i],
            expires: self.columns.expires()[i],
            announce_id_history: self.announce_id_history.history(i),
            responsiveness: self.columns.responsiveness()[i],
        })
    }

    /// Record a path the predicate just accepted. The structured `announce`
    /// carries every field needed to reproduce its exact wire payload via
    /// `Announce::to_wire` on re-emission (signature included).
    pub fn upsert_route(
        &mut self,
        hops: u8,
        arrived_at: InstantMillis,
        announce: &Announce<'_>,
    ) -> UpsertRouteOutcome {
        let expires = InstantMillis(arrived_at.0.saturating_add(DEFAULT_PATH_EXPIRY_MILLIS));
        match self.index_of(&announce.destination) {
            None => {
                // Bound-check the columns *before* taking up arena
                // space, so a ColumnsFull error doesn't leak a AppDataHandle.
                // For growable backends, `capacity()` is effectively infinite
                // and this check is a no-op.
                if self.columns.len() >= self.columns.capacity() {
                    return UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull);
                }
                let Ok(handle) = self.retained_app_data.insert(announce.app_data) else {
                    return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
                };
                let row = RouteEntry {
                    hops,
                    expires,
                    responsiveness: RouteResponsiveness::Responsive,
                    public_keys: announce.public_keys,
                    dotted_name_hash: announce.dotted_name_hash,
                    retained_announce_id: announce.announce_id,
                    maybe_ratchet: announce.ratchet,
                    signature: announce.signature,
                    maybe_app_data_handle: Some(handle),
                };
                match self.columns.push(announce.destination, row) {
                    Ok(i) => {
                        // New slot is empty (default-initialized); just record the id.
                        self.announce_id_history.remember(i, announce.announce_id);
                        UpsertRouteOutcome::Inserted
                    }
                    Err(ColumnsFull) => {
                        // Pre-check above should have caught this. Surfacing
                        // it defensively in case a backend's capacity policy
                        // is stricter than `capacity()` reports.
                        UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull)
                    }
                }
            }
            Some(i) => {
                // Two coherence boundaries here, not one:
                //   - Routing fields (hops, expires, responsiveness) are
                //     derived from the packet and the predicate's accept
                //     decision — independent of the announce payload.
                //   - Announce-correlated fields (public_keys, ratchet,
                //     signature, app_data) must stay consistent with each
                //     other (the signature signs the rest); refresh as one.
                //
                // So we refresh routing first (always survives), then attempt
                // the announce-payload replace, and only on its success do we
                // refresh the announce-correlated fields. If the arena can't
                // hold the new payload, the route stays freshened but the old
                // announce stays on hand for the call site to recover via the
                // egress cache (RNS's held-announce queue) once it exists.
                let maybe_handle = self.columns.app_data_handle()[i];
                self.columns.set_row(
                    i,
                    RouteEntry {
                        hops,
                        expires,
                        responsiveness: RouteResponsiveness::Responsive,
                        // Announce-correlated fields stay as they were until
                        // the replace clears.
                        public_keys: self.columns.public_keys()[i],
                        dotted_name_hash: self.columns.dotted_name_hash()[i],
                        retained_announce_id: self.columns.retained_announce_id()[i],
                        signature: self.columns.signature()[i],
                        maybe_ratchet: self.columns.ratchet()[i],
                        maybe_app_data_handle: maybe_handle,
                    },
                );

                // Any inserted destination carries a handle; the `Option` is
                // for default (unused) slots only. If the invariant ever
                // breaks, the routing was already refreshed — surface the
                // same outcome a failed replace would so the call site
                // recovers along the same path.
                let Some(handle) = maybe_handle else {
                    debug_assert!(false, "existing destination missing app_data handle");
                    return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
                };

                if self
                    .retained_app_data
                    .replace(handle, announce.app_data)
                    .is_err()
                {
                    return UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull);
                }

                self.announce_id_history.remember(i, announce.announce_id);
                self.columns.set_row(
                    i,
                    RouteEntry {
                        hops,
                        expires,
                        responsiveness: RouteResponsiveness::Responsive,
                        public_keys: announce.public_keys,
                        dotted_name_hash: announce.dotted_name_hash,
                        retained_announce_id: announce.announce_id,
                        maybe_ratchet: announce.ratchet,
                        signature: announce.signature,
                        maybe_app_data_handle: Some(handle),
                    },
                );
                UpsertRouteOutcome::Updated
            }
        }
    }

    /// The retained announce's `app_data` (its variable application payload),
    /// or `None` if the destination is unknown. The structured protocol fields
    /// (public_keys, ratchet, signature, …) are reached via `retained_announce`.
    pub fn app_data_for(&self, destination: &DestinationHash) -> Option<&[u8]> {
        Some(self.retained_announce_for(destination)?.announce.app_data)
    }

    /// Everything a rebroadcast needs about a known destination's retained
    /// announce (the hop count [and soon, transport id I believe?] plus the structured `Announce` itself) gathered
    /// in one lookup. `None` if the destination is unknown. Re-emission calls
    /// `announce.to_wire(buf)` to reproduce the original payload byte-identically
    /// so the retained signature still validates.
    pub fn retained_announce_for(
        &self,
        destination: &DestinationHash,
    ) -> Option<RetainedAnnounce<'_>> {
        let i = self.index_of(destination)?;
        let handle = self.columns.app_data_handle()[i]?;
        let app_data = self.retained_app_data.get(handle);
        Some(RetainedAnnounce {
            hops: self.columns.hops()[i],
            announce: Announce {
                destination: self.columns.destinations()[i],
                public_keys: self.columns.public_keys()[i],
                dotted_name_hash: self.columns.dotted_name_hash()[i],
                announce_id: self.columns.retained_announce_id()[i],
                ratchet: self.columns.ratchet()[i],
                signature: self.columns.signature()[i],
                app_data,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
    use crate::routing::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn announce_id(nonce_byte: u8, timebase: u64) -> AnnounceId {
        let mut bytes = [0u8; 10];
        bytes[..5].copy_from_slice(&[nonce_byte; 5]);
        bytes[5..].copy_from_slice(&timebase.to_be_bytes()[3..]);
        AnnounceId::from_wire(bytes)
    }

    /// A stand-in announce app_data, tagged so distinct ones are distinguishable.
    fn app_data(tag: u8) -> [u8; 16] {
        [tag; 16]
    }

    /// A synthetic announce with the routing-irrelevant fields zeroed. The path
    /// table doesn't inspect public_keys / dotted_name_hash / signature, so they
    /// can be filler for these tests — the ratchet-specific test passes `Some`.
    fn announce_for<'a>(
        destination: DestinationHash,
        announce_id: AnnounceId,
        ratchet: Option<RatchetKey>,
        app_data: &'a [u8],
    ) -> Announce<'a> {
        Announce {
            destination,
            public_keys: IdentityPublicKeys {
                encryption: X25519PublicKey([0u8; 32]),
                signing: Ed25519PublicKey([0u8; 32]),
            },
            dotted_name_hash: DottedNameHash::new([0u8; 10]),
            announce_id,
            ratchet,
            signature: Ed25519Signature([0u8; 64]),
            app_data,
        }
    }

    /// Record a path with a no-ratchet synthetic announce — the common case for
    /// these tests; ratchet-specific assertions construct the announce inline.
    fn record<const D: usize, const S: usize, const A: usize, const F: usize, const O: usize>(
        table: &mut DefaultRoutingTable<D, S, A, F, O>,
        destination: DestinationHash,
        hops: u8,
        arrival: InstantMillis,
        announce_id: AnnounceId,
        app_data: &[u8],
    ) -> UpsertRouteOutcome {
        table.upsert_route(
            hops,
            arrival,
            &announce_for(destination, announce_id, None, app_data),
        )
    }

    #[test]
    fn first_record_creates_a_path() {
        let mut table: RoutingTable = RoutingTable::default();
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
    fn refresh_updates_in_place_and_remembers_distinct_ids() {
        let mut table: RoutingTable = RoutingTable::default();
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
        assert_eq!(table.route_count(), 1); // same destination, not a second row
        assert_eq!(table.hop_count_to(&dest(1)), Some(2)); // hops refreshed

        let view = table.existing_route_for(&dest(1)).unwrap();
        assert_eq!(view.announce_id_history.len(), 2);
    }

    #[test]
    fn re_recording_the_same_id_does_not_duplicate_it() {
        let mut table: RoutingTable = RoutingTable::default();
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
                .existing_route_for(&dest(1))
                .unwrap()
                .announce_id_history
                .len(),
            1
        );
    }

    #[test]
    fn seen_set_evicts_oldest_when_full() {
        let mut table: RoutingTable = RoutingTable::default();
        // Fill past capacity; the first id must be evicted, the last retained.
        for n in 0..(DEFAULT_MAX_ANNOUNCE_IDS_PER_DESTINATION as u64 + 3) {
            record(
                &mut table,
                dest(1),
                1,
                InstantMillis(n),
                announce_id(0, n),
                &app_data(0),
            );
        }
        let view = table.existing_route_for(&dest(1)).unwrap();
        assert_eq!(
            view.announce_id_history.len(),
            DEFAULT_MAX_ANNOUNCE_IDS_PER_DESTINATION
        );
        // Oldest (timebase 0,1,2) gone; newest present.
        assert!(!view.announce_id_history.contains(&announce_id(0, 0)));
        assert!(view.announce_id_history.contains(&announce_id(
            0,
            DEFAULT_MAX_ANNOUNCE_IDS_PER_DESTINATION as u64 + 2
        )));
    }

    #[test]
    fn new_destinations_past_capacity_are_dropped() {
        let mut table: RoutingTable = RoutingTable::default();
        for n in 0..DEFAULT_MAX_TRACKED_DESTINATIONS {
            assert_eq!(
                record(
                    &mut table,
                    dest(n as u8),
                    1,
                    InstantMillis(0),
                    announce_id(0, n as u64),
                    &app_data(n as u8)
                ),
                UpsertRouteOutcome::Inserted
            );
        }
        assert_eq!(table.route_count(), DEFAULT_MAX_TRACKED_DESTINATIONS);
        // One destination too many: dropped, count unchanged.
        assert_eq!(
            record(
                &mut table,
                dest(0xFF),
                1,
                InstantMillis(0),
                announce_id(0, 999),
                &app_data(0xFF)
            ),
            UpsertRouteOutcome::Dropped(DropCause::RoutingTableFull)
        );
        assert_eq!(table.route_count(), DEFAULT_MAX_TRACKED_DESTINATIONS);
        // But a known destination still refreshes.
        assert_eq!(
            record(
                &mut table,
                dest(0),
                1,
                InstantMillis(1),
                announce_id(1, 1),
                &app_data(0)
            ),
            UpsertRouteOutcome::Updated
        );
    }

    #[test]
    fn record_retains_the_payload_and_refresh_replaces_it() {
        let mut table: RoutingTable = RoutingTable::default();
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(100),
            announce_id(0xAA, 1),
            &[1, 2, 3],
        );
        assert_eq!(table.app_data_for(&dest(1)), Some(&[1, 2, 3][..]));
        assert_eq!(table.app_data_for(&dest(2)), None); // unknown destination

        // A refresh swaps the retained announce, even to a different length.
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
        let mut table: RoutingTable = RoutingTable::default();
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
    fn a_new_path_whose_payload_overflows_the_arena_is_dropped() {
        // Arena holds 8 bytes total; entry/destination caps are generous.
        let mut table: DefaultRoutingTable<4, 8, 8> = DefaultRoutingTable::default();
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
        // The arena is now full, so a second path can't be backed: drop it whole.
        assert_eq!(
            record(
                &mut table,
                dest(2),
                1,
                InstantMillis(0),
                announce_id(2, 1),
                &[0xBB; 1]
            ),
            UpsertRouteOutcome::Dropped(DropCause::PayloadArenaFull)
        );
        assert_eq!(table.route_count(), 1);
        assert_eq!(table.hop_count_to(&dest(2)), None);
    }

    #[test]
    fn refresh_that_cannot_retain_a_better_announce_exits_early_with_the_cause() {
        // 8-byte arena: the first payload fits, but growing it past reclaim won't.
        let mut table: DefaultRoutingTable<4, 8, 8> = DefaultRoutingTable::default();
        record(
            &mut table,
            dest(1),
            5,
            InstantMillis(0),
            announce_id(1, 1),
            &[0xAA; 6],
        );
        assert_eq!(table.app_data_for(&dest(1)), Some(&[0xAA; 6][..]));

        // A better announce (fewer hops) arrives but its payload (9) won't fit even
        // after reclaiming the old 6. We surface the cause and bail; recovering the
        // dropped announce (handing it to the egress cache) is the call site's job.
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
        // The route was refreshed before the bail; the old announce stays on hand
        // (untouched) until the call site recovers.
        assert_eq!(table.hop_count_to(&dest(1)), Some(2));
        assert_eq!(table.app_data_for(&dest(1)), Some(&[0xAA; 6][..]));
    }

    #[test]
    fn ratchet_is_retained_for_faithful_rebroadcast() {
        let mut table: RoutingTable = RoutingTable::default();
        let ratchet = Some(RatchetKey::new([0xFE; 32]));
        let body = app_data(0xAA);
        // An announce carrying a ratchet must remember it structurally — the
        // signature is over the ratchet bytes, so re-emission needs the same one.
        table.upsert_route(
            3,
            InstantMillis(0),
            &announce_for(dest(1), announce_id(0xAA, 1), ratchet, &body),
        );
        let retained = table.retained_announce_for(&dest(1)).unwrap();
        assert_eq!(retained.announce.ratchet, ratchet);
        assert_eq!(retained.hops, 3);
        assert_eq!(retained.announce.app_data, &body[..]);

        // A refresh with a ratchet-less announce updates the retained ratchet.
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(1),
            announce_id(0xBB, 2),
            &app_data(0xBB),
        );
        let retained = table.retained_announce_for(&dest(1)).unwrap();
        assert_eq!(retained.announce.ratchet, None);
        assert_eq!(retained.hops, 2);

        // An unknown destination has no retained announce.
        assert!(table.retained_announce_for(&dest(2)).is_none());
    }
}
