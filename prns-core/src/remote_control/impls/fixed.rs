use heapless::Vec;

use crate::remote_control::{RemoteControlAccessTable, RemoteControlControllerGrant};
use crate::storage::TablePushError;

#[derive(Debug)]
pub struct FixedRemoteControlAccessTable<const ACCESS_SLOTS: usize> {
    grants: Vec<RemoteControlControllerGrant, ACCESS_SLOTS>,
}

impl<const ACCESS_SLOTS: usize> Default for FixedRemoteControlAccessTable<ACCESS_SLOTS> {
    fn default() -> Self {
        Self { grants: Vec::new() }
    }
}

impl<const ACCESS_SLOTS: usize> RemoteControlAccessTable
    for FixedRemoteControlAccessTable<ACCESS_SLOTS>
{
    fn capacity(&self) -> usize {
        ACCESS_SLOTS
    }

    fn len(&self) -> usize {
        self.grants.len()
    }

    fn grants(&self) -> &[RemoteControlControllerGrant] {
        self.grants.as_slice()
    }

    fn upsert(&mut self, grant: RemoteControlControllerGrant) -> Result<(), TablePushError> {
        let identity_hash = grant.controller().identity_hash();
        if let Some(current) = self
            .grants
            .iter_mut()
            .find(|candidate| candidate.controller().identity_hash() == identity_hash)
        {
            *current = grant;
            return Ok(());
        }
        self.grants
            .push(grant)
            .map_err(|_| TablePushError::TableFull)
    }

    fn swap_remove(&mut self, index: usize) {
        self.grants.swap_remove(index);
    }
}
