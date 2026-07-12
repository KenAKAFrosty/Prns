use alloc::vec::Vec;

use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::routing::path_requests::pending::{
    PendingPathRequest, PendingPathRequestTable, TrackPathRequestError,
};
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapPendingPathRequestTable {
    destinations: Vec<DestinationHash>,
    command_ids: Vec<CommandId>,
    timeout_ats: Vec<InstantMillis>,
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

    fn push(&mut self, request: PendingPathRequest) -> Result<usize, TrackPathRequestError> {
        self.destinations.push(request.destination);
        self.command_ids.push(request.command_id);
        self.timeout_ats.push(request.timeout_at);
        Ok(self.destinations.len() - 1)
    }

    fn swap_remove(&mut self, index: usize) {
        self.destinations.swap_remove(index);
        self.command_ids.swap_remove(index);
        self.timeout_ats.swap_remove(index);
    }
}
