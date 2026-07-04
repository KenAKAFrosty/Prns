use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::routing::path_requests::pending::{
    PendingPathRequest, PendingPathRequestColumns, TrackPathRequestError,
};
use crate::wire::DestinationHash;

#[derive(Debug)]
pub struct FixedPendingPathRequestColumns<const MAX_PENDING_PATH_REQUESTS: usize> {
    len: usize,
    destinations: [DestinationHash; MAX_PENDING_PATH_REQUESTS],
    command_ids: [CommandId; MAX_PENDING_PATH_REQUESTS],
    timeout_ats: [InstantMillis; MAX_PENDING_PATH_REQUESTS],
}

impl<const MAX_PENDING_PATH_REQUESTS: usize> Default
    for FixedPendingPathRequestColumns<MAX_PENDING_PATH_REQUESTS>
{
    fn default() -> Self {
        Self {
            len: 0,
            destinations: [DestinationHash::new([0u8; 16]); MAX_PENDING_PATH_REQUESTS],
            command_ids: [CommandId(0); MAX_PENDING_PATH_REQUESTS],
            timeout_ats: [InstantMillis(0); MAX_PENDING_PATH_REQUESTS],
        }
    }
}

impl<const MAX_PENDING_PATH_REQUESTS: usize> PendingPathRequestColumns
    for FixedPendingPathRequestColumns<MAX_PENDING_PATH_REQUESTS>
{
    fn capacity(&self) -> usize {
        MAX_PENDING_PATH_REQUESTS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations[..self.len]
    }
    fn command_ids(&self) -> &[CommandId] {
        &self.command_ids[..self.len]
    }
    fn timeout_ats(&self) -> &[InstantMillis] {
        &self.timeout_ats[..self.len]
    }

    fn push(&mut self, request: PendingPathRequest) -> Result<usize, TrackPathRequestError> {
        if self.len >= MAX_PENDING_PATH_REQUESTS {
            return Err(TrackPathRequestError::TableFull);
        }
        let i = self.len;
        self.destinations[i] = request.destination;
        self.command_ids[i] = request.command_id;
        self.timeout_ats[i] = request.timeout_at;
        self.len += 1;
        Ok(i)
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            self.destinations[index] = self.destinations[last];
            self.command_ids[index] = self.command_ids[last];
            self.timeout_ats[index] = self.timeout_ats[last];
        }
        self.len = last;
    }
}
