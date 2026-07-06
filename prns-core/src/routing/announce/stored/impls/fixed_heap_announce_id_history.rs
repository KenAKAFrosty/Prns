use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::boxed::Box;
use allocator_api2::vec::Vec;

use crate::routing::announce::stored::{AnnounceIdHistory, RememberOutcome};
use crate::routing::announce::AnnounceId;

fn filled<T: Clone, A: Allocator>(value: T, len: usize, alloc: A) -> Box<[T], A> {
    let mut column = Vec::with_capacity_in(len, alloc);
    column.resize(len, value);
    column.into_boxed_slice()
}

pub struct FixedHeapAnnounceIdHistory<
    const MAX_DESTINATIONS: usize,
    const MAX_PER_DESTINATION: usize,
    A: Allocator = Global,
> {
    rows: Box<[[AnnounceId; MAX_PER_DESTINATION]], A>,
    len: Box<[u8], A>,
}

impl<const MAX_DESTINATIONS: usize, const MAX_PER_DESTINATION: usize, A: Allocator + Default>
    Default for FixedHeapAnnounceIdHistory<MAX_DESTINATIONS, MAX_PER_DESTINATION, A>
{
    fn default() -> Self {
        let zero = AnnounceId::from_wire([0u8; 10]);
        Self {
            rows: filled([zero; MAX_PER_DESTINATION], MAX_DESTINATIONS, A::default()),
            len: filled(0u8, MAX_DESTINATIONS, A::default()),
        }
    }
}

impl<const MAX_DESTINATIONS: usize, const MAX_PER_DESTINATION: usize, A: Allocator>
    AnnounceIdHistory for FixedHeapAnnounceIdHistory<MAX_DESTINATIONS, MAX_PER_DESTINATION, A>
{
    fn history(&self, slot: usize) -> &[AnnounceId] {
        &self.rows[slot][..self.len[slot] as usize]
    }

    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome {
        const {
            assert!(
                MAX_PER_DESTINATION >= 1,
                "MAX_PER_DESTINATION must be at least 1"
            );
            assert!(
                MAX_PER_DESTINATION <= u8::MAX as usize,
                "MAX_PER_DESTINATION must fit in u8 (len storage)"
            );
        }

        let len = self.len[slot] as usize;
        if self.rows[slot][..len].contains(&id) {
            return RememberOutcome::AlreadyKnown;
        }
        if len < MAX_PER_DESTINATION {
            self.rows[slot][len] = id;
            self.len[slot] = (len + 1) as u8;
            return RememberOutcome::StoredFresh;
        }
        self.rows[slot].copy_within(1.., 0);
        self.rows[slot][MAX_PER_DESTINATION - 1] = id;
        RememberOutcome::StoredEvictingOldest
    }

    fn swap_remove(&mut self, i: usize, last: usize) {
        self.rows[i] = self.rows[last];
        self.len[i] = self.len[last];
        self.len[last] = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aid(byte: u8) -> AnnounceId {
        AnnounceId::from_wire([byte; 10])
    }

    #[test]
    fn fills_a_row_then_evicts_oldest_first() {
        let mut store: FixedHeapAnnounceIdHistory<4, 4> = FixedHeapAnnounceIdHistory::default();
        for id in [1u8, 2, 3, 4] {
            assert_eq!(store.remember(0, aid(id)), RememberOutcome::StoredFresh);
        }
        assert_eq!(
            store.remember(0, aid(5)),
            RememberOutcome::StoredEvictingOldest
        );
        assert_eq!(store.history(0), &[aid(2), aid(3), aid(4), aid(5)][..]);
    }

    #[test]
    fn a_known_id_is_not_stored_twice() {
        let mut store: FixedHeapAnnounceIdHistory<4, 4> = FixedHeapAnnounceIdHistory::default();
        store.remember(0, aid(1));
        assert_eq!(store.remember(0, aid(1)), RememberOutcome::AlreadyKnown);
        assert_eq!(store.history(0), &[aid(1)][..]);
    }

    #[test]
    fn swap_remove_moves_the_last_row_into_the_hole() {
        let mut store: FixedHeapAnnounceIdHistory<3, 4> = FixedHeapAnnounceIdHistory::default();
        store.remember(0, aid(1));
        store.remember(1, aid(10));
        store.remember(2, aid(20));
        store.remember(2, aid(21));

        store.swap_remove(0, 2);

        assert_eq!(store.history(0), &[aid(20), aid(21)][..]);
        assert_eq!(store.history(1), &[aid(10)][..]);
        assert!(store.history(2).is_empty());
    }

    #[test]
    fn carries_the_production_scale_table() {
        type Hist = FixedHeapAnnounceIdHistory<1024, 64>;
        let mut store = Hist::default();
        for slot in 0..1024usize {
            store.remember(slot, aid((slot % 251) as u8));
        }
        assert_eq!(store.history(1023), &[aid((1023 % 251) as u8)][..]);
        assert_eq!(store.history(0), &[aid(0)][..]);
    }
}
