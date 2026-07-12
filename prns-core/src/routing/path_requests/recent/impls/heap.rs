use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::routing::path_requests::recent::RecentPathRequestTable;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapRecentPathRequestTable {
    destinations: Vec<DestinationHash>,
    requested_ats: Vec<InstantMillis>,
}

impl RecentPathRequestTable for HeapRecentPathRequestTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn requested_ats(&self) -> &[InstantMillis] {
        &self.requested_ats
    }

    fn push(&mut self, destination: DestinationHash, requested_at: InstantMillis) {
        self.destinations.push(destination);
        self.requested_ats.push(requested_at);
    }

    fn swap_remove(&mut self, index: usize) {
        self.destinations.swap_remove(index);
        self.requested_ats.swap_remove(index);
    }
}
