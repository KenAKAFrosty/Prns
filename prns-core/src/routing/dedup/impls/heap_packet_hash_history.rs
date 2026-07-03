use alloc::vec::Vec;

use crate::routing::dedup::{
    PacketHash, PacketHashHistory, RememberPacketOutcome, PACKET_HASH_LEN,
};

/// RNS 1.3.5 `Transport.hashlist_maxsize // 2`: the reference rotates its
/// hashlist once it grows past half the configured maximum (1,000,000).
const RNS_GENERATION_CAPACITY: usize = 500_000;

const EMPTY: usize = usize::MAX;
const MIN_BUCKETS: usize = 8;

#[derive(Debug)]
struct Generation {
    hashes: Vec<[u8; PACKET_HASH_LEN]>,
    index: Vec<usize>,
}

impl Default for Generation {
    fn default() -> Self {
        let mut index = Vec::new();
        index.resize(MIN_BUCKETS, EMPTY);
        Self {
            hashes: Vec::new(),
            index,
        }
    }
}

impl Generation {
    fn key(hash: &[u8; PACKET_HASH_LEN]) -> u64 {
        u64::from_be_bytes([
            hash[0], hash[1], hash[2], hash[3], hash[4], hash[5], hash[6], hash[7],
        ])
    }

    fn bucket(&self, key: u64) -> usize {
        ((key as u128 * self.index.len() as u128) >> u64::BITS) as usize
    }

    fn contains(&self, hash: &[u8; PACKET_HASH_LEN]) -> bool {
        let n = self.index.len();
        let mut pos = self.bucket(Self::key(hash));
        loop {
            let slot = self.index[pos];
            if slot == EMPTY {
                return false;
            }
            if self.hashes[slot] == *hash {
                return true;
            }
            pos = (pos + 1) % n;
        }
    }

    fn index_insert(&mut self, slot: usize) {
        let n = self.index.len();
        let mut pos = self.bucket(Self::key(&self.hashes[slot]));
        while self.index[pos] != EMPTY {
            pos = (pos + 1) % n;
        }
        self.index[pos] = slot;
    }

    fn grow_index_if_loaded(&mut self) {
        if (self.hashes.len() + 1) * 3 > self.index.len() * 2 {
            let new_buckets = self.index.len() * 2;
            self.index.clear();
            self.index.resize(new_buckets, EMPTY);
            for slot in 0..self.hashes.len() {
                self.index_insert(slot);
            }
        }
    }

    fn insert(&mut self, hash: [u8; PACKET_HASH_LEN]) {
        self.grow_index_if_loaded();
        let slot = self.hashes.len();
        self.hashes.push(hash);
        self.index_insert(slot);
    }

    fn clear_retaining_capacity(&mut self) {
        self.hashes.clear();
        self.index.fill(EMPTY);
    }

    fn len(&self) -> usize {
        self.hashes.len()
    }
}

#[derive(Debug, Default)]
pub struct HeapPacketHashHistory {
    current: Generation,
    previous: Generation,
}

impl PacketHashHistory for HeapPacketHashHistory {
    fn generation_capacity(&self) -> usize {
        RNS_GENERATION_CAPACITY
    }

    fn len(&self) -> usize {
        self.current.len() + self.previous.len()
    }

    fn contains(&self, hash: &PacketHash) -> bool {
        self.current.contains(hash.as_bytes()) || self.previous.contains(hash.as_bytes())
    }

    fn remember(&mut self, hash: PacketHash) -> RememberPacketOutcome {
        if self.contains(&hash) {
            return RememberPacketOutcome::AlreadyKnown;
        }

        if self.current.len() < RNS_GENERATION_CAPACITY {
            self.current.insert(*hash.as_bytes());
            return RememberPacketOutcome::StoredFresh;
        }

        core::mem::swap(&mut self.current, &mut self.previous);
        self.current.clear_retaining_capacity();
        self.current.insert(*hash.as_bytes());
        RememberPacketOutcome::StoredAfterRotation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_and_reports_duplicates() {
        let mut history = HeapPacketHashHistory::default();
        let hash = PacketHash::new([0xAB; 32]);

        assert_eq!(history.remember(hash), RememberPacketOutcome::StoredFresh);
        assert_eq!(history.remember(hash), RememberPacketOutcome::AlreadyKnown);
        assert!(history.contains(&hash));
        assert_eq!(history.len(), 1);
        assert_eq!(history.generation_capacity(), 500_000);
    }

    #[test]
    fn many_distinct_hashes_grow_the_index_without_false_duplicates() {
        let mut history = HeapPacketHashHistory::default();
        let mut state = 0x9E37_79B9_u64;
        let mut hashes = Vec::new();
        for _ in 0..10_000 {
            let mut bytes = [0u8; 32];
            for chunk in bytes.chunks_mut(8) {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                chunk.copy_from_slice(&state.to_le_bytes());
            }
            let hash = PacketHash::new(bytes);
            assert_eq!(history.remember(hash), RememberPacketOutcome::StoredFresh);
            hashes.push(hash);
        }
        assert_eq!(history.len(), 10_000);
        for hash in &hashes {
            assert!(history.contains(hash));
            assert_eq!(history.remember(*hash), RememberPacketOutcome::AlreadyKnown);
        }
    }
}
