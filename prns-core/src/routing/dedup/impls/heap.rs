use alloc::vec::Vec;

use crate::lemire_index::HeapLemireIndex;
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
    /// RNS 1.3.5 `Transport.hashlist_maxsize // 2`: the reference rotates its
    /// hashlist once it grows past half the configured maximum (1,000,000).
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
        if self.contains(&hash) {
            return RememberPacketOutcome::AlreadyKnown;
        }

        if self.current.len() < Self::RNS_GENERATION_CAPACITY {
            self.current.insert(hash);
            return RememberPacketOutcome::StoredFresh;
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
