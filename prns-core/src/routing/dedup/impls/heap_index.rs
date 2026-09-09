use alloc::vec::Vec;

use crate::lemire_index::{exceeds_two_thirds_load, IndexKey};
use crate::routing::dedup::PacketHash;

#[derive(Debug, Clone, Copy, PartialEq)]
struct PackedBucket(u32);

impl PackedBucket {
    const EMPTY: Self = Self(0);
    const SLOT_BITS: u32 = 19;
    const SLOT_MASK: u32 = (1 << Self::SLOT_BITS) - 1;
    const FINGERPRINT_MASK: u16 = (1 << (u32::BITS - Self::SLOT_BITS)) - 1;

    fn occupied(hash: &PacketHash, slot: usize) -> Self {
        Self::from_parts(fingerprint(hash), slot)
    }

    fn from_parts(fingerprint: u16, slot: usize) -> Self {
        debug_assert!(slot < HeapPacketHashIndex::MAX_ROWS);
        let encoded_slot = slot as u32 + 1;
        Self((u32::from(fingerprint) << Self::SLOT_BITS) | encoded_slot)
    }

    fn slot(self) -> Option<usize> {
        let encoded = self.0 & Self::SLOT_MASK;
        if encoded == 0 {
            None
        } else {
            Some(encoded as usize - 1)
        }
    }

    fn matches(self, fingerprint: u16) -> bool {
        (self.0 >> Self::SLOT_BITS) as u16 == fingerprint
    }
}

fn fingerprint(hash: &PacketHash) -> u16 {
    let bytes = hash.as_bytes();
    u16::from_be_bytes([bytes[8], bytes[9]]) & PackedBucket::FINGERPRINT_MASK
}

enum ProbePosition {
    Occupied,
    Vacant(usize),
}

pub(super) enum HeapPacketHashIndexEntry<'a> {
    Occupied,
    Vacant(HeapPacketHashIndexVacantEntry<'a>),
}

pub(super) struct HeapPacketHashIndexVacantEntry<'a> {
    index: &'a mut HeapPacketHashIndex,
    position: usize,
    fingerprint: u16,
}

#[derive(Debug)]
pub(super) struct HeapPacketHashIndex {
    buckets: Vec<PackedBucket>,
}

impl Default for HeapPacketHashIndex {
    fn default() -> Self {
        let mut buckets = Vec::new();
        buckets.resize(Self::MIN_BUCKETS, PackedBucket::EMPTY);
        Self { buckets }
    }
}

impl HeapPacketHashIndex {
    const MIN_BUCKETS: usize = 8;
    pub(super) const MAX_ROWS: usize = PackedBucket::SLOT_MASK as usize;

    fn bucket(&self, hash: &PacketHash) -> usize {
        ((hash.lemire_key() as u128 * self.buckets.len() as u128) >> u64::BITS) as usize
    }

    fn next_position(&self, position: usize) -> usize {
        let next = position + 1;
        if next == self.buckets.len() {
            0
        } else {
            next
        }
    }

    fn probe(&self, target: &PacketHash, rows: &[PacketHash]) -> ProbePosition {
        let mut position = self.bucket(target);
        let target_fingerprint = fingerprint(target);
        loop {
            let bucket = self.buckets[position];
            let Some(slot) = bucket.slot() else {
                return ProbePosition::Vacant(position);
            };
            if bucket.matches(target_fingerprint) && rows[slot] == *target {
                return ProbePosition::Occupied;
            }
            position = self.next_position(position);
        }
    }

    pub(super) fn contains(&self, target: &PacketHash, rows: &[PacketHash]) -> bool {
        matches!(self.probe(target, rows), ProbePosition::Occupied)
    }

    pub(super) fn entry<'a>(
        &'a mut self,
        target: &PacketHash,
        rows: &[PacketHash],
    ) -> HeapPacketHashIndexEntry<'a> {
        match self.probe(target, rows) {
            ProbePosition::Occupied => HeapPacketHashIndexEntry::Occupied,
            ProbePosition::Vacant(position) => {
                HeapPacketHashIndexEntry::Vacant(HeapPacketHashIndexVacantEntry {
                    index: self,
                    position,
                    fingerprint: fingerprint(target),
                })
            }
        }
    }

    pub(super) fn insert(&mut self, slot: usize, rows: &[PacketHash]) {
        if exceeds_two_thirds_load(rows.len(), self.buckets.len()) {
            self.rebuild(rows);
            return;
        }
        self.place(slot, rows);
    }

    fn place(&mut self, slot: usize, rows: &[PacketHash]) {
        debug_assert!(slot < Self::MAX_ROWS);
        let hash = &rows[slot];
        let mut position = self.bucket(hash);
        while self.buckets[position] != PackedBucket::EMPTY {
            position = self.next_position(position);
        }
        self.buckets[position] = PackedBucket::occupied(hash, slot);
    }

    fn rebuild(&mut self, rows: &[PacketHash]) {
        let mut bucket_count = self.buckets.len().max(Self::MIN_BUCKETS);
        while exceeds_two_thirds_load(rows.len(), bucket_count) {
            let grown = bucket_count.saturating_mul(2);
            if grown == bucket_count {
                break;
            }
            bucket_count = grown;
        }
        self.buckets.clear();
        self.buckets.resize(bucket_count, PackedBucket::EMPTY);
        for slot in 0..rows.len() {
            self.place(slot, rows);
        }
    }

    pub(super) fn clear(&mut self) {
        self.buckets.fill(PackedBucket::EMPTY);
    }
}

impl HeapPacketHashIndexVacantEntry<'_> {
    pub(super) fn insert(self, slot: usize, rows: &[PacketHash]) {
        if exceeds_two_thirds_load(rows.len(), self.index.buckets.len()) {
            self.index.rebuild(rows);
            return;
        }
        debug_assert!(slot < HeapPacketHashIndex::MAX_ROWS);
        debug_assert_eq!(self.index.buckets[self.position], PackedBucket::EMPTY);
        self.index.buckets[self.position] = PackedBucket::from_parts(self.fingerprint, slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colliding_hash(last: u8) -> PacketHash {
        let mut bytes = [0xA5; 32];
        bytes[31] = last;
        PacketHash::new(bytes)
    }

    #[test]
    fn packed_boundaries_remain_distinct_from_empty() {
        for fingerprint in [0, PackedBucket::FINGERPRINT_MASK] {
            for slot in [0, HeapPacketHashIndex::MAX_ROWS - 1] {
                let bucket = PackedBucket::from_parts(fingerprint, slot);
                assert_ne!(bucket, PackedBucket::EMPTY);
                assert_eq!(bucket.slot(), Some(slot));
                assert!(bucket.matches(fingerprint));
            }
        }
    }

    #[test]
    fn matching_fingerprints_still_require_the_complete_hash() {
        let mut index = HeapPacketHashIndex::default();
        let mut rows = Vec::new();
        for hash in [colliding_hash(1), colliding_hash(2)] {
            let HeapPacketHashIndexEntry::Vacant(vacancy) = index.entry(&hash, &rows) else {
                panic!("distinct complete hash was occupied");
            };
            rows.push(hash);
            vacancy.insert(rows.len() - 1, &rows);
        }

        assert!(index.contains(&colliding_hash(1), &rows));
        assert!(index.contains(&colliding_hash(2), &rows));
        assert!(!index.contains(&colliding_hash(3), &rows));
    }

    #[test]
    fn clearing_retains_the_bucket_residence_for_reuse() {
        let mut index = HeapPacketHashIndex::default();
        let mut rows = Vec::new();
        for last in 0..32 {
            let hash = colliding_hash(last);
            rows.push(hash);
            index.insert(rows.len() - 1, &rows);
        }
        let bucket_count = index.buckets.len();

        index.clear();

        assert_eq!(index.buckets.len(), bucket_count);
        assert!(index
            .buckets
            .iter()
            .all(|bucket| *bucket == PackedBucket::EMPTY));
    }
}
