//! Heap-backed, growable seen-announce-id history (the typical std/alloc backend).
//!
//! One `Vec<AnnounceId>` per routing slot, grown on demand — no tiers, no eviction,
//! so dedup never forgets. Presented through the shared view as a single run
//! (`from_slices(&ids, &[])`); the two-slice view is the no_std twin's lending
//! shape, which this backend satisfies with everything in the first slice.

use alloc::vec::Vec;

use crate::routing::announce::AnnounceId;
use crate::routing::storage::{AnnounceIdHistory, AnnounceIdHistoryView, RememberOutcome};

#[derive(Debug, Default)]
pub struct HeapAnnounceIdHistory {
    per_slot: Vec<Vec<AnnounceId>>,
}

impl AnnounceIdHistory for HeapAnnounceIdHistory {
    fn history(&self, slot: usize) -> AnnounceIdHistoryView<'_> {
        match self.per_slot.get(slot) {
            Some(ids) => AnnounceIdHistoryView::from_slices(ids, &[]),
            None => AnnounceIdHistoryView::EMPTY,
        }
    }

    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome {
        if self.per_slot.len() <= slot {
            self.per_slot.resize_with(slot + 1, Vec::new);
        }
        let ids = &mut self.per_slot[slot];
        if ids.contains(&id) {
            RememberOutcome::AlreadyKnown
        } else {
            ids.push(id);
            RememberOutcome::StoredFresh
        }
    }

    fn swap_remove(&mut self, i: usize, last: usize) {
        self.per_slot.swap(i, last);
        self.per_slot.truncate(last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aid(byte: u8) -> AnnounceId {
        AnnounceId::from_wire([byte; 10])
    }

    #[test]
    fn grows_per_slot_without_eviction_and_dedups() {
        let mut history = HeapAnnounceIdHistory::default();
        assert!(history.history(3).is_empty());

        for n in 0..200u8 {
            assert_eq!(history.remember(0, aid(n)), RememberOutcome::StoredFresh);
        }
        assert_eq!(history.history(0).len(), 200);
        assert!(history.history(0).contains(&aid(0)));

        assert_eq!(history.remember(0, aid(7)), RememberOutcome::AlreadyKnown);
        assert_eq!(history.history(0).len(), 200);

        assert_eq!(history.remember(2, aid(99)), RememberOutcome::StoredFresh);
        assert_eq!(history.history(2).len(), 1);
        assert!(history.history(1).is_empty());
    }

    #[test]
    fn swap_remove_moves_the_last_slots_ids_into_the_hole() {
        let mut history = HeapAnnounceIdHistory::default();
        history.remember(0, aid(1));
        history.remember(1, aid(10));
        history.remember(2, aid(20));
        history.remember(2, aid(21));

        history.swap_remove(0, 2);

        assert!(history.history(0).contains(&aid(20)));
        assert!(history.history(0).contains(&aid(21)));
        assert!(!history.history(0).contains(&aid(1)));
        assert!(history.history(1).contains(&aid(10)));
        assert!(history.history(2).is_empty());
    }
}
