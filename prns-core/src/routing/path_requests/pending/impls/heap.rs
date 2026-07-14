use alloc::vec::Vec;

use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::lemire_index::HeapLemireIndex;
use crate::routing::path_requests::pending::{
    PendingPathRequest, PendingPathRequestTable, TrackPathRequestError,
};
#[cfg(feature = "std")]
use crate::routing::temporal_index::HeapDeadlineIndex;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapPendingPathRequestTable {
    destinations: Vec<DestinationHash>,
    command_ids: Vec<CommandId>,
    timeout_ats: Vec<InstantMillis>,
    index: HeapLemireIndex,
    #[cfg(feature = "std")]
    timeout_index: HeapDeadlineIndex,
}

impl PendingPathRequestTable for HeapPendingPathRequestTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn command_ids(&self) -> &[CommandId] {
        &self.command_ids
    }
    fn timeout_ats(&self) -> &[InstantMillis] {
        &self.timeout_ats
    }

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.index.get(destination, &self.destinations)
    }

    fn earliest_indexed_timeout(&mut self) -> Option<InstantMillis> {
        #[cfg(feature = "std")]
        {
            let row_count = self.timeout_ats.len();
            let timeout_ats = &self.timeout_ats;
            return self
                .timeout_index
                .earliest_exact(row_count, |row| timeout_ats.get(row).copied());
        }
        #[cfg(not(feature = "std"))]
        self.timeout_ats.iter().min().copied()
    }

    fn first_expired(&mut self, now: InstantMillis) -> Option<usize> {
        #[cfg(feature = "std")]
        {
            let row_count = self.timeout_ats.len();
            let timeout_ats = &self.timeout_ats;
            return self
                .timeout_index
                .first_due(row_count, now, |row| timeout_ats.get(row).copied());
        }
        #[cfg(not(feature = "std"))]
        self.timeout_ats
            .iter()
            .position(|timeout_at| *timeout_at <= now)
    }

    fn push(&mut self, request: PendingPathRequest) -> Result<usize, TrackPathRequestError> {
        let row = self.destinations.len();
        self.destinations.push(request.destination);
        self.command_ids.push(request.command_id);
        self.timeout_ats.push(request.timeout_at);
        self.index.insert(row, &self.destinations);
        #[cfg(feature = "std")]
        {
            let timeout_ats = &self.timeout_ats;
            self.timeout_index
                .insert(row, Some(request.timeout_at), |row| {
                    timeout_ats.get(row).copied()
                });
        }
        Ok(row)
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
            let timeout_ats = &self.timeout_ats;
            self.timeout_index
                .swap_remove(index, last, |row| timeout_ats.get(row).copied());
        }
        self.destinations.swap_remove(index);
        self.command_ids.swap_remove(index);
        self.timeout_ats.swap_remove(index);
    }
}
