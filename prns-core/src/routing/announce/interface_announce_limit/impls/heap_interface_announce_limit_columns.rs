use alloc::vec::Vec;

use crate::routing::announce::interface_announce_limit::{
    InterfaceAnnounceLimit, InterfaceAnnounceLimitColumns,
};

#[derive(Debug, Default)]
pub struct HeapInterfaceAnnounceLimitColumns {
    rows: Vec<InterfaceAnnounceLimit>,
}

impl InterfaceAnnounceLimitColumns for HeapInterfaceAnnounceLimitColumns {
    fn capacity(&self) -> usize {
        usize::MAX
    }

    fn rows(&self) -> &[InterfaceAnnounceLimit] {
        &self.rows
    }

    fn rows_mut(&mut self) -> &mut [InterfaceAnnounceLimit] {
        &mut self.rows
    }

    fn push(&mut self, row: InterfaceAnnounceLimit) {
        self.rows.push(row);
    }

    fn swap_remove(&mut self, index: usize) {
        self.rows.swap_remove(index);
    }
}
