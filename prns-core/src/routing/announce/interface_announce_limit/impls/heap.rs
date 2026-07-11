use alloc::vec::Vec;

use crate::routing::announce::interface_announce_limit::{
    InterfaceAnnounceLimit, InterfaceAnnounceLimitTable,
};

#[derive(Debug, Default)]
pub struct HeapInterfaceAnnounceLimitTable {
    rows: Vec<InterfaceAnnounceLimit>,
}

impl InterfaceAnnounceLimitTable for HeapInterfaceAnnounceLimitTable {
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
