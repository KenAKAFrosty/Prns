use alloc::vec::Vec;

use crate::routing::announce::held::{HeldAnnounce, HeldAnnounceColumns, MAX_HELD_ANNOUNCES};

#[derive(Debug, Default)]
pub struct HeapHeldAnnounceColumns {
    rows: Vec<HeldAnnounce>,
}

impl HeldAnnounceColumns for HeapHeldAnnounceColumns {
    fn capacity(&self) -> usize {
        MAX_HELD_ANNOUNCES
    }

    fn rows(&self) -> &[HeldAnnounce] {
        &self.rows
    }

    fn rows_mut(&mut self) -> &mut [HeldAnnounce] {
        &mut self.rows
    }

    fn push(&mut self, row: HeldAnnounce) {
        self.rows.push(row);
    }

    fn swap_remove(&mut self, index: usize) {
        self.rows.swap_remove(index);
    }
}
