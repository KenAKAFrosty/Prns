use super::super::core::{exceeds_two_thirds_load, IndexKey, IndexRow};

#[derive(Debug)]
pub struct HeapLemireIndex {
    slots: alloc::vec::Vec<u32>,
}

impl Default for HeapLemireIndex {
    fn default() -> Self {
        let mut slots = alloc::vec::Vec::new();
        slots.resize(Self::MIN_BUCKETS, Self::EMPTY);
        Self { slots }
    }
}

impl HeapLemireIndex {
    const EMPTY: u32 = u32::MAX;
    const MIN_BUCKETS: usize = 8;
    pub const MAX_ROWS: usize = Self::EMPTY as usize;

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

    fn position_of_slot<R: IndexRow>(&self, target_slot: usize, rows: &[R]) -> Option<usize> {
        let target = rows.get(target_slot)?.index_key();
        let n = self.slots.len();
        let mut pos = self.bucket(target.lemire_key());
        loop {
            let slot = self.slots[pos];
            if slot == Self::EMPTY {
                return None;
            }
            if slot as usize == target_slot {
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
            slot < Self::MAX_ROWS,
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
        let Some(hole) = self.position(target, rows) else {
            return;
        };
        self.remove_position(hole, rows);
    }

    pub fn remove_slot<R: IndexRow>(&mut self, slot: usize, rows: &[R]) {
        let Some(hole) = self.position_of_slot(slot, rows) else {
            return;
        };
        self.remove_position(hole, rows);
    }

    fn remove_position<R: IndexRow>(&mut self, mut hole: usize, rows: &[R]) {
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
                slot < Self::MAX_ROWS,
                "HeapLemireIndex cannot represent this row number as u32"
            );
            self.slots[pos] = slot as u32;
        }
    }

    pub fn repoint_slot<R: IndexRow>(&mut self, previous: usize, slot: usize, rows: &[R]) {
        if let Some(pos) = self.position_of_slot(previous, rows) {
            debug_assert!(
                slot < Self::MAX_ROWS,
                "HeapLemireIndex cannot represent this row number as u32"
            );
            self.slots[pos] = slot as u32;
        }
    }

    pub fn clear(&mut self) {
        self.slots.fill(Self::EMPTY);
    }
}
