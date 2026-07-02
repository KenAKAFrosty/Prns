use alloc::vec::Vec;

use crate::routing::path_requests::interface_path_request_limit::{
    InterfacePathRequestLimit, InterfacePathRequestLimitColumns,
};

#[derive(Debug, Default)]
pub struct HeapInterfacePathRequestLimitColumns {
    rows: Vec<InterfacePathRequestLimit>,
}

impl InterfacePathRequestLimitColumns for HeapInterfacePathRequestLimitColumns {
    fn capacity(&self) -> usize {
        usize::MAX
    }

    fn rows(&self) -> &[InterfacePathRequestLimit] {
        &self.rows
    }

    fn rows_mut(&mut self) -> &mut [InterfacePathRequestLimit] {
        &mut self.rows
    }

    fn push(&mut self, row: InterfacePathRequestLimit) {
        self.rows.push(row);
    }

    fn swap_remove(&mut self, index: usize) {
        self.rows.swap_remove(index);
    }
}
