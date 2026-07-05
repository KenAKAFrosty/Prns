use alloc::vec::Vec;

use crate::routing::announce::rate_limit::{
    AnnounceRateColumns, AnnounceRateEntry, RateEntryAdmission,
};
use crate::wire::DestinationHash;

const EMPTY: usize = usize::MAX;
const MIN_BUCKETS: usize = 8;

#[derive(Debug)]
pub struct HeapAnnounceRateColumns {
    destinations: Vec<DestinationHash>,
    entries: Vec<AnnounceRateEntry>,
    index: Vec<usize>,
}

impl Default for HeapAnnounceRateColumns {
    fn default() -> Self {
        let mut index = Vec::new();
        index.resize(MIN_BUCKETS, EMPTY);
        Self {
            destinations: Vec::new(),
            entries: Vec::new(),
            index,
        }
    }
}

impl HeapAnnounceRateColumns {
    fn key(destination: &DestinationHash) -> u64 {
        let b = destination.as_bytes();
        u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
    }

    fn bucket(&self, key: u64) -> usize {
        ((key as u128 * self.index.len() as u128) >> u64::BITS) as usize
    }

    fn index_position(&self, destination: &DestinationHash) -> Option<usize> {
        let n = self.index.len();
        let mut pos = self.bucket(Self::key(destination));
        loop {
            let slot = self.index[pos];
            if slot == EMPTY {
                return None;
            }
            if self.destinations[slot] == *destination {
                return Some(pos);
            }
            pos = (pos + 1) % n;
        }
    }

    fn index_insert(&mut self, slot: usize) {
        let n = self.index.len();
        let mut pos = self.bucket(Self::key(&self.destinations[slot]));
        while self.index[pos] != EMPTY {
            pos = (pos + 1) % n;
        }
        self.index[pos] = slot;
    }

    fn grow_index_if_loaded(&mut self) {
        if (self.destinations.len() + 1) * 3 > self.index.len() * 2 {
            let new_buckets = self.index.len() * 2;
            self.index.clear();
            self.index.resize(new_buckets, EMPTY);
            for slot in 0..self.destinations.len() {
                self.index_insert(slot);
            }
        }
    }
}

impl AnnounceRateColumns for HeapAnnounceRateColumns {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index_position(destination).map(|pos| self.index[pos])
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn entries_mut(&mut self) -> &mut [AnnounceRateEntry] {
        &mut self.entries
    }

    fn insert(
        &mut self,
        destination: DestinationHash,
        entry: AnnounceRateEntry,
    ) -> RateEntryAdmission {
        self.grow_index_if_loaded();
        let slot = self.destinations.len();
        self.destinations.push(destination);
        self.entries.push(entry);
        self.index_insert(slot);
        RateEntryAdmission::Recorded
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

    fn entry_at(ms: u64) -> AnnounceRateEntry {
        AnnounceRateEntry {
            last_allowed_announce_at: InstantMillis(ms),
            ..AnnounceRateEntry::default()
        }
    }

    #[test]
    fn the_index_finds_inserted_destinations_and_misses_absent_ones() {
        let mut columns = HeapAnnounceRateColumns::default();
        assert_eq!(
            columns.insert(dest_n(1), entry_at(10)),
            RateEntryAdmission::Recorded
        );
        assert_eq!(
            columns.insert(dest_n(2), entry_at(20)),
            RateEntryAdmission::Recorded
        );

        assert_eq!(columns.index_of(&dest_n(1)), Some(0));
        assert_eq!(columns.index_of(&dest_n(2)), Some(1));
        assert_eq!(columns.index_of(&dest_n(999)), None);
    }

    #[test]
    fn the_table_grows_unbounded_and_every_row_stays_findable_through_reindexing() {
        let mut columns = HeapAnnounceRateColumns::default();
        for n in 0..2_000u32 {
            assert_eq!(
                columns.insert(dest_n(n), entry_at(n as u64)),
                RateEntryAdmission::Recorded
            );
        }
        assert_eq!(columns.len(), 2_000);
        for n in 0..2_000u32 {
            assert_eq!(columns.index_of(&dest_n(n)), Some(n as usize));
        }
        assert_eq!(columns.index_of(&dest_n(999_999)), None);
    }
}
