use crate::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};

pub type PathRequestIdBytes = [u8; TRUNCATED_HASH_BYTE_LEN];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRequestNovelty {
    Fresh,
    Duplicate,
}

pub trait SeenPathRequestTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn ids(&self) -> &[PathRequestIdBytes];

    /// Record one `(destination, id)`, evicting the oldest when full (FIFO).
    fn remember(&mut self, destination: DestinationHash, id: PathRequestIdBytes);
}

#[derive(Debug, Default)]
pub struct SeenPathRequests<C: SeenPathRequestTable> {
    table: C,
}

impl<C: SeenPathRequestTable> SeenPathRequests<C> {
    pub fn observe(
        &mut self,
        destination: DestinationHash,
        id: PathRequestIdBytes,
    ) -> PathRequestNovelty {
        let seen = self
            .table
            .destinations()
            .iter()
            .zip(self.table.ids())
            .any(|(candidate, candidate_id)| *candidate == destination && *candidate_id == id);
        if seen {
            return PathRequestNovelty::Duplicate;
        }
        self.table.remember(destination, id);
        PathRequestNovelty::Fresh
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    #[test]
    fn a_first_sighting_is_fresh_and_a_repeat_is_a_duplicate() {
        let mut seen: SeenPathRequests<FixedSeenPathRequestTable<4>> = SeenPathRequests::default();
        assert_eq!(seen.observe(dest(1), [0xAA; 16]), PathRequestNovelty::Fresh);
        assert_eq!(
            seen.observe(dest(1), [0xAA; 16]),
            PathRequestNovelty::Duplicate,
        );
    }

    #[test]
    fn a_different_id_for_the_same_destination_is_fresh() {
        let mut seen: SeenPathRequests<FixedSeenPathRequestTable<4>> = SeenPathRequests::default();
        assert_eq!(seen.observe(dest(1), [0xAA; 16]), PathRequestNovelty::Fresh);
        assert_eq!(seen.observe(dest(1), [0xBB; 16]), PathRequestNovelty::Fresh);
        assert_eq!(seen.observe(dest(2), [0xAA; 16]), PathRequestNovelty::Fresh);
    }

    #[test]
    fn the_oldest_id_ages_out_when_the_ring_fills() {
        let mut seen: SeenPathRequests<FixedSeenPathRequestTable<2>> = SeenPathRequests::default();
        assert_eq!(seen.observe(dest(1), [1; 16]), PathRequestNovelty::Fresh);
        assert_eq!(seen.observe(dest(2), [2; 16]), PathRequestNovelty::Fresh);
        assert_eq!(seen.observe(dest(3), [3; 16]), PathRequestNovelty::Fresh);
        assert_eq!(seen.len(), 2);
        assert_eq!(seen.observe(dest(1), [1; 16]), PathRequestNovelty::Fresh);
        assert_eq!(
            seen.observe(dest(3), [3; 16]),
            PathRequestNovelty::Duplicate
        );
    }

    #[test]
    fn at_capacity_zero_nothing_is_ever_remembered() {
        let mut seen: SeenPathRequests<FixedSeenPathRequestTable<0>> = SeenPathRequests::default();
        assert_eq!(seen.observe(dest(1), [1; 16]), PathRequestNovelty::Fresh);
        assert_eq!(seen.observe(dest(1), [1; 16]), PathRequestNovelty::Fresh);
        assert!(seen.is_empty());
    }

    #[test]
    fn heap_columns_dedup_past_any_fixed_ceiling() {
        let mut seen: SeenPathRequests<HeapSeenPathRequestTable> = SeenPathRequests::default();
        for n in 0..64u8 {
            assert_eq!(seen.observe(dest(n), [n; 16]), PathRequestNovelty::Fresh);
        }
        assert_eq!(seen.len(), 64);
        assert_eq!(
            seen.observe(dest(17), [17; 16]),
            PathRequestNovelty::Duplicate
        );
    }
}
