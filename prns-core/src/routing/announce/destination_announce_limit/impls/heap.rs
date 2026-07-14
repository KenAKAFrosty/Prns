use alloc::vec::Vec;

use crate::lemire_index::HeapLemireIndex;
use crate::routing::announce::destination_announce_limit::{
    DestinationAnnounceLimit, DestinationAnnounceLimitAdmission, DestinationAnnounceLimitTable,
};
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapDestinationAnnounceLimitTable {
    destinations: Vec<DestinationHash>,
    entries: Vec<DestinationAnnounceLimit>,
    index: HeapLemireIndex,
}

impl DestinationAnnounceLimitTable for HeapDestinationAnnounceLimitTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index.get(destination, &self.destinations)
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn entries_mut(&mut self) -> &mut [DestinationAnnounceLimit] {
        &mut self.entries
    }

    fn insert(
        &mut self,
        destination: DestinationHash,
        entry: DestinationAnnounceLimit,
    ) -> DestinationAnnounceLimitAdmission {
        let slot = self.destinations.len();
        self.destinations.push(destination);
        self.entries.push(entry);
        self.index.insert(slot, &self.destinations);
        DestinationAnnounceLimitAdmission::Recorded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::InstantMillis;

    fn dest_n(n: u32) -> DestinationHash {
        let key = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&key.to_be_bytes());
        b[8..12].copy_from_slice(&n.to_be_bytes());
        DestinationHash::new(b)
    }

    fn entry_at(ms: u64) -> DestinationAnnounceLimit {
        DestinationAnnounceLimit {
            last_allowed_announce_at: InstantMillis(ms),
            ..DestinationAnnounceLimit::default()
        }
    }

    #[test]
    fn the_index_finds_inserted_destinations_and_misses_absent_ones() {
        let mut table = HeapDestinationAnnounceLimitTable::default();
        assert_eq!(
            table.insert(dest_n(1), entry_at(10)),
            DestinationAnnounceLimitAdmission::Recorded
        );
        assert_eq!(
            table.insert(dest_n(2), entry_at(20)),
            DestinationAnnounceLimitAdmission::Recorded
        );

        assert_eq!(table.index_of(&dest_n(1)), Some(0));
        assert_eq!(table.index_of(&dest_n(2)), Some(1));
        assert_eq!(table.index_of(&dest_n(999)), None);
    }

    #[test]
    fn the_table_grows_unbounded_and_every_row_stays_findable_through_reindexing() {
        let mut table = HeapDestinationAnnounceLimitTable::default();
        for n in 0..2_000u32 {
            assert_eq!(
                table.insert(dest_n(n), entry_at(n as u64)),
                DestinationAnnounceLimitAdmission::Recorded
            );
        }
        assert_eq!(table.len(), 2_000);
        for n in 0..2_000u32 {
            assert_eq!(table.index_of(&dest_n(n)), Some(n as usize));
        }
        assert_eq!(table.index_of(&dest_n(999_999)), None);
    }
}
