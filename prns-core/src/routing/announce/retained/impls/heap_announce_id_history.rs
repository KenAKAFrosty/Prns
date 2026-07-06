//! Heap-backed seen-announce-id history: one `Vec<AnnounceId>` per routing slot,
//! oldest-out once a slot reaches the reference's per-destination cap.

use alloc::vec::Vec;

use crate::routing::announce::defaults::MAX_ANNOUNCE_IDS_PER_DESTINATION;
use crate::routing::announce::retained::{AnnounceIdHistory, RememberOutcome};
use crate::routing::announce::AnnounceId;

#[derive(Debug, Default)]
pub struct HeapAnnounceIdHistory {
    per_slot: Vec<Vec<AnnounceId>>,
}

impl AnnounceIdHistory for HeapAnnounceIdHistory {
    fn history(&self, slot: usize) -> &[AnnounceId] {
        match self.per_slot.get(slot) {
            Some(ids) => ids,
            None => &[],
        }
    }

    fn remember(&mut self, slot: usize, id: AnnounceId) -> RememberOutcome {
        if self.per_slot.len() <= slot {
            self.per_slot.resize_with(slot + 1, Vec::new);
        }
        let ids = &mut self.per_slot[slot];
        if ids.contains(&id) {
            RememberOutcome::AlreadyKnown
        } else if ids.len() < MAX_ANNOUNCE_IDS_PER_DESTINATION {
            ids.push(id);
            RememberOutcome::StoredFresh
        } else {
            ids.remove(0);
            ids.push(id);
            RememberOutcome::StoredEvictingOldest
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
    fn caps_each_slot_at_the_reference_depth_evicting_oldest_and_dedups() {
        let mut history = HeapAnnounceIdHistory::default();
        assert!(history.history(3).is_empty());

        for n in 0..MAX_ANNOUNCE_IDS_PER_DESTINATION {
            assert_eq!(
                history.remember(0, aid(n as u8)),
                RememberOutcome::StoredFresh
            );
        }
        assert_eq!(history.remember(0, aid(7)), RememberOutcome::AlreadyKnown);
        assert_eq!(
            history.remember(0, aid(MAX_ANNOUNCE_IDS_PER_DESTINATION as u8)),
            RememberOutcome::StoredEvictingOldest
        );
        assert_eq!(history.history(0).len(), MAX_ANNOUNCE_IDS_PER_DESTINATION);
        assert!(!history.history(0).contains(&aid(0)));
        assert!(history.history(0).contains(&aid(1)));
        assert!(history
            .history(0)
            .contains(&aid(MAX_ANNOUNCE_IDS_PER_DESTINATION as u8)));

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
