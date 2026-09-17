use alloc::vec::Vec;

use crate::lemire_index::{HeapIndexEntry, HeapLemireIndex};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};

#[derive(Debug, Default)]
struct Generation {
    hashes: Vec<PacketHash>,
    index: HeapLemireIndex,
}

impl Generation {
    fn contains(&self, hash: &PacketHash) -> bool {
        self.index.contains(hash, &self.hashes)
    }

    fn insert(&mut self, hash: PacketHash) {
        self.hashes.push(hash);
        self.index.insert(self.hashes.len() - 1, &self.hashes);
    }

    fn clear_retaining_capacity(&mut self) {
        self.hashes.clear();
        self.index.clear();
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

impl HeapPacketHashHistory {
    /// RNS 1.4.2 `Transport.hashlist_maxsize // 2`: the reference rotates its hashlist once it grows past half the configured maximum (1,000,000).
    pub const RNS_GENERATION_CAPACITY: usize = 500_000;
}

impl PacketHashHistory for HeapPacketHashHistory {
    fn generation_capacity(&self) -> usize {
        Self::RNS_GENERATION_CAPACITY
    }

    fn len(&self) -> usize {
        self.current.len() + self.previous.len()
    }

    fn contains(&self, hash: &PacketHash) -> bool {
        self.current.contains(hash) || self.previous.contains(hash)
    }

    fn remember(&mut self, hash: PacketHash) -> RememberPacketOutcome {
        let current_len = self.current.len();
        match self.current.index.entry(&hash, &self.current.hashes) {
            HeapIndexEntry::Occupied => return RememberPacketOutcome::AlreadyKnown,
            HeapIndexEntry::Vacant(vacancy) => {
                if self.previous.contains(&hash) {
                    return RememberPacketOutcome::AlreadyKnown;
                }
                if current_len < Self::RNS_GENERATION_CAPACITY {
                    self.current.hashes.push(hash);
                    vacancy.insert(current_len, &self.current.hashes);
                    return RememberPacketOutcome::StoredFresh;
                }
            }
        }
        core::mem::swap(&mut self.current, &mut self.previous);
        self.current.clear_retaining_capacity();
        self.current.insert(hash);
        RememberPacketOutcome::StoredAfterRotation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered_hash(number: u64) -> PacketHash {
        let key = number.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&key.to_be_bytes());
        bytes[8..16].copy_from_slice(&number.to_be_bytes());
        PacketHash::new(bytes)
    }

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

    #[test]
    fn rotation_retains_the_full_generation_for_duplicate_detection() {
        let mut history = HeapPacketHashHistory::default();
        for number in 0..HeapPacketHashHistory::RNS_GENERATION_CAPACITY as u64 {
            assert_eq!(
                history.remember(numbered_hash(number)),
                RememberPacketOutcome::StoredFresh
            );
        }

        assert_eq!(
            history.remember(numbered_hash(
                HeapPacketHashHistory::RNS_GENERATION_CAPACITY as u64
            )),
            RememberPacketOutcome::StoredAfterRotation
        );
        assert_eq!(
            history.remember(numbered_hash(0)),
            RememberPacketOutcome::AlreadyKnown
        );
        assert_eq!(
            history.remember(numbered_hash(
                HeapPacketHashHistory::RNS_GENERATION_CAPACITY as u64
            )),
            RememberPacketOutcome::AlreadyKnown
        );
    }
}
