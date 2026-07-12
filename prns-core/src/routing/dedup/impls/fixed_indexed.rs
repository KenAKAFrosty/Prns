use crate::lemire_index::LemireIndex;
use crate::routing::dedup::{
    dedup_index_buckets, PacketHash, PacketHashHistory, RememberPacketOutcome, PACKET_HASH_LEN,
};

struct Generation<const CAP: usize, const BUCKETS: usize> {
    len: usize,
    hashes: [PacketHash; CAP],
    index: LemireIndex<BUCKETS>,
}

impl<const CAP: usize, const BUCKETS: usize> Default for Generation<CAP, BUCKETS> {
    fn default() -> Self {
        const {
            assert!(
                BUCKETS >= dedup_index_buckets(CAP),
                "BUCKETS must give the dedup index its 2/3-load headroom over CAP: size it with dedup_index_buckets(CAP)",
            );
            assert!(
                CAP < u16::MAX as usize,
                "FixedIndexedPacketHashHistory indexes slots as u16; keep CAP below 65535",
            );
        }
        Self {
            len: 0,
            hashes: [PacketHash::new([0u8; PACKET_HASH_LEN]); CAP],
            index: LemireIndex::default(),
        }
    }
}

impl<const CAP: usize, const BUCKETS: usize> Generation<CAP, BUCKETS> {
    fn contains(&self, hash: &PacketHash) -> bool {
        self.index.contains(hash, &self.hashes[..])
    }

    fn insert(&mut self, hash: PacketHash) {
        let slot = self.len;
        self.hashes[slot] = hash;
        self.index.insert(slot, &self.hashes[..]);
        self.len += 1;
    }

    fn clear(&mut self) {
        self.len = 0;
        self.index.clear();
    }

    fn len(&self) -> usize {
        self.len
    }
}

pub struct FixedIndexedPacketHashHistory<const CAP: usize, const BUCKETS: usize> {
    generations: [Generation<CAP, BUCKETS>; 2],
    current: usize,
}

impl<const CAP: usize, const BUCKETS: usize> Default
    for FixedIndexedPacketHashHistory<CAP, BUCKETS>
{
    fn default() -> Self {
        Self {
            generations: [Generation::default(), Generation::default()],
            current: 0,
        }
    }
}

impl<const CAP: usize, const BUCKETS: usize> PacketHashHistory
    for FixedIndexedPacketHashHistory<CAP, BUCKETS>
{
    fn generation_capacity(&self) -> usize {
        CAP
    }

    fn len(&self) -> usize {
        self.generations[0].len() + self.generations[1].len()
    }

    fn contains(&self, hash: &PacketHash) -> bool {
        self.generations[0].contains(hash) || self.generations[1].contains(hash)
    }

    fn remember(&mut self, hash: PacketHash) -> RememberPacketOutcome {
        if self.contains(&hash) {
            return RememberPacketOutcome::AlreadyKnown;
        }
        if self.generations[self.current].len() < CAP {
            self.generations[self.current].insert(hash);
            return RememberPacketOutcome::StoredFresh;
        }
        self.current ^= 1;
        self.generations[self.current].clear();
        self.generations[self.current].insert(hash);
        RememberPacketOutcome::StoredAfterRotation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::dedup::dedup_index_buckets;

    fn hash(byte: u8) -> PacketHash {
        PacketHash::new([byte; PACKET_HASH_LEN])
    }

    type Hist2 = FixedIndexedPacketHashHistory<2, { dedup_index_buckets(2) }>;

    #[test]
    fn remembers_across_both_generations_and_reports_duplicates() {
        let mut h = Hist2::default();
        assert!(h.is_empty());
        assert_eq!(h.remember(hash(1)), RememberPacketOutcome::StoredFresh);
        assert_eq!(h.remember(hash(2)), RememberPacketOutcome::StoredFresh);
        assert_eq!(h.remember(hash(1)), RememberPacketOutcome::AlreadyKnown);
        assert_eq!(h.len(), 2);

        assert_eq!(
            h.remember(hash(3)),
            RememberPacketOutcome::StoredAfterRotation
        );
        assert!(h.contains(&hash(1)));
        assert!(h.contains(&hash(2)));
        assert!(h.contains(&hash(3)));
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn two_rotations_forget_the_oldest_generation() {
        let mut h = Hist2::default();
        let _ = h.remember(hash(1));
        let _ = h.remember(hash(2));
        assert_eq!(
            h.remember(hash(3)),
            RememberPacketOutcome::StoredAfterRotation
        );
        let _ = h.remember(hash(4));
        assert_eq!(
            h.remember(hash(5)),
            RememberPacketOutcome::StoredAfterRotation
        );

        assert!(!h.contains(&hash(1)));
        assert!(!h.contains(&hash(2)));
        assert!(h.contains(&hash(3)));
        assert!(h.contains(&hash(4)));
        assert!(h.contains(&hash(5)));
    }

    #[test]
    fn a_duplicate_in_the_previous_generation_is_still_known() {
        let mut h = Hist2::default();
        let _ = h.remember(hash(1));
        let _ = h.remember(hash(2));
        let _ = h.remember(hash(3));
        assert_eq!(h.remember(hash(1)), RememberPacketOutcome::AlreadyKnown);
    }

    #[test]
    fn a_full_generation_keeps_every_hash_findable_through_probe_collisions() {
        type Hist8 = FixedIndexedPacketHashHistory<8, { dedup_index_buckets(8) }>;
        let mut h = Hist8::default();
        for n in 0..8u8 {
            assert_eq!(h.remember(hash(n)), RememberPacketOutcome::StoredFresh);
        }
        for n in 0..8u8 {
            assert!(h.contains(&hash(n)));
        }
        assert_eq!(
            h.remember(hash(8)),
            RememberPacketOutcome::StoredAfterRotation
        );
        for n in 0..=8u8 {
            assert!(h.contains(&hash(n)));
        }
        assert!(!h.contains(&hash(9)));
    }
}
