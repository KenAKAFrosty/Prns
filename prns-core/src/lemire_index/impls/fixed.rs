use super::super::core::{IndexKey, IndexRow};

const EMPTY: u16 = u16::MAX;

#[derive(Debug)]
pub struct LemireIndex<const BUCKETS: usize> {
    slots: [u16; BUCKETS],
}

impl<const BUCKETS: usize> Default for LemireIndex<BUCKETS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const BUCKETS: usize> LemireIndex<BUCKETS> {
    pub const fn new() -> Self {
        Self {
            slots: [EMPTY; BUCKETS],
        }
    }

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

    fn position_of_slot<R: IndexRow>(&self, target_slot: usize, rows: &[R]) -> Option<usize> {
        let target = rows.get(target_slot)?.index_key();
        let mut pos = Self::bucket(target.lemire_key());
        loop {
            let slot = self.slots[pos];
            if slot == EMPTY {
                return None;
            }
            if slot as usize == target_slot {
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
