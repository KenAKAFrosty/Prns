use crate::routing::dedup::PacketHash;
use crate::wire::DestinationHash;

const EMPTY: u16 = u16::MAX;

pub trait IndexKey: Copy + Eq {
    fn lemire_key(&self) -> u64;
}

fn lemire_key_from_prefix(bytes: &[u8]) -> u64 {
    let b = bytes;
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

impl IndexKey for DestinationHash {
    fn lemire_key(&self) -> u64 {
        lemire_key_from_prefix(self.as_bytes())
    }
}

impl IndexKey for PacketHash {
    fn lemire_key(&self) -> u64 {
        lemire_key_from_prefix(self.as_bytes())
    }
}

#[derive(Debug)]
pub struct LemireIndex<const BUCKETS: usize> {
    slots: [u16; BUCKETS],
}

impl<const BUCKETS: usize> Default for LemireIndex<BUCKETS> {
    fn default() -> Self {
        Self {
            slots: [EMPTY; BUCKETS],
        }
    }
}

impl<const BUCKETS: usize> LemireIndex<BUCKETS> {
    fn bucket(key: u64) -> usize {
        ((key as u128 * BUCKETS as u128) >> u64::BITS) as usize
    }

    fn position<K: IndexKey>(&self, target: &K, keys: &[K]) -> Option<usize> {
        let mut pos = Self::bucket(target.lemire_key());
        loop {
            let slot = self.slots[pos];
            if slot == EMPTY {
                return None;
            }
            if keys[slot as usize] == *target {
                return Some(pos);
            }
            pos = (pos + 1) % BUCKETS;
        }
    }

    pub fn get<K: IndexKey>(&self, target: &K, keys: &[K]) -> Option<usize> {
        self.position(target, keys)
            .map(|pos| self.slots[pos] as usize)
    }

    pub fn contains<K: IndexKey>(&self, target: &K, keys: &[K]) -> bool {
        self.position(target, keys).is_some()
    }

    pub fn insert<K: IndexKey>(&mut self, slot: usize, keys: &[K]) {
        let mut pos = Self::bucket(keys[slot].lemire_key());
        while self.slots[pos] != EMPTY {
            pos = (pos + 1) % BUCKETS;
        }
        self.slots[pos] = slot as u16;
    }

    pub fn remove<K: IndexKey>(&mut self, target: &K, keys: &[K]) {
        let Some(mut hole) = self.position(target, keys) else {
            return;
        };
        loop {
            self.slots[hole] = EMPTY;
            let mut scan = hole;
            loop {
                scan = (scan + 1) % BUCKETS;
                let slot = self.slots[scan];
                if slot == EMPTY {
                    return;
                }
                let home = Self::bucket(keys[slot as usize].lemire_key());
                let blocks_move = if hole <= scan {
                    home > hole && home <= scan
                } else {
                    home > hole || home <= scan
                };
                if !blocks_move {
                    self.slots[hole] = slot;
                    hole = scan;
                    break;
                }
            }
        }
    }

    pub fn repoint<K: IndexKey>(&mut self, target: &K, slot: usize, keys: &[K]) {
        if let Some(pos) = self.position(target, keys) {
            self.slots[pos] = slot as u16;
        }
    }

    pub fn clear(&mut self) {
        self.slots = [EMPTY; BUCKETS];
    }
}
