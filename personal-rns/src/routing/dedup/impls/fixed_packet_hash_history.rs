use crate::routing::dedup::{
    PacketHash, PacketHashHistory, RememberPacketOutcome, PACKET_HASH_LEN,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedPacketHashHistory<const GENERATION_CAPACITY: usize> {
    current_len: usize,
    previous_len: usize,
    current: [PacketHash; GENERATION_CAPACITY],
    previous: [PacketHash; GENERATION_CAPACITY],
}

impl<const GENERATION_CAPACITY: usize> Default for FixedPacketHashHistory<GENERATION_CAPACITY> {
    fn default() -> Self {
        Self {
            current_len: 0,
            previous_len: 0,
            current: [PacketHash::new([0u8; PACKET_HASH_LEN]); GENERATION_CAPACITY],
            previous: [PacketHash::new([0u8; PACKET_HASH_LEN]); GENERATION_CAPACITY],
        }
    }
}

impl<const GENERATION_CAPACITY: usize> PacketHashHistory
    for FixedPacketHashHistory<GENERATION_CAPACITY>
{
    fn generation_capacity(&self) -> usize {
        GENERATION_CAPACITY
    }

    fn len(&self) -> usize {
        self.current_len + self.previous_len
    }

    fn contains(&self, hash: &PacketHash) -> bool {
        self.current[..self.current_len].contains(hash)
            || self.previous[..self.previous_len].contains(hash)
    }

    fn remember(&mut self, hash: PacketHash) -> RememberPacketOutcome {
        if self.contains(&hash) {
            return RememberPacketOutcome::AlreadyKnown;
        }

        if self.current_len < GENERATION_CAPACITY {
            self.current[self.current_len] = hash;
            self.current_len += 1;
            return RememberPacketOutcome::StoredFresh;
        }

        core::mem::swap(&mut self.current, &mut self.previous);
        self.previous_len = self.current_len;
        self.current[0] = hash;
        self.current_len = 1;
        RememberPacketOutcome::StoredAfterRotation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> PacketHash {
        PacketHash::new([byte; PACKET_HASH_LEN])
    }

    #[test]
    fn remembers_across_both_generations_and_reports_duplicates() {
        let mut history = FixedPacketHashHistory::<2>::default();
        assert!(history.is_empty());

        assert_eq!(
            history.remember(hash(1)),
            RememberPacketOutcome::StoredFresh
        );
        assert_eq!(
            history.remember(hash(2)),
            RememberPacketOutcome::StoredFresh
        );
        assert_eq!(
            history.remember(hash(1)),
            RememberPacketOutcome::AlreadyKnown
        );
        assert_eq!(history.len(), 2);

        assert_eq!(
            history.remember(hash(3)),
            RememberPacketOutcome::StoredAfterRotation,
        );
        assert!(history.contains(&hash(1)));
        assert!(history.contains(&hash(2)));
        assert!(history.contains(&hash(3)));
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn two_rotations_forget_the_oldest_generation() {
        let mut history = FixedPacketHashHistory::<2>::default();
        for byte in 1..=2 {
            let _ = history.remember(hash(byte));
        }
        assert_eq!(
            history.remember(hash(3)),
            RememberPacketOutcome::StoredAfterRotation,
        );
        let _ = history.remember(hash(4));
        assert_eq!(
            history.remember(hash(5)),
            RememberPacketOutcome::StoredAfterRotation,
        );

        assert!(!history.contains(&hash(1)));
        assert!(!history.contains(&hash(2)));
        assert!(history.contains(&hash(3)));
        assert!(history.contains(&hash(4)));
        assert!(history.contains(&hash(5)));
    }

    #[test]
    fn a_duplicate_in_the_previous_generation_is_still_known() {
        let mut history = FixedPacketHashHistory::<2>::default();
        let _ = history.remember(hash(1));
        let _ = history.remember(hash(2));
        let _ = history.remember(hash(3));
        assert_eq!(
            history.remember(hash(1)),
            RememberPacketOutcome::AlreadyKnown
        );
    }
}
