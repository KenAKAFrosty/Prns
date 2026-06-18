use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::announce::interface_announce_limit::{
    BurstState, InterfaceAnnounceLimit, InterfaceAnnounceLimitColumns,
};

#[derive(Debug)]
pub struct FixedInterfaceAnnounceLimitColumns<const MAX_INTERFACES: usize> {
    len: usize,
    rows: [InterfaceAnnounceLimit; MAX_INTERFACES],
}

impl<const MAX_INTERFACES: usize> Default for FixedInterfaceAnnounceLimitColumns<MAX_INTERFACES> {
    fn default() -> Self {
        Self {
            len: 0,
            rows: [InterfaceAnnounceLimit {
                interface: InterfaceId::new([0u8; 8]),
                created_at: InstantMillis(0),
                window_start: InstantMillis(0),
                window_count: 0,
                burst: BurstState::Calm,
            }; MAX_INTERFACES],
        }
    }
}

impl<const MAX_INTERFACES: usize> InterfaceAnnounceLimitColumns
    for FixedInterfaceAnnounceLimitColumns<MAX_INTERFACES>
{
    fn capacity(&self) -> usize {
        MAX_INTERFACES
    }

    fn rows(&self) -> &[InterfaceAnnounceLimit] {
        &self.rows[..self.len]
    }

    fn rows_mut(&mut self) -> &mut [InterfaceAnnounceLimit] {
        &mut self.rows[..self.len]
    }

    fn push(&mut self, row: InterfaceAnnounceLimit) {
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
