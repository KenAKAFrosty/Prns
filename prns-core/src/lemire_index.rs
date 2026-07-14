//! A side index answering "which row holds this key" without scanning the table.
//!
//! Each bucket holds one row number.
//! A key picks its bucket by multiplying its own leading bytes against the bucket count and keeping the high half.
//! Our keys are usually truncated hashes, already uniform, so their bytes need no extra hashing (Lemire's reduction).
//! A taken bucket sends the newcomer one to the right, a lookup walks the same way, and the first empty bucket it meets is proof the key is absent.
//! Removal therefore can't simply empty a bucket mid-run (a later key's walk would stop short at the hole) so the entries after it re-pack, each moving back only if its own home bucket still reaches it there.
//!
//! [`LemireIndex`]'s callers hold two invariants with const asserts: `BUCKETS` keeps free headroom over the table's capacity through their domain sizing functions, so a missing key always meets an empty bucket rather than walking forever; and tables stay below `u16::MAX` rows because slot numbers are `u16` and reserve their top value as the empty marker.
//! [`HeapLemireIndex`] serves the growable tables and holds both invariants itself: its buckets double (re-placing every key) whenever one more row would pass 2/3 full, and its slots widen to `u32`.

use crate::routing::dedup::PacketHash;
use crate::wire::DestinationHash;

const EMPTY: u16 = u16::MAX;

pub(crate) const fn buckets_for_two_thirds_load(entries: usize) -> usize {
    if entries == 0 {
        return 1;
    }
    entries.saturating_add(entries.div_ceil(2))
}

#[cfg(any(feature = "alloc", test))]
const fn exceeds_two_thirds_load(entries: usize, buckets: usize) -> bool {
    entries > buckets.saturating_sub(buckets.div_ceil(3))
}

pub trait IndexKey: Copy + Eq {
    fn lemire_key(&self) -> u64;
}

pub trait IndexRow {
    type Key: IndexKey;

    fn index_key(&self) -> &Self::Key;
}

impl<K: IndexKey> IndexRow for K {
    type Key = K;

    fn index_key(&self) -> &Self::Key {
        self
    }
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

impl IndexKey for crate::routing::links::LinkId {
    fn lemire_key(&self) -> u64 {
        lemire_key_from_prefix(self.as_bytes())
    }
}

impl IndexKey for crate::interfaces::InterfaceId {
    /// Little-endian, unlike the hash keys above: an id's first byte is its kind, shared by every interface of that kind (a server's thousand TCP clients all match), and the bucket reduction weighs high bits most.
    /// Read low-endian, the kind byte lands in the bits the reduction barely sees and the channel-tag hash bytes pick the bucket.
    fn lemire_key(&self) -> u64 {
        u64::from_le_bytes(*self.as_bytes())
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

    fn position<R: IndexRow>(&self, target: &R::Key, rows: &[R]) -> Option<usize> {
        let mut pos = Self::bucket(target.lemire_key());
        loop {
            let slot = self.slots[pos];
            if slot == EMPTY {
                return None;
            }
            if rows[slot as usize].index_key() == target {
                return Some(pos);
            }
            pos = (pos + 1) % BUCKETS;
        }
    }

    pub fn get<R: IndexRow>(&self, target: &R::Key, rows: &[R]) -> Option<usize> {
        self.position(target, rows)
            .map(|pos| self.slots[pos] as usize)
    }

    pub fn contains<R: IndexRow>(&self, target: &R::Key, rows: &[R]) -> bool {
        self.position(target, rows).is_some()
    }

    pub fn insert<R: IndexRow>(&mut self, slot: usize, rows: &[R]) {
        debug_assert!(
            slot < EMPTY as usize,
            "LemireIndex cannot represent this row number as u16"
        );
        let mut pos = Self::bucket(rows[slot].index_key().lemire_key());
        while self.slots[pos] != EMPTY {
            pos = (pos + 1) % BUCKETS;
        }
        self.slots[pos] = slot as u16;
    }

    pub fn remove<R: IndexRow>(&mut self, target: &R::Key, rows: &[R]) {
        let Some(mut hole) = self.position(target, rows) else {
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
                let home = Self::bucket(rows[slot as usize].index_key().lemire_key());
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

    pub fn repoint<R: IndexRow>(&mut self, target: &R::Key, slot: usize, rows: &[R]) {
        if let Some(pos) = self.position(target, rows) {
            debug_assert!(
                slot < EMPTY as usize,
                "LemireIndex cannot represent this row number as u16"
            );
            self.slots[pos] = slot as u16;
        }
    }

    pub fn clear(&mut self) {
        self.slots = [EMPTY; BUCKETS];
    }
}

#[cfg(feature = "alloc")]
#[derive(Debug)]
pub struct HeapLemireIndex {
    slots: alloc::vec::Vec<u32>,
}

#[cfg(feature = "alloc")]
impl Default for HeapLemireIndex {
    fn default() -> Self {
        let mut slots = alloc::vec::Vec::new();
        slots.resize(Self::MIN_BUCKETS, Self::EMPTY);
        Self { slots }
    }
}

#[cfg(feature = "alloc")]
impl HeapLemireIndex {
    const EMPTY: u32 = u32::MAX;
    const MIN_BUCKETS: usize = 8;

    fn bucket(&self, key: u64) -> usize {
        ((key as u128 * self.slots.len() as u128) >> u64::BITS) as usize
    }

    fn position<R: IndexRow>(&self, target: &R::Key, rows: &[R]) -> Option<usize> {
        let n = self.slots.len();
        let mut pos = self.bucket(target.lemire_key());
        loop {
            let slot = self.slots[pos];
            if slot == Self::EMPTY {
                return None;
            }
            if rows[slot as usize].index_key() == target {
                return Some(pos);
            }
            pos = (pos + 1) % n;
        }
    }

    pub fn get<R: IndexRow>(&self, target: &R::Key, rows: &[R]) -> Option<usize> {
        self.position(target, rows)
            .map(|pos| self.slots[pos] as usize)
    }

    pub fn contains<R: IndexRow>(&self, target: &R::Key, rows: &[R]) -> bool {
        self.position(target, rows).is_some()
    }

    /// The caller pushes the row first, so `rows` already holds `slot`.
    pub fn insert<R: IndexRow>(&mut self, slot: usize, rows: &[R]) {
        if exceeds_two_thirds_load(rows.len(), self.slots.len()) {
            self.rebuild(rows);
            return;
        }
        self.place(slot, rows);
    }

    fn place<R: IndexRow>(&mut self, slot: usize, rows: &[R]) {
        debug_assert!(
            slot < Self::EMPTY as usize,
            "HeapLemireIndex cannot represent this row number as u32"
        );
        let n = self.slots.len();
        let mut pos = self.bucket(rows[slot].index_key().lemire_key());
        while self.slots[pos] != Self::EMPTY {
            pos = (pos + 1) % n;
        }
        self.slots[pos] = slot as u32;
    }

    fn rebuild<R: IndexRow>(&mut self, rows: &[R]) {
        let mut buckets = self.slots.len().max(Self::MIN_BUCKETS);
        while exceeds_two_thirds_load(rows.len(), buckets) {
            let grown = buckets.saturating_mul(2);
            if grown == buckets {
                break;
            }
            buckets = grown;
        }
        self.slots.clear();
        self.slots.resize(buckets, Self::EMPTY);
        for slot in 0..rows.len() {
            self.place(slot, rows);
        }
    }

    pub fn remove<R: IndexRow>(&mut self, target: &R::Key, rows: &[R]) {
        let Some(mut hole) = self.position(target, rows) else {
            return;
        };
        let n = self.slots.len();
        loop {
            self.slots[hole] = Self::EMPTY;
            let mut scan = hole;
            loop {
                scan = (scan + 1) % n;
                let slot = self.slots[scan];
                if slot == Self::EMPTY {
                    return;
                }
                let home = self.bucket(rows[slot as usize].index_key().lemire_key());
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

    pub fn repoint<R: IndexRow>(&mut self, target: &R::Key, slot: usize, rows: &[R]) {
        if let Some(pos) = self.position(target, rows) {
            debug_assert!(
                slot < Self::EMPTY as usize,
                "HeapLemireIndex cannot represent this row number as u32"
            );
            self.slots[pos] = slot as u32;
        }
    }

    pub fn clear(&mut self) {
        self.slots.fill(Self::EMPTY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::InterfaceId;

    #[test]
    fn bucket_sizing_preserves_two_thirds_headroom_without_overflow() {
        assert_eq!(buckets_for_two_thirds_load(0), 1);
        assert_eq!(buckets_for_two_thirds_load(1), 2);
        assert_eq!(buckets_for_two_thirds_load(2), 3);
        assert_eq!(buckets_for_two_thirds_load(3), 5);
        assert_eq!(buckets_for_two_thirds_load(1_024), 1_536);
        assert_eq!(buckets_for_two_thirds_load(usize::MAX), usize::MAX);
        assert!(!exceeds_two_thirds_load(5, 8));
        assert!(exceeds_two_thirds_load(6, 8));
        assert!(!exceeds_two_thirds_load(usize::MAX / 2, usize::MAX));
    }

    #[test]
    fn interface_ids_sharing_a_kind_byte_still_spread_across_buckets() {
        let a = InterfaceId::new([0x07, 0, 0, 0, 0, 0, 0, 0x11]);
        let b = InterfaceId::new([0x07, 0, 0, 0, 0, 0, 0, 0x22]);
        assert_eq!(a.lemire_key() & 0xff, 0x07);
        assert_ne!(a.lemire_key() >> 56, b.lemire_key() >> 56);
    }

    #[cfg(feature = "alloc")]
    mod heap {
        use super::super::HeapLemireIndex;
        use crate::wire::DestinationHash;

        fn dest_n(n: u32) -> DestinationHash {
            let key = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&key.to_be_bytes());
            b[8..12].copy_from_slice(&n.to_be_bytes());
            DestinationHash::new(b)
        }

        #[test]
        fn every_key_stays_findable_across_rebuilds_and_absent_keys_miss() {
            let mut index = HeapLemireIndex::default();
            let mut keys = std::vec::Vec::new();
            for n in 0..1_000u32 {
                keys.push(dest_n(n));
                index.insert(keys.len() - 1, &keys);
            }
            for (slot, key) in keys.iter().enumerate() {
                assert_eq!(index.get(key, &keys), Some(slot));
            }
            assert_eq!(index.get(&dest_n(1_000), &keys), None);
        }

        #[test]
        fn removal_re_packs_so_later_keys_survive_and_repoint_follows_a_swap() {
            let mut index = HeapLemireIndex::default();
            let mut keys = std::vec::Vec::new();
            for n in 0..100u32 {
                keys.push(dest_n(n));
                index.insert(keys.len() - 1, &keys);
            }

            let removed = keys[10];
            index.remove(&removed, &keys);
            let moved = keys[keys.len() - 1];
            index.repoint(&moved, 10, &keys);
            keys.swap_remove(10);

            assert_eq!(index.get(&removed, &keys), None);
            for (slot, key) in keys.iter().enumerate() {
                assert_eq!(index.get(key, &keys), Some(slot));
            }
        }

        #[test]
        fn clear_empties_every_bucket_without_shrinking() {
            let mut index = HeapLemireIndex::default();
            let mut keys = std::vec::Vec::new();
            for n in 0..100u32 {
                keys.push(dest_n(n));
                index.insert(keys.len() - 1, &keys);
            }
            index.clear();
            assert_eq!(index.get(&dest_n(0), &keys), None);

            keys.clear();
            keys.push(dest_n(7));
            index.insert(0, &keys);
            assert_eq!(index.get(&dest_n(7), &keys), Some(0));
        }
    }
}
