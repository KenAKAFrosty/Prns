use crate::engine::InstantMillis;
use crate::routing::path_requests::recent::RecentPathRequestTable;
use crate::wire::DestinationHash;

#[derive(Debug)]
pub struct FixedRecentPathRequestTable<const MAX_RECENT_PATH_REQUESTS: usize> {
    len: usize,
    destinations: [DestinationHash; MAX_RECENT_PATH_REQUESTS],
    requested_ats: [InstantMillis; MAX_RECENT_PATH_REQUESTS],
}

impl<const MAX_RECENT_PATH_REQUESTS: usize> Default
    for FixedRecentPathRequestTable<MAX_RECENT_PATH_REQUESTS>
{
    fn default() -> Self {
        Self {
            len: 0,
            destinations: [DestinationHash::new([0u8; 16]); MAX_RECENT_PATH_REQUESTS],
            requested_ats: [InstantMillis(0); MAX_RECENT_PATH_REQUESTS],
        }
    }
}

impl<const MAX_RECENT_PATH_REQUESTS: usize> RecentPathRequestTable
    for FixedRecentPathRequestTable<MAX_RECENT_PATH_REQUESTS>
{
    fn capacity(&self) -> usize {
        MAX_RECENT_PATH_REQUESTS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations[..self.len]
    }
    fn requested_ats(&self) -> &[InstantMillis] {
        &self.requested_ats[..self.len]
    }

    fn push(&mut self, destination: DestinationHash, requested_at: InstantMillis) {
        if self.len >= MAX_RECENT_PATH_REQUESTS {
            return;
        }
        let i = self.len;
        self.destinations[i] = destination;
        self.requested_ats[i] = requested_at;
        self.len += 1;
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            self.destinations[index] = self.destinations[last];
            self.requested_ats[index] = self.requested_ats[last];
        }
        self.len = last;
    }
}
