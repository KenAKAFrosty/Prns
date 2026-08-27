use heapless::Vec;

use crate::remote_control::{RemoteControlAccessTable, RemoteControlControllerIdentity};
use crate::storage::TablePushError;

#[derive(Debug)]
pub struct FixedRemoteControlAccessTable<const ACCESS_SLOTS: usize> {
    identities: Vec<RemoteControlControllerIdentity, ACCESS_SLOTS>,
}

impl<const ACCESS_SLOTS: usize> Default for FixedRemoteControlAccessTable<ACCESS_SLOTS> {
    fn default() -> Self {
        Self {
            identities: Vec::new(),
        }
    }
}

impl<const ACCESS_SLOTS: usize> RemoteControlAccessTable
    for FixedRemoteControlAccessTable<ACCESS_SLOTS>
{
    fn capacity(&self) -> usize {
        ACCESS_SLOTS
    }

    fn len(&self) -> usize {
        self.identities.len()
    }

    fn identities(&self) -> &[RemoteControlControllerIdentity] {
        self.identities.as_slice()
    }

    fn upsert(&mut self, identity: RemoteControlControllerIdentity) -> Result<(), TablePushError> {
        let identity_hash = identity.identity_hash();
        if let Some(current) = self
            .identities
            .iter_mut()
            .find(|candidate| candidate.identity_hash() == identity_hash)
        {
            *current = identity;
            return Ok(());
        }
        self.identities
            .push(identity)
            .map_err(|_| TablePushError::TableFull)
    }

    fn swap_remove(&mut self, index: usize) {
        self.identities.swap_remove(index);
    }
}
