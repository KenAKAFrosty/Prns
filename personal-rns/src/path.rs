//! The routing table: which destinations we can reach, in how many hops, and
//! the recent announces that taught us (plus the constants the acceptance
//! predicate enforces).
//!
//! `DestinationTable` is generic over three substitutable storage backends —
//! see [`crate::storage`] for the trait catalogue. The default type
//! parameters resolve to the no_std stack-resident backends
//! (`FixedArrayColumns`, `TieredSeenAnnounceIds`, `PayloadStore`), so bare
//! `DestinationTable` is the embedded-friendly default and
//! `DefaultDestinationTable<...>` re-introduces the const generics that size
//! them. A capable host substitutes alternate backends (heap-resident,
//! mmap-backed, etc.) at the type parameters.

use crate::announce::Announce;
use crate::engine::InstantMillis;
pub use crate::storage::SeenAnnounceIds;
use crate::storage::{
    AppDataBackend, ColumnsFull, DestinationColumns, FixedArrayColumns, PathRow, PayloadStore,
    SeenAnnounceIdsStorage, TieredSeenAnnounceIds,
};
use crate::wire::DestinationHash;

/// RNS's `RNS.Transport.PATHFINDER_M`
pub const MAX_HOP_COUNT: u8 = 128;

/// RNS's `RNS.Transport.PATHFINDER_E`
const DEFAULT_PATH_EXPIRY_MILLIS: u64 = 60 * 60 * 24 * 7 * 1000;

/// RNS's `RNS.Transport.MAX_RANDOM_BLOBS`
pub const DEFAULT_MAX_SEEN_ANNOUNCE_IDS: usize = 64;

/// How many destinations the table tracks. Fixed-capacity for the no-allocator
/// targets; a new destination arriving past this is dropped (v1 policy).
pub const DEFAULT_MAX_TRACKED_DESTINATIONS: usize = 32;

/// Total byte budget for retained announce **app_data** (the one variable,
/// genuinely-opaque tail of an announce — the rest is stored as structured
/// columns and serialized back to wire via `Announce::to_wire` on re-emission).
/// Sized as an average per path rather than worst-case × destination count, so
/// a packed [`PayloadStore`] backs a full table at a fraction of the worst-case
/// footprint. A capable host widens this independently of the destination
/// count.
pub const DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES: usize = DEFAULT_MAX_TRACKED_DESTINATIONS * 256;

/// Per-path inline floor for seen-announce-id retention. Every tracked path is
/// guaranteed this many slots regardless of arena pressure — covers the typical
/// multipath dedup fan-in (interfaces × routes) for small-to-medium mesh
/// deployments. See [`crate::storage::tiered_seen_ids`] for the full reasoning.
pub const DEFAULT_SEEN_IDS_FLOOR_PER_PATH: usize = 4;

/// Total shared overflow capacity for seen-announce-id retention. Sized as the
/// destination count × an average per-path overflow draw rather than worst-case.
///
/// Chatty paths borrow against the budget up to `MAX_SEEN_ANNOUNCE_IDS`, quiet
/// paths leave it for the chatty ones. A capable host widens this independently
/// of the floor.
pub const DEFAULT_SEEN_IDS_OVERFLOW_CAPACITY: usize = DEFAULT_MAX_TRACKED_DESTINATIONS * 8;

/// Whether a learned path is currently answering direct traffic. RNS tracks
/// this as a boolean `path_is_unresponsive`; modelled as a two-state type so the
/// predicate reads as intent rather than a bare flag. Only an `Unresponsive`
/// incumbent can be failed over from at equal evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathResponsiveness {
    Responsive,
    Unresponsive,
}

/// The fields of an existing path the acceptance predicate consults, gathered
/// from the table's columns on a lookup hit. The seen-id set is a borrowed
/// two-slice view ([`SeenAnnounceIds`]) over the tiered store, so the predicate
/// reads it in place.
#[derive(Debug, Clone, Copy)]
pub struct ExistingPath<'a> {
    pub hops: u8,
    pub expires: InstantMillis,
    pub seen_announce_ids: SeenAnnounceIds<'a>,
    pub responsiveness: PathResponsiveness,
}

/// What rebroadcasting a known destination's retained announce needs, gathered
/// from the table's columns on a hit: the hop count to emit, plus the structured
/// announce itself. Re-emission serializes it back to wire via
/// [`Announce::to_wire`], reproducing the original payload byte-identically so
/// the retained signature still validates.
#[derive(Debug, Clone)]
pub struct RetainedAnnounce<'a> {
    pub hops: u8,
    pub announce: Announce<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropCause {
    DestinationTableFull,
    PayloadArenaFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordPathOutcome {
    Inserted,
    Refreshed,
    Dropped(DropCause),
}

/// Routing table composed of three substitutable storage backends:
///
/// - `C: DestinationColumns` — the SoA per-destination columns.
/// - `S: SeenAnnounceIdsStorage` — the per-path set of recently-seen announce ids.
/// - `P: AppDataBackend` — the variable-length app_data arena.
///
/// Defaults resolve to the no_std stack-resident backends. See
/// [`DefaultDestinationTable`] for the const-generic-sized convenience alias
/// and [`crate::storage`] for the trait catalogue.
///
/// `PartialEq` is structural — it compares every backend's representation
/// byte-for-byte. The engine's determinism tests rely on this; do not use it
/// to ask "do two tables built by different routes hold the same paths."
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DestinationTable<
    C: DestinationColumns = FixedArrayColumns<DEFAULT_MAX_TRACKED_DESTINATIONS>,
    S: SeenAnnounceIdsStorage = TieredSeenAnnounceIds<
        DEFAULT_SEEN_IDS_FLOOR_PER_PATH,
        DEFAULT_SEEN_IDS_OVERFLOW_CAPACITY,
        DEFAULT_MAX_TRACKED_DESTINATIONS,
        DEFAULT_MAX_SEEN_ANNOUNCE_IDS,
    >,
    P: AppDataBackend = PayloadStore<
        DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES,
        DEFAULT_MAX_TRACKED_DESTINATIONS,
    >,
> {
    columns: C,
    seen_ids_store: S,
    app_data_store: P,
}

// I'm REALLY wondering if some of the messiness I'm feeling is because
// this default sorta blurs the lines, and at least should go elsewhere
// (like closer to the "default case *construction* site of all this")
// rather than fully baked in like this

/// Convenience alias for the no_std default backends parameterized by the
/// existing const-generic knobs. Lets call sites that want to tune the
/// no_std backends' sizes do so with a familiar const-generic shape rather
/// than spelling out the full `DestinationTable<C, S, P>`.
pub type DefaultDestinationTable<
    const MAX_TRACKED_DESTINATIONS: usize = DEFAULT_MAX_TRACKED_DESTINATIONS,
    const MAX_SEEN_ANNOUNCE_IDS: usize = DEFAULT_MAX_SEEN_ANNOUNCE_IDS,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize = DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES,
    const SEEN_IDS_FLOOR_PER_PATH: usize = DEFAULT_SEEN_IDS_FLOOR_PER_PATH,
    const SEEN_IDS_OVERFLOW_CAPACITY: usize = DEFAULT_SEEN_IDS_OVERFLOW_CAPACITY,
> = DestinationTable<
    FixedArrayColumns<MAX_TRACKED_DESTINATIONS>,
    TieredSeenAnnounceIds<
        SEEN_IDS_FLOOR_PER_PATH,
        SEEN_IDS_OVERFLOW_CAPACITY,
        MAX_TRACKED_DESTINATIONS,
        MAX_SEEN_ANNOUNCE_IDS,
    >,
    PayloadStore<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>,
>;

impl<C, S, P> DestinationTable<C, S, P>
where
    C: DestinationColumns,
    S: SeenAnnounceIdsStorage,
    P: AppDataBackend,
{
    pub fn path_count(&self) -> usize {
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

    pub fn existing_path_for(&self, destination: &DestinationHash) -> Option<ExistingPath<'_>> {
        let i = self.index_of(destination)?;
        Some(ExistingPath {
            hops: self.columns.hops()[i],
            expires: self.columns.expires()[i],
            seen_announce_ids: self.seen_ids_store.seen_ids(i),
            responsiveness: self.columns.responsiveness()[i],
        })
    }

    /// Record a path the predicate just accepted. The structured `announce`
    /// carries every field needed to reproduce its exact wire payload via
    /// `Announce::to_wire` on re-emission (signature included).
    pub fn record_accepted_path(
        &mut self,
        hops: u8,
        arrived_at: InstantMillis,
        announce: &Announce<'_>,
    ) -> RecordPathOutcome {
        let expires = InstantMillis(arrived_at.0.saturating_add(DEFAULT_PATH_EXPIRY_MILLIS));
        match self.index_of(&announce.destination) {
            None => {
                // Bound-check the columns *before* taking up arena
                // space, so a ColumnsFull error doesn't leak a PayloadHandle.
                // For growable backends, `capacity()` is effectively infinite
                // and this check is a no-op.
                if self.columns.len() >= self.columns.capacity() {
                    return RecordPathOutcome::Dropped(DropCause::DestinationTableFull);
                }
                let Ok(handle) = self.app_data_store.insert(announce.app_data) else {
                    return RecordPathOutcome::Dropped(DropCause::PayloadArenaFull);
                };
                let row = PathRow {
                    hops,
                    expires,
                    responsiveness: PathResponsiveness::Responsive,
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
                        self.seen_ids_store.remember(i, announce.announce_id);
                        RecordPathOutcome::Inserted
                    }
                    Err(ColumnsFull) => {
                        // Pre-check above should have caught this. Surfacing
                        // it defensively in case a backend's capacity policy
                        // is stricter than `capacity()` reports.
                        RecordPathOutcome::Dropped(DropCause::DestinationTableFull)
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
                    PathRow {
                        hops,
                        expires,
                        responsiveness: PathResponsiveness::Responsive,
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
                    return RecordPathOutcome::Dropped(DropCause::PayloadArenaFull);
                };

                if self
                    .app_data_store
                    .replace(handle, announce.app_data)
                    .is_err()
                {
                    return RecordPathOutcome::Dropped(DropCause::PayloadArenaFull);
                }

                self.seen_ids_store.remember(i, announce.announce_id);
                self.columns.set_row(
                    i,
                    PathRow {
                        hops,
                        expires,
                        responsiveness: PathResponsiveness::Responsive,
                        public_keys: announce.public_keys,
                        dotted_name_hash: announce.dotted_name_hash,
                        retained_announce_id: announce.announce_id,
                        maybe_ratchet: announce.ratchet,
                        signature: announce.signature,
                        maybe_app_data_handle: Some(handle),
                    },
                );
                RecordPathOutcome::Refreshed
            }
        }
    }

    /// The retained announce's `app_data` (its variable application payload),
    /// or `None` if the destination is unknown. The structured protocol fields
    /// (public_keys, ratchet, signature, …) are reached via `retained_announce`.
    pub fn announce_payload_for(&self, destination: &DestinationHash) -> Option<&[u8]> {
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
        let app_data = self.app_data_store.get(handle);
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
    use crate::announce::{AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};

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
        table: &mut DefaultDestinationTable<D, S, A, F, O>,
        destination: DestinationHash,
        hops: u8,
        arrival: InstantMillis,
        announce_id: AnnounceId,
        app_data: &[u8],
    ) -> RecordPathOutcome {
        table.record_accepted_path(
            hops,
            arrival,
            &announce_for(destination, announce_id, None, app_data),
        )
    }

    #[test]
    fn first_record_creates_a_path() {
        let mut table: DestinationTable = DestinationTable::default();
        assert_eq!(
            record(
                &mut table,
                dest(1),
                2,
                InstantMillis(100),
                announce_id(0xAA, 1),
                &app_data(0xAA)
            ),
            RecordPathOutcome::Inserted
        );
        assert_eq!(table.path_count(), 1);
        assert_eq!(table.hop_count_to(&dest(1)), Some(2));
        assert_eq!(table.hop_count_to(&dest(2)), None);
    }

    #[test]
    fn refresh_updates_in_place_and_remembers_distinct_ids() {
        let mut table: DestinationTable = DestinationTable::default();
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
        assert_eq!(table.path_count(), 1); // same destination, not a second row
        assert_eq!(table.hop_count_to(&dest(1)), Some(2)); // hops refreshed

        let view = table.existing_path_for(&dest(1)).unwrap();
        assert_eq!(view.seen_announce_ids.len(), 2);
    }

    #[test]
    fn re_recording_the_same_id_does_not_duplicate_it() {
        let mut table: DestinationTable = DestinationTable::default();
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
                .existing_path_for(&dest(1))
                .unwrap()
                .seen_announce_ids
                .len(),
            1
        );
    }

    #[test]
    fn seen_set_evicts_oldest_when_full() {
        let mut table: DestinationTable = DestinationTable::default();
        // Fill past capacity; the first id must be evicted, the last retained.
        for n in 0..(DEFAULT_MAX_SEEN_ANNOUNCE_IDS as u64 + 3) {
            record(
                &mut table,
                dest(1),
                1,
                InstantMillis(n),
                announce_id(0, n),
                &app_data(0),
            );
        }
        let view = table.existing_path_for(&dest(1)).unwrap();
        assert_eq!(view.seen_announce_ids.len(), DEFAULT_MAX_SEEN_ANNOUNCE_IDS);
        // Oldest (timebase 0,1,2) gone; newest present.
        assert!(!view.seen_announce_ids.contains(&announce_id(0, 0)));
        assert!(view
            .seen_announce_ids
            .contains(&announce_id(0, DEFAULT_MAX_SEEN_ANNOUNCE_IDS as u64 + 2)));
    }

    #[test]
    fn new_destinations_past_capacity_are_dropped() {
        let mut table: DestinationTable = DestinationTable::default();
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
                RecordPathOutcome::Inserted
            );
        }
        assert_eq!(table.path_count(), DEFAULT_MAX_TRACKED_DESTINATIONS);
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
            RecordPathOutcome::Dropped(DropCause::DestinationTableFull)
        );
        assert_eq!(table.path_count(), DEFAULT_MAX_TRACKED_DESTINATIONS);
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
            RecordPathOutcome::Refreshed
        );
    }

    #[test]
    fn record_retains_the_payload_and_refresh_replaces_it() {
        let mut table: DestinationTable = DestinationTable::default();
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(100),
            announce_id(0xAA, 1),
            &[1, 2, 3],
        );
        assert_eq!(table.announce_payload_for(&dest(1)), Some(&[1, 2, 3][..]));
        assert_eq!(table.announce_payload_for(&dest(2)), None); // unknown destination

        // A refresh swaps the retained announce, even to a different length.
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(200),
            announce_id(0xBB, 2),
            &[9, 9, 9, 9, 9],
        );
        assert_eq!(
            table.announce_payload_for(&dest(1)),
            Some(&[9, 9, 9, 9, 9][..])
        );
    }

    #[test]
    fn distinct_destinations_retain_independent_payloads() {
        let mut table: DestinationTable = DestinationTable::default();
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
        assert_eq!(table.announce_payload_for(&dest(1)), Some(&[0xA1; 4][..]));
        assert_eq!(table.announce_payload_for(&dest(2)), Some(&[0xB2; 7][..]));
        assert_eq!(table.announce_payload_for(&dest(3)), Some(&[0xC3; 2][..]));
    }

    #[test]
    fn a_new_path_whose_payload_overflows_the_arena_is_dropped() {
        // Arena holds 8 bytes total; entry/destination caps are generous.
        let mut table: DefaultDestinationTable<4, 8, 8> = DefaultDestinationTable::default();
        assert_eq!(
            record(
                &mut table,
                dest(1),
                1,
                InstantMillis(0),
                announce_id(1, 1),
                &[0xAA; 8]
            ),
            RecordPathOutcome::Inserted
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
            RecordPathOutcome::Dropped(DropCause::PayloadArenaFull)
        );
        assert_eq!(table.path_count(), 1);
        assert_eq!(table.hop_count_to(&dest(2)), None);
    }

    #[test]
    fn refresh_that_cannot_retain_a_better_announce_exits_early_with_the_cause() {
        // 8-byte arena: the first payload fits, but growing it past reclaim won't.
        let mut table: DefaultDestinationTable<4, 8, 8> = DefaultDestinationTable::default();
        record(
            &mut table,
            dest(1),
            5,
            InstantMillis(0),
            announce_id(1, 1),
            &[0xAA; 6],
        );
        assert_eq!(table.announce_payload_for(&dest(1)), Some(&[0xAA; 6][..]));

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
            RecordPathOutcome::Dropped(DropCause::PayloadArenaFull)
        );
        // The route was refreshed before the bail; the old announce stays on hand
        // (untouched) until the call site recovers.
        assert_eq!(table.hop_count_to(&dest(1)), Some(2));
        assert_eq!(table.announce_payload_for(&dest(1)), Some(&[0xAA; 6][..]));
    }

    #[test]
    fn ratchet_is_retained_for_faithful_rebroadcast() {
        let mut table: DestinationTable = DestinationTable::default();
        let ratchet = Some(RatchetKey::new([0xFE; 32]));
        let body = app_data(0xAA);
        // An announce carrying a ratchet must remember it structurally — the
        // signature is over the ratchet bytes, so re-emission needs the same one.
        table.record_accepted_path(
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
