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
        assert!(history.history(3).is_empty()); // unused slot

        // 200 distinct ids on one slot — well past any fixed per-destination cap;
        // a growable history keeps them all (no eviction).
        for n in 0..200u8 {
            assert_eq!(history.remember(0, aid(n)), RememberOutcome::StoredFresh);
        }
        assert_eq!(history.history(0).len(), 200);
        assert!(history.history(0).contains(&aid(0))); // oldest still present

        // Re-hearing a known id is a no-op.
        assert_eq!(history.remember(0, aid(7)), RememberOutcome::AlreadyKnown);
        assert_eq!(history.history(0).len(), 200);

        // A later slot grows on demand, independent of slot 0.
        assert_eq!(history.remember(2, aid(99)), RememberOutcome::StoredFresh);
        assert_eq!(history.history(2).len(), 1);
        assert!(history.history(1).is_empty());
    }
}
