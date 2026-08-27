use alloc::vec::Vec;

use crate::remote_control::{RemoteControlAccessTable, RemoteControlControllerIdentity};
use crate::storage::TablePushError;

#[derive(Debug, Default)]
pub struct HeapRemoteControlAccessTable {
    identities: Vec<RemoteControlControllerIdentity>,
}

impl RemoteControlAccessTable for HeapRemoteControlAccessTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }

    fn len(&self) -> usize {
        self.identities.len()
    }

    fn identities(&self) -> &[RemoteControlControllerIdentity] {
        &self.identities
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
        self.identities.push(identity);
        Ok(())
    }

    fn swap_remove(&mut self, index: usize) {
        self.identities.swap_remove(index);
    }
}
