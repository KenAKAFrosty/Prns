use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::lemire_index::HeapLemireIndex;
use crate::routing::path_requests::recent::{RecentPathRequestTable, PATH_REQUEST_MIN_INTERVAL_MS};
#[cfg(feature = "std")]
use crate::routing::temporal_index::HeapDeadlineIndex;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapRecentPathRequestTable {
    destinations: Vec<DestinationHash>,
    requested_ats: Vec<InstantMillis>,
    index: HeapLemireIndex,
    #[cfg(feature = "std")]
    expiry_index: HeapDeadlineIndex,
}

#[cfg(feature = "std")]
fn expires_at(requested_at: InstantMillis) -> Option<InstantMillis> {
    requested_at
        .0
        .checked_add(PATH_REQUEST_MIN_INTERVAL_MS)
        .map(InstantMillis)
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

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index.get(destination, &self.destinations)
    }

    fn first_stale(&mut self, now: InstantMillis) -> Option<usize> {
        #[cfg(feature = "std")]
        {
            let row_count = self.requested_ats.len();
            let requested_ats = &self.requested_ats;
            return self.expiry_index.first_due(row_count, now, |row| {
                requested_ats.get(row).copied().and_then(expires_at)
            });
        }
        #[cfg(not(feature = "std"))]
        self.requested_ats.iter().position(|requested_at| {
            now.0.saturating_sub(requested_at.0) >= PATH_REQUEST_MIN_INTERVAL_MS
        })
    }

    fn push(&mut self, destination: DestinationHash, requested_at: InstantMillis) {
        let row = self.destinations.len();
        self.destinations.push(destination);
        self.requested_ats.push(requested_at);
        self.index.insert(row, &self.destinations);
        #[cfg(feature = "std")]
        {
            let requested_ats = &self.requested_ats;
            self.expiry_index
                .insert(row, expires_at(requested_at), |row| {
                    requested_ats.get(row).copied().and_then(expires_at)
                });
        }
    }

    fn swap_remove(&mut self, index: usize) {
        if index >= self.destinations.len() {
            return;
        }
        let last = self.destinations.len() - 1;
        self.index.remove_slot(index, &self.destinations);
        if index != last {
            self.index.repoint_slot(last, index, &self.destinations);
        }
        #[cfg(feature = "std")]
        {
            let requested_ats = &self.requested_ats;
            self.expiry_index.swap_remove(index, last, |row| {
                requested_ats.get(row).copied().and_then(expires_at)
            });
        }
        self.destinations.swap_remove(index);
        self.requested_ats.swap_remove(index);
    }
}
