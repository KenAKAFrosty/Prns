use crate::routing::announce::stored::{AnnounceIdHistory, RememberOutcome};
use crate::routing::announce::AnnounceId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedAnnounceIdHistory<const MAX_DESTINATIONS: usize, const MAX_PER_DESTINATION: usize> {
    rows: [[AnnounceId; MAX_PER_DESTINATION]; MAX_DESTINATIONS],
    len: [u8; MAX_DESTINATIONS],
}

impl<const MAX_DESTINATIONS: usize, const MAX_PER_DESTINATION: usize> Default
    for FixedAnnounceIdHistory<MAX_DESTINATIONS, MAX_PER_DESTINATION>
{
    fn default() -> Self {
        let zero = AnnounceId::from_wire([0u8; 10]);
        Self {
            rows: [[zero; MAX_PER_DESTINATION]; MAX_DESTINATIONS],
            len: [0; MAX_DESTINATIONS],
        }
    }
}

impl<const MAX_DESTINATIONS: usize, const MAX_PER_DESTINATION: usize> AnnounceIdHistory
    for FixedAnnounceIdHistory<MAX_DESTINATIONS, MAX_PER_DESTINATION>
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
        let mut store: FixedAnnounceIdHistory<4, 4> = FixedAnnounceIdHistory::default();
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
        let mut store: FixedAnnounceIdHistory<4, 4> = FixedAnnounceIdHistory::default();
        store.remember(0, aid(1));
        assert_eq!(store.remember(0, aid(1)), RememberOutcome::AlreadyKnown);
        assert_eq!(store.history(0), &[aid(1)][..]);
    }

    #[test]
    fn rows_are_independent_per_destination() {
        let mut store: FixedAnnounceIdHistory<3, 4> = FixedAnnounceIdHistory::default();
        store.remember(0, aid(1));
        store.remember(2, aid(9));
        assert_eq!(store.history(0), &[aid(1)][..]);
        assert!(store.history(1).is_empty());
        assert_eq!(store.history(2), &[aid(9)][..]);
    }

    #[test]
    fn swap_remove_moves_the_last_row_into_the_hole() {
        let mut store: FixedAnnounceIdHistory<3, 4> = FixedAnnounceIdHistory::default();
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
    fn removing_the_last_row_just_clears_it() {
        let mut store: FixedAnnounceIdHistory<3, 4> = FixedAnnounceIdHistory::default();
        store.remember(1, aid(1));
        store.swap_remove(1, 1);
        assert!(store.history(1).is_empty());
    }

    #[test]
    fn identical_operation_sequences_yield_byte_identical_stores() {
        fn build() -> FixedAnnounceIdHistory<3, 4> {
            let mut s = FixedAnnounceIdHistory::<3, 4>::default();
            s.remember(0, aid(1));
            s.remember(1, aid(11));
            s.remember(0, aid(2));
            s.remember(0, aid(3));
            s.remember(0, aid(4));
            s.remember(0, aid(5));
            s
        }
        assert_eq!(build(), build());
    }
}
