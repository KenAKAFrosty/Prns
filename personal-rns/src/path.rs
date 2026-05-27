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

use crate::announce::AnnounceId;
use crate::engine::InstantMillis;
use crate::payload_store::{PayloadHandle, PayloadStore};
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

/// Total byte budget for retained announce payloads, shared across all tracked
/// destinations. Sized as a per-announce average rather than worst-case ×
/// destination count: a packed [`PayloadStore`] holds only the bytes each
/// announce actually carries, so betting ~256 B/announce (typical payloads run
/// ~150–230 B; the cap is ~481) backs a full table at a fraction of the
/// worst-case footprint. A capable host widens this independently of the
/// destination count.
pub const DEFAULT_ANNOUNCE_PAYLOAD_ARENA_BYTES: usize = DEFAULT_MAX_TRACKED_DESTINATIONS * 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RememberOutcome {
    AlreadyKnown,
    StoredFresh,
    StoredEvictingOldest,
}

/// A fixed-capacity, insertion-ordered set of the most-recently-heard announce
/// ids for one destination (a bounded FIFO with dedup). Re-hearing an id is a
/// no-op (no promotion). Mirrors RNS's `random_blobs.append(...)` then the
/// `random_blobs[-MAX_RANDOM_BLOBS:]` truncation.
/// <https://github.com/markqvist/Reticulum/blob/1.3.1/RNS/Transport.py#L1880-L1882>
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentAnnounceIds<const MAX_SEEN_ANNOUNCE_IDS: usize>(
    heapless::Vec<AnnounceId, MAX_SEEN_ANNOUNCE_IDS>,
);

impl<const MAX_SEEN_ANNOUNCE_IDS: usize> RecentAnnounceIds<MAX_SEEN_ANNOUNCE_IDS> {
    const fn new() -> Self {
        Self(heapless::Vec::new())
    }

    fn as_slice(&self) -> &[AnnounceId] {
        self.0.as_slice()
    }

    /// Record `announce_id` as recently heard, unless already present; evict the
    /// oldest-inserted id when at capacity
    fn remember(&mut self, announce_id: AnnounceId) -> RememberOutcome {
        if self.0.contains(&announce_id) {
            return RememberOutcome::AlreadyKnown;
        }
        if self.0.is_full() {
            let ids = self.0.as_mut_slice();
            ids.copy_within(1.., 0);
            *ids.last_mut().expect("a full set is non-empty") = announce_id;
            RememberOutcome::StoredEvictingOldest
        } else {
            let _ = self.0.push(announce_id);
            RememberOutcome::StoredFresh
        }
    }
}

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
/// from the table's columns on a lookup hit. Borrows the seen-id set rather than
/// copying it, so the predicate reads the column in place.
#[derive(Debug, Clone, Copy)]
pub struct ExistingPath<'a> {
    pub hops: u8,
    pub expires: InstantMillis,
    pub seen_announce_ids: &'a [AnnounceId],
    pub responsiveness: PathResponsiveness,
}

/// Why a path could not be recorded: either the destination table is full, or
/// the payload arena has no room to retain the announce that backs the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropCause {
    DestinationTableFull,
    PayloadArenaFull,
}

/// What recording an accepted announce did to the table. Names the outcomes the
/// bare success/failure of the write would otherwise collapse.
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
    const ANNOUNCE_PAYLOAD_ARENA_BYTES: usize = DEFAULT_ANNOUNCE_PAYLOAD_ARENA_BYTES,
> {
    len: usize,
    destination: [DestinationHash; MAX_TRACKED_DESTINATIONS],
    hops: [u8; MAX_TRACKED_DESTINATIONS],
    expires: [InstantMillis; MAX_TRACKED_DESTINATIONS],
    responsiveness: [PathResponsiveness; MAX_TRACKED_DESTINATIONS],
    seen_announce_ids_for_destination:
        [RecentAnnounceIds<MAX_SEEN_ANNOUNCE_IDS>; MAX_TRACKED_DESTINATIONS],
    /// Handle into `payloads` for each path's retained announce; `None` for
    /// unused rows. A live row always holds `Some`, though it may resolve to
    /// empty bytes when a better announce arrived that the arena couldn't fit.
    payload: [Option<PayloadHandle>; MAX_TRACKED_DESTINATIONS],
    payloads: PayloadStore<ANNOUNCE_PAYLOAD_ARENA_BYTES, MAX_TRACKED_DESTINATIONS>,
}

impl<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_SEEN_ANNOUNCE_IDS: usize,
        const ANNOUNCE_PAYLOAD_ARENA_BYTES: usize,
    > Default
    for PathTable<MAX_TRACKED_DESTINATIONS, MAX_SEEN_ANNOUNCE_IDS, ANNOUNCE_PAYLOAD_ARENA_BYTES>
{
    fn default() -> Self {
        Self {
            len: 0,
            destination: [DestinationHash::new([0u8; 16]); MAX_TRACKED_DESTINATIONS],
            hops: [0u8; MAX_TRACKED_DESTINATIONS],
            expires: [InstantMillis(0); MAX_TRACKED_DESTINATIONS],
            responsiveness: [PathResponsiveness::Responsive; MAX_TRACKED_DESTINATIONS],
            seen_announce_ids_for_destination: core::array::from_fn(|_| {
                RecentAnnounceIds::<MAX_SEEN_ANNOUNCE_IDS>::new()
            }),
            payload: [None; MAX_TRACKED_DESTINATIONS],
            payloads: PayloadStore::new(),
        }
    }
}

impl<
        const MAX_TRACKED_DESTINATIONS: usize,
        const MAX_SEEN_ANNOUNCE_IDS: usize,
        const ANNOUNCE_PAYLOAD_ARENA_BYTES: usize,
    > PathTable<MAX_TRACKED_DESTINATIONS, MAX_SEEN_ANNOUNCE_IDS, ANNOUNCE_PAYLOAD_ARENA_BYTES>
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
            seen_announce_ids: self.seen_announce_ids_for_destination[i].as_slice(),
            responsiveness: self.responsiveness[i],
        })
    }

    pub fn record_accepted_path(
        &mut self,
        destination: DestinationHash,
        hops: u8,
        arrival: InstantMillis,
        announce_id: AnnounceId,
        payload: &[u8],
    ) -> RecordPathOutcome {
        let expires = InstantMillis(arrival.0.saturating_add(DEFAULT_PATH_EXPIRY_MILLIS));
        match self.index_of(&destination) {
            Some(i) => {
                self.hops[i] = hops;
                self.expires[i] = expires;
                self.responsiveness[i] = PathResponsiveness::Responsive;
                self.seen_announce_ids_for_destination[i].remember(announce_id);
                // The route is refreshed above regardless; only retaining the
                // announce can fail. If a better announce won't fit, exit early
                // surfacing the cause rather than recovering here — the call site
                // recovers on this outcome by handing the announce to the egress
                // cache (RNS's held-announce queue) once it exists. The old payload
                // stays on hand until then; this column is the path-request cache,
                // bound by time (PATHFINDER_E), not memory.
                if let Some(handle) = self.payload[i] {
                    if self.payloads.replace(handle, payload).is_err() {
                        return RecordPathOutcome::Dropped(DropCause::PayloadArenaFull);
                    }
                }
                RecordPathOutcome::Refreshed
            }
            None => {
                if self.len >= MAX_TRACKED_DESTINATIONS {
                    return RecordPathOutcome::Dropped(DropCause::DestinationTableFull);
                }
                // Retain the announce before committing the row. On arena overflow,
                // exit early surfacing the cause and install nothing — the call site
                // recovers on this outcome by handing the announce to the egress
                // cache once it exists.
                let Ok(handle) = self.payloads.insert(payload) else {
                    return RecordPathOutcome::Dropped(DropCause::PayloadArenaFull);
                };
                let i = self.len;
                self.destination[i] = destination;
                self.hops[i] = hops;
                self.expires[i] = expires;
                self.responsiveness[i] = PathResponsiveness::Responsive;
                self.seen_announce_ids_for_destination[i] = RecentAnnounceIds::new();
                self.seen_announce_ids_for_destination[i].remember(announce_id);
                self.payload[i] = Some(handle);
                self.len += 1;
                RecordPathOutcome::Inserted
            }
        }
    }

    /// The retained announce payload for a known destination, or `None` if the
    /// destination is unknown or its announce isn't currently on hand (cleared
    /// to make room for a better one that didn't fit).
    pub fn announce_payload(&self, destination: &DestinationHash) -> Option<&[u8]> {
        let i = self.index_of(destination)?;
        let handle = self.payload[i]?;
        let bytes = self.payloads.get(handle);
        (!bytes.is_empty()).then_some(bytes)
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

    /// A stand-in announce payload, tagged so distinct ones are distinguishable.
    fn payload(tag: u8) -> [u8; 16] {
        [tag; 16]
    }

    #[test]
    fn recent_announce_ids_report_each_remember_outcome() {
        let mut recent = RecentAnnounceIds::<2>::new();
        assert_eq!(
            recent.remember(announce_id(0, 1)),
            RememberOutcome::StoredFresh
        );
        // Re-hearing the same id is a no-op (no promotion).
        assert_eq!(
            recent.remember(announce_id(0, 1)),
            RememberOutcome::AlreadyKnown
        );
        assert_eq!(
            recent.remember(announce_id(0, 2)),
            RememberOutcome::StoredFresh
        );
        // Capacity 2 is now full; a new id evicts the oldest-inserted (timebase 1).
        assert_eq!(
            recent.remember(announce_id(0, 3)),
            RememberOutcome::StoredEvictingOldest
        );
        assert!(!recent.as_slice().contains(&announce_id(0, 1)));
        assert!(recent.as_slice().contains(&announce_id(0, 3)));
    }

    #[test]
    fn first_record_creates_a_path() {
        let mut table: PathTable = PathTable::default();
        assert_eq!(
            table.record_accepted_path(
                dest(1),
                2,
                InstantMillis(100),
                announce_id(0xAA, 1),
                &payload(0xAA)
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
        table.record_accepted_path(
            dest(1),
            4,
            InstantMillis(100),
            announce_id(0xAA, 1),
            &payload(0xAA),
        );
        table.record_accepted_path(
            dest(1),
            2,
            InstantMillis(200),
            announce_id(0xBB, 2),
            &payload(0xBB),
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
        table.record_accepted_path(dest(1), 2, InstantMillis(100), id, &payload(0xAA));
        table.record_accepted_path(dest(1), 2, InstantMillis(150), id, &payload(0xAA));
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
            table.record_accepted_path(
                dest(1),
                1,
                InstantMillis(n),
                announce_id(0, n),
                &payload(0),
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
                table.record_accepted_path(
                    dest(n as u8),
                    1,
                    InstantMillis(0),
                    announce_id(0, n as u64),
                    &payload(n as u8)
                ),
                RecordPathOutcome::Inserted
            );
        }
        assert_eq!(table.path_count(), DEFAULT_MAX_TRACKED_DESTINATIONS);
        // One destination too many: dropped, count unchanged.
        assert_eq!(
            table.record_accepted_path(
                dest(0xFF),
                1,
                InstantMillis(0),
                announce_id(0, 999),
                &payload(0xFF)
            ),
            RecordPathOutcome::Dropped(DropCause::DestinationTableFull)
        );
        assert_eq!(table.path_count(), DEFAULT_MAX_TRACKED_DESTINATIONS);
        // But a known destination still refreshes.
        assert_eq!(
            table.record_accepted_path(
                dest(0),
                1,
                InstantMillis(1),
                announce_id(1, 1),
                &payload(0)
            ),
            RecordPathOutcome::Refreshed
        );
    }

    #[test]
    fn record_retains_the_payload_and_refresh_replaces_it() {
        let mut table: PathTable = PathTable::default();
        table.record_accepted_path(
            dest(1),
            2,
            InstantMillis(100),
            announce_id(0xAA, 1),
            &[1, 2, 3],
        );
        assert_eq!(table.announce_payload(&dest(1)), Some(&[1, 2, 3][..]));
        assert_eq!(table.announce_payload(&dest(2)), None); // unknown destination

        // A refresh swaps the retained announce, even to a different length.
        table.record_accepted_path(
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
        table.record_accepted_path(dest(1), 1, InstantMillis(0), announce_id(1, 1), &[0xA1; 4]);
        table.record_accepted_path(dest(2), 1, InstantMillis(0), announce_id(2, 1), &[0xB2; 7]);
        table.record_accepted_path(dest(3), 1, InstantMillis(0), announce_id(3, 1), &[0xC3; 2]);
        assert_eq!(table.announce_payload(&dest(1)), Some(&[0xA1; 4][..]));
        assert_eq!(table.announce_payload(&dest(2)), Some(&[0xB2; 7][..]));
        assert_eq!(table.announce_payload(&dest(3)), Some(&[0xC3; 2][..]));
    }

    #[test]
    fn a_new_path_whose_payload_overflows_the_arena_is_dropped() {
        // Arena holds 8 bytes total; entry/destination caps are generous.
        let mut table: PathTable<4, 8, 8> = PathTable::default();
        assert_eq!(
            table.record_accepted_path(dest(1), 1, InstantMillis(0), announce_id(1, 1), &[0xAA; 8]),
            RecordPathOutcome::Inserted
        );
        // The arena is now full, so a second path can't be backed: drop it whole.
        assert_eq!(
            table.record_accepted_path(dest(2), 1, InstantMillis(0), announce_id(2, 1), &[0xBB; 1]),
            RecordPathOutcome::Dropped(DropCause::PayloadArenaFull)
        );
        assert_eq!(table.path_count(), 1);
        assert_eq!(table.hop_count_to(&dest(2)), None);
    }

    #[test]
    fn refresh_that_cannot_retain_a_better_announce_exits_early_with_the_cause() {
        // 8-byte arena: the first payload fits, but growing it past reclaim won't.
        let mut table: PathTable<4, 8, 8> = PathTable::default();
        table.record_accepted_path(dest(1), 5, InstantMillis(0), announce_id(1, 1), &[0xAA; 6]);
        assert_eq!(table.announce_payload(&dest(1)), Some(&[0xAA; 6][..]));

        // A better announce (fewer hops) arrives but its payload (9) won't fit even
        // after reclaiming the old 6. We surface the cause and bail; recovering the
        // dropped announce (handing it to the egress cache) is the call site's job.
        let outcome =
            table.record_accepted_path(dest(1), 2, InstantMillis(1), announce_id(2, 2), &[0xBB; 9]);
        assert_eq!(
            outcome,
            RecordPathOutcome::Dropped(DropCause::PayloadArenaFull)
        );
        // The route was refreshed before the bail; the old announce stays on hand
        // (untouched) until the call site recovers.
        assert_eq!(table.hop_count_to(&dest(1)), Some(2));
        assert_eq!(table.announce_payload(&dest(1)), Some(&[0xAA; 6][..]));
    }
}
