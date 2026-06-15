use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::routing::path_requests::recent::RecentPathRequestColumns;
use crate::wire::DestinationHash;

/// The reference's `path_requests` is an unbounded dict pruned by age; a
/// daemon-grade cap keeps a destination flood from ballooning memory, matching
/// the other path-request tables' hygiene at the same order of magnitude.
pub const DEFAULT_MAX_RECENT_PATH_REQUESTS: usize = 1024;

#[derive(Debug, Default)]
pub struct HeapRecentPathRequestColumns {
    destinations: Vec<DestinationHash>,
    requested_ats: Vec<InstantMillis>,
}

impl RecentPathRequestColumns for HeapRecentPathRequestColumns {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_RECENT_PATH_REQUESTS
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
        if self.len() >= self.capacity() {
            return;
        }
        self.destinations.push(destination);
        self.requested_ats.push(requested_at);
    }

    fn swap_remove(&mut self, index: usize) {
        self.destinations.swap_remove(index);
        self.requested_ats.swap_remove(index);
    }
}
