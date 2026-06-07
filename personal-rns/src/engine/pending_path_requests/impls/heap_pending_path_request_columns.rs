use alloc::vec::Vec;

use crate::engine::commands::CommandId;
use crate::engine::pending_path_requests::{
    PendingPathRequest, PendingPathRequestColumns, TrackPathRequestError,
};
use crate::engine::InstantMillis;
use crate::wire::DestinationHash;

/// A daemon-grade cap on outstanding path requests — far above any realistic
/// number a node has in flight, but a backstop against a runaway caller, in the
/// same spirit as the receipts and reverse-route ceilings.
pub const DEFAULT_MAX_PENDING_PATH_REQUESTS: usize = 1024;

#[derive(Debug, Default)]
pub struct HeapPendingPathRequestColumns {
    destinations: Vec<DestinationHash>,
    command_ids: Vec<CommandId>,
    timeout_ats: Vec<InstantMillis>,
}

impl PendingPathRequestColumns for HeapPendingPathRequestColumns {
    fn capacity(&self) -> usize {
        DEFAULT_MAX_PENDING_PATH_REQUESTS
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
        if self.len() >= self.capacity() {
            return Err(TrackPathRequestError::TableFull);
        }
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
