use alloc::collections::BTreeSet;

use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};

/// RNS 1.3.1 `Transport.hashlist_maxsize // 2`: the reference rotates its
/// hashlist once it grows past half the configured maximum (1,000,000).
const RNS_GENERATION_CAPACITY: usize = 500_000;

#[derive(Debug, Default)]
pub struct HeapPacketHashHistory {
    current: BTreeSet<[u8; 32]>,
    previous: BTreeSet<[u8; 32]>,
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

        self.previous = core::mem::take(&mut self.current);
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
}
