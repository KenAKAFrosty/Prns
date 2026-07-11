use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::path_requests::interface_path_request_limit::{
    BurstState, InterfacePathRequestLimit, InterfacePathRequestLimitTable,
};

#[derive(Debug)]
pub struct FixedInterfacePathRequestLimitTable<const MAX_INTERFACES: usize> {
    len: usize,
    rows: [InterfacePathRequestLimit; MAX_INTERFACES],
}

impl<const MAX_INTERFACES: usize> Default for FixedInterfacePathRequestLimitTable<MAX_INTERFACES> {
    fn default() -> Self {
        Self {
            len: 0,
            rows: [InterfacePathRequestLimit {
                interface: InterfaceId::new([0u8; 8]),
                created_at: InstantMillis(0),
                window_start: InstantMillis(0),
                window_count: 0,
                burst: BurstState::Calm,
            }; MAX_INTERFACES],
        }
    }
}

impl<const MAX_INTERFACES: usize> InterfacePathRequestLimitTable
    for FixedInterfacePathRequestLimitTable<MAX_INTERFACES>
{
    fn capacity(&self) -> usize {
        MAX_INTERFACES
    }

    fn rows(&self) -> &[InterfacePathRequestLimit] {
        &self.rows[..self.len]
    }

    fn rows_mut(&mut self) -> &mut [InterfacePathRequestLimit] {
        &mut self.rows[..self.len]
    }

    fn push(&mut self, row: InterfacePathRequestLimit) {
        if self.len >= MAX_INTERFACES {
            return;
        }
        self.rows[self.len] = row;
        self.len += 1;
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            self.rows[index] = self.rows[last];
        }
        self.len = last;
    }
}
