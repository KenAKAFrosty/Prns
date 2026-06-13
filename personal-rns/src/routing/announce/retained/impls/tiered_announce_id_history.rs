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
//! ## Determinism
//!
//! `PartialEq` is structural: the arrays compare byte-for-byte, including the
//! stale tail past `overflow_used` that prior shifts left behind. Two stores
//! built by identical op sequences yield byte-identical bytes, so `==` is the
//! determinism check. Like `PackedAppDataArena`, this is **not** "same set of ids".
//! Comparing two stores built by different routes is a misuse.

use crate::routing::announce::AnnounceId;
use crate::routing::announce::retained::{AnnounceIdHistory, AnnounceIdHistoryView, RememberOutcome};

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

        if floor_len < FLOOR_PER_DESTINATION && total < MAX_PER_DESTINATION {
            self.floor[slot][floor_len] = id;
            self.floor_len[slot] = (floor_len + 1) as u8;
            return RememberOutcome::StoredFresh;
        }

        let used = self.overflow_used();
        if total < MAX_PER_DESTINATION && used < OVERFLOW_CAPACITY {
            let path_offset = self.overflow_offset(slot);
            let insert_at = path_offset + overflow_len;
            self.overflow.copy_within(insert_at..used, insert_at + 1);
            self.overflow[insert_at] = id;
            self.overflow_len[slot] = (overflow_len + 1) as u8;
            return RememberOutcome::StoredFresh;
        }

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
        TieredAnnounceIdHistory::history(self, slot)
    }

    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome {
        TieredAnnounceIdHistory::remember(self, slot, id)
    }

    fn swap_remove(&mut self, i: usize, last: usize) {
        if i == last {
            self.floor_len[i] = 0;
            self.overflow_len[i] = 0;
            return;
        }

        let last_floor = self.floor[last];
        let last_floor_len = self.floor_len[last];
        let last_ovlen = self.overflow_len[last] as usize;
        let last_off = self.overflow_offset(last);
        let mut moved = [AnnounceId::from_wire([0u8; 10]); MAX_PER_DESTINATION];
        moved[..last_ovlen].copy_from_slice(&self.overflow[last_off..last_off + last_ovlen]);

        let off_i = self.overflow_offset(i);
        let ovlen_i = self.overflow_len[i] as usize;
        self.overflow
            .copy_within(off_i + ovlen_i..last_off, off_i + last_ovlen);
        self.overflow[off_i..off_i + last_ovlen].copy_from_slice(&moved[..last_ovlen]);
        self.overflow_len[i] = last_ovlen as u8;

        self.floor[i] = last_floor;
        self.floor_len[i] = last_floor_len;
        self.floor_len[last] = 0;
        self.overflow_len[last] = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aid(byte: u8) -> AnnounceId {
        AnnounceId::from_wire([byte; 10])
    }

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
        store.remember(0, aid(1));
        store.remember(0, aid(2));
        assert_eq!(tier_lens(&store, 0), (2, 0));
        store.remember(0, aid(3));
        store.remember(0, aid(4));
        assert_eq!(tier_lens(&store, 0), (2, 2));
        let view = store.history(0);
        assert_eq!(view.len(), 4);
        let collected: heapless::Vec<AnnounceId, 4> = view.iter().copied().collect();
        assert_eq!(collected.as_slice(), &[aid(1), aid(2), aid(3), aid(4)][..]);
    }

    #[test]
    fn multiple_paths_pack_overflow_contiguously() {
        let mut store: TieredAnnounceIdHistory<1, 8, 3, 64> = TieredAnnounceIdHistory::new();
        for slot in 0..3 {
            store.remember(slot, aid(10 * slot as u8 + 1));
            store.remember(slot, aid(10 * slot as u8 + 2));
            store.remember(slot, aid(10 * slot as u8 + 3));
        }
        for slot in 0..3 {
            assert_eq!(store.floor_slice(slot), &[aid(10 * slot as u8 + 1)][..]);
            assert_eq!(
                store.overflow_slice(slot),
                &[aid(10 * slot as u8 + 2), aid(10 * slot as u8 + 3)][..]
            );
        }
        assert_eq!(store.overflow_used(), 6);
    }

    #[test]
    fn growing_one_paths_overflow_does_not_disturb_anothers_view() {
        let mut store: TieredAnnounceIdHistory<1, 8, 3, 64> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1));
        store.remember(0, aid(2));
        store.remember(1, aid(11));
        store.remember(1, aid(12));
        store.remember(0, aid(3));
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
        let mut store: TieredAnnounceIdHistory<2, 8, 2, 4> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1));
        store.remember(0, aid(2));
        store.remember(0, aid(3));
        store.remember(0, aid(4));
        assert_eq!(
            store.remember(0, aid(5)),
            RememberOutcome::StoredEvictingOldest
        );
        assert_eq!(store.floor_slice(0), &[aid(2), aid(3)][..]);
        assert_eq!(store.overflow_slice(0), &[aid(4), aid(5)][..]);
        let view = store.history(0);
        assert!(!view.contains(&aid(1)));
        assert!(view.contains(&aid(5)));
    }

    #[test]
    fn arena_exhausted_rotates_within_floor_when_path_has_no_overflow() {
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
        let mut store: TieredAnnounceIdHistory<1, 2, 1, 64> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1));
        store.remember(0, aid(2));
        store.remember(0, aid(3));
        assert_eq!(
            store.remember(0, aid(4)),
            RememberOutcome::StoredEvictingOldest
        );
        assert_eq!(store.floor_slice(0), &[aid(2)][..]);
        assert_eq!(store.overflow_slice(0), &[aid(3), aid(4)][..]);
        assert!(!store.history(0).contains(&aid(1)));
    }

    #[test]
    fn floor_equal_to_per_path_cap_reverts_to_per_path_full_allocation() {
        let mut store: TieredAnnounceIdHistory<3, 0, 1, 3> = TieredAnnounceIdHistory::new();
        for n in 0..5u8 {
            store.remember(0, aid(n));
        }
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
            s.remember(0, aid(5));
            s
        }
        assert_eq!(build(), build());
    }

    #[test]
    fn max_per_path_below_floor_rotates_visible_prefix() {
        let mut store: TieredAnnounceIdHistory<4, 8, 1, 2> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1));
        store.remember(0, aid(2));
        assert_eq!(tier_lens(&store, 0), (2, 0));
        assert_eq!(
            store.remember(0, aid(3)),
            RememberOutcome::StoredEvictingOldest
        );
        assert_eq!(store.floor_slice(0), &[aid(2), aid(3)][..]);
        let view = store.history(0);
        assert_eq!(view.len(), 2);
        assert!(view.contains(&aid(3)));
        assert!(!view.contains(&aid(1)));
    }

    #[test]
    fn view_is_empty_for_an_unused_slot() {
        let store: TieredAnnounceIdHistory<4, 16, 4, 64> = TieredAnnounceIdHistory::new();
        assert!(store.history(2).is_empty());
        assert_eq!(store.history(2).len(), 0);
    }

    #[test]
    fn swap_remove_moves_a_floor_only_slot_into_the_hole() {
        let mut store: TieredAnnounceIdHistory<2, 16, 4, 64> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1));
        store.remember(1, aid(10));
        store.remember(2, aid(20));
        store.remember(2, aid(21));

        store.swap_remove(0, 2);

        assert!(store.history(0).contains(&aid(20)));
        assert!(store.history(0).contains(&aid(21)));
        assert!(!store.history(0).contains(&aid(1)));
        assert!(store.history(1).contains(&aid(10)));
        assert_eq!(tier_lens(&store, 0), (2, 0));
        assert_eq!(tier_lens(&store, 1), (1, 0));
    }

    #[test]
    fn swap_remove_repacks_overflow_when_the_middle_carries_some() {
        let mut store: TieredAnnounceIdHistory<2, 16, 4, 64> = TieredAnnounceIdHistory::new();
        for id in [1u8, 2, 3] {
            store.remember(0, aid(id));
        }
        for id in [10u8, 11, 12] {
            store.remember(1, aid(id));
        }
        for id in [20u8, 21, 22, 23] {
            store.remember(2, aid(id));
        }

        store.swap_remove(0, 2);

        for id in [20u8, 21, 22, 23] {
            assert!(store.history(0).contains(&aid(id)));
        }
        assert!(!store.history(0).contains(&aid(1)));
        assert_eq!(tier_lens(&store, 0), (2, 2));
        for id in [10u8, 11, 12] {
            assert!(
                store.history(1).contains(&aid(id)),
                "the middle slot's overflow must survive the repack"
            );
        }
        assert_eq!(tier_lens(&store, 1), (2, 1));
    }

    #[test]
    fn swap_remove_of_the_last_slot_just_clears_it() {
        let mut store: TieredAnnounceIdHistory<2, 16, 4, 64> = TieredAnnounceIdHistory::new();
        store.remember(0, aid(1));
        for id in [10u8, 11, 12] {
            store.remember(1, aid(id));
        }

        store.swap_remove(1, 1);

        assert_eq!(tier_lens(&store, 1), (0, 0));
        assert!(!store.history(1).contains(&aid(10)));
        assert!(store.history(0).contains(&aid(1)));
    }
}
