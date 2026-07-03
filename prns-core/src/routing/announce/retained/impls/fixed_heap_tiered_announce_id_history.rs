//! The fixed-capacity, heap-backed twin of [`TieredAnnounceIdHistory`]: the floor, overflow
//! arena, and length tables live in a caller-chosen heap region (PSRAM on the S3) via `A`.

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::routing::announce::retained::{
    AnnounceIdHistory, AnnounceIdHistoryView, RememberOutcome,
};
use crate::routing::announce::AnnounceId;

fn filled<T: Clone, A: Allocator>(value: T, len: usize, alloc: A) -> Box<[T], A> {
    let mut column = Vec::with_capacity_in(len, alloc);
    column.resize(len, value);
    column.into_boxed_slice()
}

pub struct FixedHeapTieredAnnounceIdHistory<
    const FLOOR_PER_DESTINATION: usize,
    const OVERFLOW_CAPACITY: usize,
    const MAX_DESTINATIONS: usize,
    const MAX_PER_DESTINATION: usize,
    A: Allocator = Global,
> {
    floor: Box<[[AnnounceId; FLOOR_PER_DESTINATION]], A>,
    floor_len: Box<[u8], A>,
    overflow: Box<[AnnounceId], A>,
    overflow_len: Box<[u8], A>,
}

impl<
        const FLOOR_PER_DESTINATION: usize,
        const OVERFLOW_CAPACITY: usize,
        const MAX_DESTINATIONS: usize,
        const MAX_PER_DESTINATION: usize,
        A: Allocator + Default,
    > Default
    for FixedHeapTieredAnnounceIdHistory<
        FLOOR_PER_DESTINATION,
        OVERFLOW_CAPACITY,
        MAX_DESTINATIONS,
        MAX_PER_DESTINATION,
        A,
    >
{
    fn default() -> Self {
        let zero = AnnounceId::from_wire([0u8; 10]);
        Self {
            floor: filled(
                [zero; FLOOR_PER_DESTINATION],
                MAX_DESTINATIONS,
                A::default(),
            ),
            floor_len: filled(0u8, MAX_DESTINATIONS, A::default()),
            overflow: filled(zero, OVERFLOW_CAPACITY, A::default()),
            overflow_len: filled(0u8, MAX_DESTINATIONS, A::default()),
        }
    }
}

impl<
        const FLOOR_PER_DESTINATION: usize,
        const OVERFLOW_CAPACITY: usize,
        const MAX_DESTINATIONS: usize,
        const MAX_PER_DESTINATION: usize,
        A: Allocator,
    >
    FixedHeapTieredAnnounceIdHistory<
        FLOOR_PER_DESTINATION,
        OVERFLOW_CAPACITY,
        MAX_DESTINATIONS,
        MAX_PER_DESTINATION,
        A,
    >
{
    pub fn history(&self, slot: usize) -> AnnounceIdHistoryView<'_> {
        AnnounceIdHistoryView::from_slices(self.floor_slice(slot), self.overflow_slice(slot))
    }

    pub fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome {
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
        A: Allocator,
    > AnnounceIdHistory
    for FixedHeapTieredAnnounceIdHistory<
        FLOOR_PER_DESTINATION,
        OVERFLOW_CAPACITY,
        MAX_DESTINATIONS,
        MAX_PER_DESTINATION,
        A,
    >
{
    fn history(&self, slot: usize) -> AnnounceIdHistoryView<'_> {
        FixedHeapTieredAnnounceIdHistory::history(self, slot)
    }

    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome {
        FixedHeapTieredAnnounceIdHistory::remember(self, slot, id)
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
        store: &FixedHeapTieredAnnounceIdHistory<F, O, P, MP>,
        slot: usize,
    ) -> (usize, usize) {
        (
            store.floor_len[slot] as usize,
            store.overflow_len[slot] as usize,
        )
    }

    #[test]
    fn first_insert_lands_in_the_floor() {
        let mut store: FixedHeapTieredAnnounceIdHistory<4, 16, 4, 64> =
            FixedHeapTieredAnnounceIdHistory::default();
        assert_eq!(store.remember(0, aid(1)), RememberOutcome::StoredFresh);
        assert_eq!(tier_lens(&store, 0), (1, 0));
        assert!(store.history(0).contains(&aid(1)));
    }

    #[test]
    fn floor_fills_then_overflow_takes_the_rest() {
        let mut store: FixedHeapTieredAnnounceIdHistory<2, 16, 4, 64> =
            FixedHeapTieredAnnounceIdHistory::default();
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
        let mut store: FixedHeapTieredAnnounceIdHistory<1, 8, 3, 64> =
            FixedHeapTieredAnnounceIdHistory::default();
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
    fn per_path_cap_rotates_oldest_with_cross_tier_promotion() {
        let mut store: FixedHeapTieredAnnounceIdHistory<2, 8, 2, 4> =
            FixedHeapTieredAnnounceIdHistory::default();
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
    fn identical_operation_sequences_yield_byte_identical_stores() {
        fn build() -> FixedHeapTieredAnnounceIdHistory<2, 8, 3, 4> {
            let mut s = FixedHeapTieredAnnounceIdHistory::<2, 8, 3, 4>::default();
            s.remember(0, aid(1));
            s.remember(0, aid(2));
            s.remember(1, aid(11));
            s.remember(0, aid(3));
            s.remember(1, aid(12));
            s.remember(0, aid(4));
            s.remember(0, aid(5));
            s
        }
        let a = build();
        let b = build();
        assert_eq!(a.floor, b.floor);
        assert_eq!(a.floor_len, b.floor_len);
        assert_eq!(a.overflow, b.overflow);
        assert_eq!(a.overflow_len, b.overflow_len);
    }

    #[test]
    fn swap_remove_repacks_overflow_when_the_middle_carries_some() {
        let mut store: FixedHeapTieredAnnounceIdHistory<2, 16, 4, 64> =
            FixedHeapTieredAnnounceIdHistory::default();
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
    fn the_bulk_floor_carries_a_large_table() {
        type Hist2048 = FixedHeapTieredAnnounceIdHistory<4, 256, 2048, 32>;
        let mut store = Hist2048::default();
        for slot in 0..2048usize {
            store.remember(slot, aid((slot % 251) as u8));
        }
        assert!(store.history(2047).contains(&aid((2047 % 251) as u8)));
        assert!(store.history(0).contains(&aid(0)));
    }
}
