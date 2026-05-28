//! The routing table: which destinations we can reach, in how many hops, and
//! the recent announces that taught us (plus the constants the acceptance
//! predicate enforces)
//!
//! Stored Struct-of-Arrays. The `destination` key column is packed contiguously
//! so the per-announce lookup (a linear scan keyed by a 16-byte hash) sweeps one
//! dense column instead of striding over whole entries; the cold columns are
//! gathered into an [`ExistingPath`] view only on a hit, so the pure predicate
//! stays layout-agnostic. Fixed-capacity and alloc-free — the capacities are the
//! embedded-target footprint knobs.

use crate::announce::{Announce, AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey};
use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::engine::InstantMillis;
use crate::payload_store::{PayloadHandle, PayloadStore};
pub use crate::tiered_seen_ids::SeenAnnounceIds;
use crate::tiered_seen_ids::TieredSeenAnnounceIds;
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
/// deployments. See [`crate::tiered_seen_ids`] for the full reasoning.
pub const DEFAULT_SEEN_IDS_FLOOR_PER_PATH: usize = 4;

/// Total shared overflow capacity for seen-announce-id retention. Sized as the
/// destination count × an average per-path overflow draw rather than worst-case
/// — chatty paths borrow against the budget up to `MAX_SEEN_ANNOUNCE_IDS`, quiet
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

/// Struct-of-Arrays routing table. The columns share one `len` and only the
/// table's own methods mutate them, together, so they can never desync.
///
/// `PartialEq` is structural: it compares every array slot, including the unused
/// tail past `len`, so `==` means "identical representation," not "same set of
/// paths." That is deliberate and is the only way we use it — the engine's
/// determinism tests feed two states the same operations in the same order, and
/// identical ops yield byte-identical tables, so any divergence (even a merely
/// reordered one) is caught.
///
/// Practical caution for a future reader: do not use
/// `==` to ask whether two tables built by *different* routes mean the same
/// thing. Insertion order fixes the column order, and any future removal would
/// leave stale bytes in the vacated tail slot — both compare unequal while being
/// logically equivalent. If you need that, compare the live prefix `[..len]`
/// (order-insensitively) instead of deriving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathTable<
    const MAX_TRACKED_DESTINATIONS: usize = DEFAULT_MAX_TRACKED_DESTINATIONS,
    const MAX_SEEN_ANNOUNCE_IDS: usize = DEFAULT_MAX_SEEN_ANNOUNCE_IDS,
    const ANNOUNCE_APP_DATA_ARENA_BYTES: usize = DEFAULT_ANNOUNCE_APP_DATA_ARENA_BYTES,
    const SEEN_IDS_FLOOR_PER_PATH: usize = DEFAULT_SEEN_IDS_FLOOR_PER_PATH,
    const SEEN_IDS_OVERFLOW_CAPACITY: usize = DEFAULT_SEEN_IDS_OVERFLOW_CAPACITY,
> {
    len: usize,
    destination: [DestinationHash; MAX_TRACKED_DESTINATIONS],
    hops: [u8; MAX_TRACKED_DESTINATIONS],
    expires: [InstantMillis; MAX_TRACKED_DESTINATIONS],
    responsiveness: [PathResponsiveness; MAX_TRACKED_DESTINATIONS],
    /// Two-tier seen-id store: each path gets an inline floor of guaranteed
    /// slots plus a share of the packed overflow arena. Footprint stays small
    /// regardless of how many paths are chatty vs quiet — see
    /// [`crate::tiered_seen_ids`].
    seen_ids: TieredSeenAnnounceIds<
        SEEN_IDS_FLOOR_PER_PATH,
        SEEN_IDS_OVERFLOW_CAPACITY,
        MAX_TRACKED_DESTINATIONS,
    >,
    // The retained announce, as structured fields — gathered into an
    // `Announce<'_>` on a hit. Re-emission re-serializes via `Announce::to_wire`
    // (round-trip byte-identical to the received payload, so the stored
    // signature still validates).
    public_keys: [IdentityPublicKeys; MAX_TRACKED_DESTINATIONS],
    dotted_name_hash: [DottedNameHash; MAX_TRACKED_DESTINATIONS],
    retained_announce_id: [AnnounceId; MAX_TRACKED_DESTINATIONS],
    ratchet: [Option<RatchetKey>; MAX_TRACKED_DESTINATIONS],
    signature: [Ed25519Signature; MAX_TRACKED_DESTINATIONS],
    /// Handle into `app_data_store` for the announce's app_data (the one
    /// genuinely opaque, variable-length tail). `None` for unused rows; a handle
    /// to empty bytes when a better announce arrived that the arena couldn't
    /// fit and we cleared the old one rather than keep it stale.
    app_data: [Option<PayloadHandle>; MAX_TRACKED_DESTINATIONS],
    app_data_store: PayloadStore<ANNOUNCE_APP_DATA_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>,
}

impl<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_SEEN_ANNOUNCE_IDS: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
        const SEEN_IDS_FLOOR_PER_PATH: usize,
        const SEEN_IDS_OVERFLOW_CAPACITY: usize,
    > Default
    for PathTable<
        MAX_TRACKED_DESTINATIONS,
        MAX_SEEN_ANNOUNCE_IDS,
        ANNOUNCE_APP_DATA_ARENA_BYTES,
        SEEN_IDS_FLOOR_PER_PATH,
        SEEN_IDS_OVERFLOW_CAPACITY,
    >
{
    fn default() -> Self {
        Self {
            len: 0,
            destination: [DestinationHash::new([0u8; 16]); MAX_TRACKED_DESTINATIONS],
            hops: [0u8; MAX_TRACKED_DESTINATIONS],
            expires: [InstantMillis(0); MAX_TRACKED_DESTINATIONS],
            responsiveness: [PathResponsiveness::Responsive; MAX_TRACKED_DESTINATIONS],
            seen_ids: TieredSeenAnnounceIds::new(),
            public_keys: [IdentityPublicKeys {
                encryption: X25519PublicKey([0u8; 32]),
                signing: Ed25519PublicKey([0u8; 32]),
            }; MAX_TRACKED_DESTINATIONS],
            dotted_name_hash: [DottedNameHash::new([0u8; 10]); MAX_TRACKED_DESTINATIONS],
            retained_announce_id: [AnnounceId::from_wire([0u8; 10]); MAX_TRACKED_DESTINATIONS],
            ratchet: [None; MAX_TRACKED_DESTINATIONS],
            signature: [Ed25519Signature([0u8; 64]); MAX_TRACKED_DESTINATIONS],
            app_data: [None; MAX_TRACKED_DESTINATIONS],
            app_data_store: PayloadStore::new(),
        }
    }
}

impl<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_SEEN_ANNOUNCE_IDS: usize,
        const ANNOUNCE_APP_DATA_ARENA_BYTES: usize,
        const SEEN_IDS_FLOOR_PER_PATH: usize,
        const SEEN_IDS_OVERFLOW_CAPACITY: usize,
    >
    PathTable<
        MAX_TRACKED_DESTINATIONS,
        MAX_SEEN_ANNOUNCE_IDS,
        ANNOUNCE_APP_DATA_ARENA_BYTES,
        SEEN_IDS_FLOOR_PER_PATH,
        SEEN_IDS_OVERFLOW_CAPACITY,
    >
{
    pub fn path_count(&self) -> usize {
        self.len
    }

    pub fn hop_count_to(&self, destination: &DestinationHash) -> Option<u8> {
        self.index_of(destination).map(|i| self.hops[i])
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.destination[..self.len]
            .iter()
            .position(|candidate| candidate == destination)
    }

    pub fn existing_path(&self, destination: &DestinationHash) -> Option<ExistingPath<'_>> {
        let i = self.index_of(destination)?;
        Some(ExistingPath {
            hops: self.hops[i],
            expires: self.expires[i],
            seen_announce_ids: self.seen_ids.seen_ids(i),
            responsiveness: self.responsiveness[i],
        })
    }

    /// Record a path the predicate just accepted. The structured `announce`
    /// carries every field needed to reproduce its exact wire payload via
    /// `Announce::to_wire` on re-emission (signature included).
    pub fn record_accepted_path(
        &mut self,
        hops: u8,
        arrival: InstantMillis,
        announce: &Announce<'_>,
    ) -> RecordPathOutcome {
        // The tiered seen-ids store packs per-path overflow length in u8; the
        // largest possible value is `MAX_SEEN_ANNOUNCE_IDS - SEEN_IDS_FLOOR_PER_PATH`,
        // so the difference must fit in u8. If a host wants seen-id counts past
        // 255, the next move is the std-host storage backend with a wider type,
        // not silently widening this one.
        const {
            assert!(
                MAX_SEEN_ANNOUNCE_IDS.saturating_sub(SEEN_IDS_FLOOR_PER_PATH) <= u8::MAX as usize,
                "MAX_SEEN_ANNOUNCE_IDS - SEEN_IDS_FLOOR_PER_PATH must fit in u8 (overflow_len storage)"
            );
        }
        let expires = InstantMillis(arrival.0.saturating_add(DEFAULT_PATH_EXPIRY_MILLIS));
        match self.index_of(&announce.destination) {
            Some(i) => {
                self.hops[i] = hops;
                self.expires[i] = expires;
                self.responsiveness[i] = PathResponsiveness::Responsive;
                self.seen_ids
                    .remember(i, announce.announce_id, MAX_SEEN_ANNOUNCE_IDS);
                // Atomic refresh of the retained announce: try the variable
                // `app_data` first, and only update the structured fields if it
                // fits — otherwise the new signature wouldn't match the old
                // app_data. On arena overflow exit early surfacing the cause; the
                // call site recovers by handing the announce to the egress cache
                // (RNS's held-announce queue) once it exists.
                if let Some(handle) = self.app_data[i] {
                    if self
                        .app_data_store
                        .replace(handle, announce.app_data)
                        .is_err()
                    {
                        return RecordPathOutcome::Dropped(DropCause::PayloadArenaFull);
                    }
                }
                self.public_keys[i] = announce.public_keys;
                self.dotted_name_hash[i] = announce.dotted_name_hash;
                self.retained_announce_id[i] = announce.announce_id;
                self.ratchet[i] = announce.ratchet;
                self.signature[i] = announce.signature;
                RecordPathOutcome::Refreshed
            }
            None => {
                if self.len >= MAX_TRACKED_DESTINATIONS {
                    return RecordPathOutcome::Dropped(DropCause::DestinationTableFull);
                }
                // Retain app_data before committing the row. On arena overflow,
                // exit early surfacing the cause and install nothing — the call
                // site recovers on this outcome by handing the announce to the
                // egress cache once it exists.
                let Ok(handle) = self.app_data_store.insert(announce.app_data) else {
                    return RecordPathOutcome::Dropped(DropCause::PayloadArenaFull);
                };
                let i = self.len;
                self.destination[i] = announce.destination;
                self.hops[i] = hops;
                self.expires[i] = expires;
                self.responsiveness[i] = PathResponsiveness::Responsive;
                self.public_keys[i] = announce.public_keys;
                self.dotted_name_hash[i] = announce.dotted_name_hash;
                self.retained_announce_id[i] = announce.announce_id;
                self.ratchet[i] = announce.ratchet;
                self.signature[i] = announce.signature;
                // New slot is empty (default-initialized); just record the id.
                self.seen_ids
                    .remember(i, announce.announce_id, MAX_SEEN_ANNOUNCE_IDS);
                self.app_data[i] = Some(handle);
                self.len += 1;
                RecordPathOutcome::Inserted
            }
        }
    }

    /// The retained announce's `app_data` (its variable application payload),
    /// or `None` if the destination is unknown. The structured protocol fields
    /// (public_keys, ratchet, signature, …) are reached via `retained_announce`.
    pub fn announce_payload(&self, destination: &DestinationHash) -> Option<&[u8]> {
        Some(self.retained_announce(destination)?.announce.app_data)
    }

    /// Everything a rebroadcast needs about a known destination's retained
    /// announce — the hop count plus the structured `Announce` itself — gathered
    /// in one lookup. `None` if the destination is unknown. Re-emission calls
    /// `announce.to_wire(buf)` to reproduce the original payload byte-identically
    /// so the retained signature still validates.
    pub fn retained_announce(&self, destination: &DestinationHash) -> Option<RetainedAnnounce<'_>> {
        let i = self.index_of(destination)?;
        let handle = self.app_data[i]?;
        let app_data = self.app_data_store.get(handle);
        Some(RetainedAnnounce {
            hops: self.hops[i],
            announce: Announce {
                destination: self.destination[i],
                public_keys: self.public_keys[i],
                dotted_name_hash: self.dotted_name_hash[i],
                announce_id: self.retained_announce_id[i],
                ratchet: self.ratchet[i],
                signature: self.signature[i],
                app_data,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::announce::AnnounceId;

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
        table: &mut PathTable<D, S, A, F, O>,
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
        let mut table: PathTable = PathTable::default();
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
        let mut table: PathTable = PathTable::default();
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

        let view = table.existing_path(&dest(1)).unwrap();
        assert_eq!(view.seen_announce_ids.len(), 2);
    }

    #[test]
    fn re_recording_the_same_id_does_not_duplicate_it() {
        let mut table: PathTable = PathTable::default();
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
                .existing_path(&dest(1))
                .unwrap()
                .seen_announce_ids
                .len(),
            1
        );
    }

    #[test]
    fn seen_set_evicts_oldest_when_full() {
        let mut table: PathTable = PathTable::default();
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
        let view = table.existing_path(&dest(1)).unwrap();
        assert_eq!(view.seen_announce_ids.len(), DEFAULT_MAX_SEEN_ANNOUNCE_IDS);
        // Oldest (timebase 0,1,2) gone; newest present.
        assert!(!view.seen_announce_ids.contains(&announce_id(0, 0)));
        assert!(view
            .seen_announce_ids
            .contains(&announce_id(0, DEFAULT_MAX_SEEN_ANNOUNCE_IDS as u64 + 2)));
    }

    #[test]
    fn new_destinations_past_capacity_are_dropped() {
        let mut table: PathTable = PathTable::default();
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
        let mut table: PathTable = PathTable::default();
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(100),
            announce_id(0xAA, 1),
            &[1, 2, 3],
        );
        assert_eq!(table.announce_payload(&dest(1)), Some(&[1, 2, 3][..]));
        assert_eq!(table.announce_payload(&dest(2)), None); // unknown destination

        // A refresh swaps the retained announce, even to a different length.
        record(
            &mut table,
            dest(1),
            2,
            InstantMillis(200),
            announce_id(0xBB, 2),
            &[9, 9, 9, 9, 9],
        );
        assert_eq!(table.announce_payload(&dest(1)), Some(&[9, 9, 9, 9, 9][..]));
    }

    #[test]
    fn distinct_destinations_retain_independent_payloads() {
        let mut table: PathTable = PathTable::default();
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
        assert_eq!(table.announce_payload(&dest(1)), Some(&[0xA1; 4][..]));
        assert_eq!(table.announce_payload(&dest(2)), Some(&[0xB2; 7][..]));
        assert_eq!(table.announce_payload(&dest(3)), Some(&[0xC3; 2][..]));
    }

    #[test]
    fn a_new_path_whose_payload_overflows_the_arena_is_dropped() {
        // Arena holds 8 bytes total; entry/destination caps are generous.
        let mut table: PathTable<4, 8, 8> = PathTable::default();
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
        let mut table: PathTable<4, 8, 8> = PathTable::default();
        record(
            &mut table,
            dest(1),
            5,
            InstantMillis(0),
            announce_id(1, 1),
            &[0xAA; 6],
        );
        assert_eq!(table.announce_payload(&dest(1)), Some(&[0xAA; 6][..]));

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
        assert_eq!(table.announce_payload(&dest(1)), Some(&[0xAA; 6][..]));
    }

    #[test]
    fn ratchet_is_retained_for_faithful_rebroadcast() {
        let mut table: PathTable = PathTable::default();
        let ratchet = Some(RatchetKey::new([0xFE; 32]));
        let body = app_data(0xAA);
        // An announce carrying a ratchet must remember it structurally — the
        // signature is over the ratchet bytes, so re-emission needs the same one.
        table.record_accepted_path(
            3,
            InstantMillis(0),
            &announce_for(dest(1), announce_id(0xAA, 1), ratchet, &body),
        );
        let retained = table.retained_announce(&dest(1)).unwrap();
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
        let retained = table.retained_announce(&dest(1)).unwrap();
        assert_eq!(retained.announce.ratchet, None);
        assert_eq!(retained.hops, 2);

        // An unknown destination has no retained announce.
        assert!(table.retained_announce(&dest(2)).is_none());
    }
}
