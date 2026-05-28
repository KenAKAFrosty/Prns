//! A two-tier seen-announce-id set: an inline floor that guarantees every path
//! a small handful of slots, plus a shared overflow arena for bonus capacity.
//!
//! The routing table needs RNS's `MAX_RANDOM_BLOBS` (64) per destination to defend
//! against blob-replay across multipath, but most destinations use far fewer in
//! steady state, and per-destination worst-case allocation pays the maximum
//! for every path whether it earns it or not (64 × 10 B × 32 paths ≈ 20 KB,
//! always). A small per-destination inline floor (covers typical multipath fan-in,
//! always paid) plus a shared overflow arena (bonus capacity, packed) cuts the
//! footprint by ~5× while keeping the same per-destination cap.
//!
//! ## Tiers
//!
//! 1. **Floor (per-destination inline, guaranteed).** Every path gets `FLOOR_PER_DESTINATION`
//!    inline slots regardless of arena pressure. This covers the multipath dedup
//!    case for paths typical mesh deployments see (interfaces × routes).
//! 2. **Overflow (shared arena, packed).** `OVERFLOW_CAPACITY` ids total,
//!    backing every path past its floor up to the per-destination cap. Each path's
//!    overflow ids live in one contiguous span; offsets are derived from the
//!    prefix sum of per-destination lengths, not stored. Costs one `u8` per destination of
//!    bookkeeping (vs eight bytes for a `{offset, len}` pair on a 32-bit
//!    target), which keeps the metadata-to-payload ratio sane
//! 3. **Per-path cap (`MAX_PER_DESTINATION`).** Mirrors RNS's `MAX_RANDOM_BLOBS`.
//!    Const generic owned by the store, not passed by callers. Keeps the
//!    cap as a structural property of the type rather than something the
//!    consumer has to remember to pass on every call.
//!
//! ## Eviction: one rotate path
//!
//! When a `remember` would exceed either the per-destination cap *or* the shared
//! arena, we don't drop — we rotate the oldest out. The oldest is `floor[0]`
//! (chronologically, the floor fills before overflow). Floor shifts left by 1,
//! `overflow[0]` for this path promotes to `floor[floor_len - 1]` if the
//! path has any overflow, then this path's overflow slice rotates in place
//! (its length unchanged → no other path's span moves). Both the per-destination-cap
//! case and the arena-exhausted case collapse to the same code path, and there
//! is no `Dropped` outcome.
//!
//! ## Determinism
//!
//! `PartialEq` is structural: the arrays compare byte-for-byte, including the
//! stale tail past `overflow_used` that prior shifts left behind. Two stores
//! built by identical op sequences yield byte-identical bytes, so `==` is the
//! determinism check. Like `PackedAppDataArena`, this is **not** "same set of ids".
//! Comparing two stores built by different routes is a misuse.

use crate::routing::announce::AnnounceId;
use crate::routing::storage::{AnnounceIdHistory, AnnounceIdHistoryView, RememberOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TieredAnnounceIdHistory<
    const FLOOR_PER_DESTINATION: usize,
    const OVERFLOW_CAPACITY: usize,
    const MAX_DESTINATIONS: usize,
    const MAX_PER_DESTINATION: usize,
> {
    floor: [[AnnounceId; FLOOR_PER_DESTINATION]; MAX_DESTINATIONS],
    floor_len: [u8; MAX_DESTINATIONS],
    overflow: [AnnounceId; OVERFLOW_CAPACITY],
    /// Per-path overflow length. Path `i`'s overflow slice lives at
    /// `overflow[sum(overflow_len[..i]) .. sum(overflow_len[..i]) + overflow_len[i]]`,
    /// so offsets are derived from the prefix sum rather than stored.
    overflow_len: [u8; MAX_DESTINATIONS],
}

impl<
        const FLOOR_PER_DESTINATION: usize,
        const OVERFLOW_CAPACITY: usize,
        const MAX_DESTINATIONS: usize,
        const MAX_PER_DESTINATION: usize,
    > Default
    for TieredAnnounceIdHistory<
        FLOOR_PER_DESTINATION,
        OVERFLOW_CAPACITY,
        MAX_DESTINATIONS,
        MAX_PER_DESTINATION,
    >
{
    fn default() -> Self {
        let zero = AnnounceId::from_wire([0u8; 10]);
        Self {
            floor: [[zero; FLOOR_PER_DESTINATION]; MAX_DESTINATIONS],
            floor_len: [0u8; MAX_DESTINATIONS],
            overflow: [zero; OVERFLOW_CAPACITY],
            overflow_len: [0u8; MAX_DESTINATIONS],
        }
    }
}

impl<
        const FLOOR_PER_DESTINATION: usize,
        const OVERFLOW_CAPACITY: usize,
        const MAX_DESTINATIONS: usize,
        const MAX_PER_DESTINATION: usize,
    >
    TieredAnnounceIdHistory<
        FLOOR_PER_DESTINATION,
        OVERFLOW_CAPACITY,
        MAX_DESTINATIONS,
        MAX_PER_DESTINATION,
    >
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn history(&self, slot: usize) -> AnnounceIdHistoryView<'_> {
        AnnounceIdHistoryView::from_slices(self.floor_slice(slot), self.overflow_slice(slot))
    }

    /// Record `id` as recently seen for path `slot`. Re-hearing a known id is a
    /// no-op (no promotion). Inserts go to the floor first, then the shared
    /// overflow; when the path is at `MAX_PER_DESTINATION` or the arena is exhausted,
    /// the oldest id rotates out and the new one takes its place at the end.
    /// Never drops.
    pub fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome {
        // The rotate path assumes `floor_len >= 1` at Tier 3; that follows from
        // `FLOOR_PER_DESTINATION >= 1` (the floor receives the first insert via Tier 1)
        // and `MAX_PER_DESTINATION >= 1`. The u8 length fields cap floor lengths at
        // 255 and overflow per-destination lengths at `MAX_PER_DESTINATION - FLOOR_PER_DESTINATION`.
        const {
            assert!(
                FLOOR_PER_DESTINATION >= 1,
                "FLOOR_PER_DESTINATION must be at least 1"
            );
            assert!(
                MAX_PER_DESTINATION >= 1,
                "MAX_PER_DESTINATION must be at least 1"
            );
            assert!(
                FLOOR_PER_DESTINATION <= u8::MAX as usize,
                "FLOOR_PER_DESTINATION must fit in u8 (floor_len storage)"
            );
            assert!(
                MAX_PER_DESTINATION.saturating_sub(FLOOR_PER_DESTINATION) <= u8::MAX as usize,
                "MAX_PER_DESTINATION - FLOOR_PER_DESTINATION must fit in u8 (overflow_len storage)"
            );
        }

        if self.contains(slot, &id) {
            return RememberOutcome::AlreadyKnown;
        }

        let floor_len = self.floor_len[slot] as usize;
        let overflow_len = self.overflow_len[slot] as usize;
        let total = floor_len + overflow_len;

        // Tier 1: floor has room and the per-destination cap isn't reached.
        if floor_len < FLOOR_PER_DESTINATION && total < MAX_PER_DESTINATION {
            self.floor[slot][floor_len] = id;
            self.floor_len[slot] = (floor_len + 1) as u8;
            return RememberOutcome::StoredFresh;
        }

        // Tier 2: floor full (or cap below floor), per-destination cap not reached,
        // arena has room. Insert into this path's overflow span; shift the
        // arena tail right by 1 to keep all spans packed.
        let used = self.overflow_used();
        if total < MAX_PER_DESTINATION && used < OVERFLOW_CAPACITY {
            let path_offset = self.overflow_offset(slot);
            let insert_at = path_offset + overflow_len;
            self.overflow.copy_within(insert_at..used, insert_at + 1);
            self.overflow[insert_at] = id;
            self.overflow_len[slot] = (overflow_len + 1) as u8;
            return RememberOutcome::StoredFresh;
        }

        // Tier 3: at per-destination cap, or arena exhausted. Rotate the oldest out.
        // floor[0] is the oldest (the floor fills first). Shift the LIVE
        // prefix left by 1, then place either the promoted overflow[0] (if
        // any) or the new id into floor[floor_len - 1]. The overflow contents
        // rotate in place; the path's overflow length is unchanged, so no
        // other path's offset moves.
        //
        // The shift is bounded by `floor_len`, not `FLOOR_PER_DESTINATION`, so the
        // visible-slice invariant holds even when `MAX_PER_DESTINATION < FLOOR_PER_DESTINATION`
        // (the path fills its live prefix and rotates inside it).
        self.floor[slot].copy_within(1..floor_len, 0);
        if overflow_len > 0 {
            let path_offset = self.overflow_offset(slot);
            self.floor[slot][floor_len - 1] = self.overflow[path_offset];
            self.overflow
                .copy_within(path_offset + 1..path_offset + overflow_len, path_offset);
            self.overflow[path_offset + overflow_len - 1] = id;
        } else {
            // No overflow for this slot: rotation stays within the live floor.
            self.floor[slot][floor_len - 1] = id;
        }
        RememberOutcome::StoredEvictingOldest
    }

    fn floor_slice(&self, slot: usize) -> &[AnnounceId] {
        &self.floor[slot][..self.floor_len[slot] as usize]
    }

    fn overflow_slice(&self, slot: usize) -> &[AnnounceId] {
        let offset = self.overflow_offset(slot);
        let len = self.overflow_len[slot] as usize;
        &self.overflow[offset..offset + len]
    }

    /// Where in `overflow` path `slot`'s span begins — the prefix sum of every
    /// earlier path's overflow length. O(slot), trivial for `MAX_DESTINATIONS` ≤ ~64.
    fn overflow_offset(&self, slot: usize) -> usize {
        self.overflow_len[..slot].iter().map(|&n| n as usize).sum()
    }

    fn overflow_used(&self) -> usize {
        self.overflow_len.iter().map(|&n| n as usize).sum()
    }

    fn contains(&self, slot: usize, id: &AnnounceId) -> bool {
        self.floor_slice(slot).contains(id) || self.overflow_slice(slot).contains(id)
    }
}

impl<
        const FLOOR_PER_DESTINATION: usize,
        const OVERFLOW_CAPACITY: usize,
        const MAX_DESTINATIONS: usize,
        const MAX_PER_DESTINATION: usize,
    > AnnounceIdHistory
    for TieredAnnounceIdHistory<
        FLOOR_PER_DESTINATION,
        OVERFLOW_CAPACITY,
        MAX_DESTINATIONS,
        MAX_PER_DESTINATION,
    >
{
    fn history(&self, slot: usize) -> AnnounceIdHistoryView<'_> {
        // Inherent method takes precedence in method-call notation; the trait
        // impl forwards to it (the inherent method holds the actual logic).
        TieredAnnounceIdHistory::history(self, slot)
    }

    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome {
        TieredAnnounceIdHistory::remember(self, slot, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aid(byte: u8) -> AnnounceId {
        AnnounceId::from_wire([byte; 10])
    }

    /// Convenience: floor + overflow lengths describe the per-destination footprint;
    /// asserting both keeps the tier assignment honest.
    fn tier_lens<const F: usize, const O: usize, const P: usize, const MP: usize>(
        store: &TieredAnnounceIdHistory<F, O, P, MP>,
        slot: usize,
    ) -> (usize, usize) {
        (
            store.floor_len[slot] as usize,
            store.overflow_len[slot] as usize,
        )
    }

    #[test]
    fn first_insert_lands_in_the_floor() {
        let mut store: TieredAnnounceIdHistory<4, 16, 4, 64> = TieredAnnounceIdHistory::new();
        assert_eq!(store.remember(0, aid(1)), RememberOutcome::StoredFresh);
        assert_eq!(tier_lens(&store, 0), (1, 0));
        assert!(store.history(0).contains(&aid(1)));
    }

    #[test]
    fn re_remembering_a_known_id_is_a_no_op() {
        let mut store: TieredAnnounceIdHistory<4, 16, 4, 64> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1));
        assert_eq!(store.remember(0, aid(1)), RememberOutcome::AlreadyKnown);
        assert_eq!(tier_lens(&store, 0), (1, 0));
    }

    #[test]
    fn floor_fills_then_overflow_takes_the_rest() {
        let mut store: TieredAnnounceIdHistory<2, 16, 4, 64> = TieredAnnounceIdHistory::new();
        // Fill floor (2 slots).
        store.remember(0, aid(1));
        store.remember(0, aid(2));
        assert_eq!(tier_lens(&store, 0), (2, 0));
        // Next two land in overflow.
        store.remember(0, aid(3));
        store.remember(0, aid(4));
        assert_eq!(tier_lens(&store, 0), (2, 2));
        // View walks floor first, then overflow, preserving insertion order.
        let view = store.history(0);
        assert_eq!(view.len(), 4);
        let collected: heapless::Vec<AnnounceId, 4> = view.iter().copied().collect();
        assert_eq!(collected.as_slice(), &[aid(1), aid(2), aid(3), aid(4)][..]);
    }

    #[test]
    fn multiple_paths_pack_overflow_contiguously() {
        let mut store: TieredAnnounceIdHistory<1, 8, 3, 64> = TieredAnnounceIdHistory::new();
        // Each path: one in floor, two in overflow → arena holds 6 ids tightly.
        for slot in 0..3 {
            store.remember(slot, aid(10 * slot as u8 + 1));
            store.remember(slot, aid(10 * slot as u8 + 2));
            store.remember(slot, aid(10 * slot as u8 + 3));
        }
        // Path 0: floor[1], overflow[2,3]. Path 1: floor[11], overflow[12,13]. Etc.
        for slot in 0..3 {
            assert_eq!(store.floor_slice(slot), &[aid(10 * slot as u8 + 1)][..]);
            assert_eq!(
                store.overflow_slice(slot),
                &[aid(10 * slot as u8 + 2), aid(10 * slot as u8 + 3)][..]
            );
        }
        // Overflow is packed in path order.
        assert_eq!(store.overflow_used(), 6);
    }

    #[test]
    fn growing_one_paths_overflow_does_not_disturb_anothers_view() {
        let mut store: TieredAnnounceIdHistory<1, 8, 3, 64> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1)); // floor
        store.remember(0, aid(2)); // overflow
        store.remember(1, aid(11)); // floor
        store.remember(1, aid(12)); // overflow (after path 0's span)
        store.remember(0, aid(3)); // grows path 0's overflow → shifts path 1's
        assert_eq!(
            store.overflow_slice(0),
            &[aid(2), aid(3)][..],
            "path 0's overflow grew correctly",
        );
        assert_eq!(
            store.overflow_slice(1),
            &[aid(12)][..],
            "path 1's view is intact after the shift",
        );
    }

    #[test]
    fn per_path_cap_rotates_oldest_with_cross_tier_promotion() {
        // floor=2, overflow=8, per-destination cap=4. Once the path has 4 ids (2 floor
        // + 2 overflow), the next insert evicts floor[0], promotes overflow[0]
        // to floor's last slot, rotates the overflow slice, appends new at end.
        let mut store: TieredAnnounceIdHistory<2, 8, 2, 4> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1)); // floor[0]
        store.remember(0, aid(2)); // floor[1]
        store.remember(0, aid(3)); // overflow[0]
        store.remember(0, aid(4)); // overflow[1] — at cap now
        assert_eq!(
            store.remember(0, aid(5)),
            RememberOutcome::StoredEvictingOldest
        );
        // Floor: [aid(2), aid(3)] (1 evicted; 3 promoted from overflow[0])
        // Overflow: [aid(4), aid(5)] (in-place rotated)
        assert_eq!(store.floor_slice(0), &[aid(2), aid(3)][..]);
        assert_eq!(store.overflow_slice(0), &[aid(4), aid(5)][..]);
        let view = store.history(0);
        assert!(!view.contains(&aid(1)));
        assert!(view.contains(&aid(5)));
    }

    #[test]
    fn arena_exhausted_rotates_within_floor_when_path_has_no_overflow() {
        // floor=3, overflow=0 (degenerate). Once floor fills, every new id
        // rotates within floor — no overflow ever exists for this path.
        let mut store: TieredAnnounceIdHistory<3, 0, 1, 64> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1));
        store.remember(0, aid(2));
        store.remember(0, aid(3));
        assert_eq!(
            store.remember(0, aid(4)),
            RememberOutcome::StoredEvictingOldest
        );
        assert_eq!(store.floor_slice(0), &[aid(2), aid(3), aid(4)][..]);
        assert!(!store.history(0).contains(&aid(1)));
    }

    #[test]
    fn arena_exhausted_rotates_with_promotion_when_path_has_overflow() {
        // floor=1, overflow=2 (tiny shared budget). One path fills it: floor[1]
        // + overflow[2,3]. Arena now full. Next insert can't grow overflow
        // (arena exhausted) and not at per-destination cap (3 < 64), so it falls into
        // the rotate-with-promotion path.
        let mut store: TieredAnnounceIdHistory<1, 2, 1, 64> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1)); // floor[0]
        store.remember(0, aid(2)); // overflow[0]
        store.remember(0, aid(3)); // overflow[1] — arena full now
        assert_eq!(
            store.remember(0, aid(4)),
            RememberOutcome::StoredEvictingOldest
        );
        // floor[0] = old floor's only slot was aid(1); evict it. Promote
        // overflow[0] = aid(2) into floor. Overflow rotates: was [aid(2),
        // aid(3)], becomes [aid(3), aid(4)].
        assert_eq!(store.floor_slice(0), &[aid(2)][..]);
        assert_eq!(store.overflow_slice(0), &[aid(3), aid(4)][..]);
        assert!(!store.history(0).contains(&aid(1)));
    }

    #[test]
    fn floor_equal_to_per_path_cap_reverts_to_per_path_full_allocation() {
        // Abundance config: floor == per-destination cap, so overflow is vestigial.
        // Every insert lands in floor; once full, rotates within floor.
        let mut store: TieredAnnounceIdHistory<3, 0, 1, 3> = TieredAnnounceIdHistory::new();
        for n in 0..5u8 {
            store.remember(0, aid(n));
        }
        // Floor cap 3: kept the last 3 (aid(2), aid(3), aid(4)).
        assert_eq!(store.floor_slice(0), &[aid(2), aid(3), aid(4)][..]);
        assert_eq!(store.overflow_slice(0), &[] as &[AnnounceId]);
        let view = store.history(0);
        assert!(!view.contains(&aid(0)));
        assert!(!view.contains(&aid(1)));
    }

    #[test]
    fn identical_operation_sequences_yield_byte_identical_stores() {
        fn build() -> TieredAnnounceIdHistory<2, 8, 3, 4> {
            let mut s = TieredAnnounceIdHistory::<2, 8, 3, 4>::new();
            s.remember(0, aid(1));
            s.remember(0, aid(2));
            s.remember(1, aid(11));
            s.remember(0, aid(3));
            s.remember(1, aid(12));
            s.remember(0, aid(4));
            s.remember(0, aid(5)); // triggers rotate
            s
        }
        assert_eq!(build(), build());
    }

    #[test]
    fn max_per_path_below_floor_rotates_visible_prefix() {
        // FLOOR=4, MAX_PER_DESTINATION=2: the path fills the live prefix to 2 ids,
        // then every subsequent insert must rotate *within that visible slice*
        // (not over the full floor capacity). Otherwise the new id would land
        // at floor[3] while the view only exposes floor[..2].
        let mut store: TieredAnnounceIdHistory<4, 8, 1, 2> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1));
        store.remember(0, aid(2));
        assert_eq!(tier_lens(&store, 0), (2, 0));
        // At the cap. Next insert must rotate inside the live prefix.
        assert_eq!(
            store.remember(0, aid(3)),
            RememberOutcome::StoredEvictingOldest
        );
        assert_eq!(store.floor_slice(0), &[aid(2), aid(3)][..]);
        let view = store.history(0);
        assert_eq!(view.len(), 2);
        assert!(view.contains(&aid(3))); // newly remembered id is visible
        assert!(!view.contains(&aid(1))); // oldest was evicted
    }

    #[test]
    fn view_is_empty_for_an_unused_slot() {
        let store: TieredAnnounceIdHistory<4, 16, 4, 64> = TieredAnnounceIdHistory::new();
        assert!(store.history(2).is_empty());
        assert_eq!(store.history(2).len(), 0);
    }
}
