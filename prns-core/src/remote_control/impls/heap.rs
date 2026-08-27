use alloc::vec::Vec;

use crate::remote_control::{RemoteControlAccessTable, RemoteControlControllerGrant};
use crate::storage::TablePushError;

#[derive(Debug, Default)]
pub struct HeapRemoteControlAccessTable {
    grants: Vec<RemoteControlControllerGrant>,
}

impl RemoteControlAccessTable for HeapRemoteControlAccessTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }

    fn len(&self) -> usize {
        self.grants.len()
    }

    fn grants(&self) -> &[RemoteControlControllerGrant] {
        &self.grants
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
        self.grants.push(grant);
        Ok(())
    }

    fn swap_remove(&mut self, index: usize) {
        self.grants.swap_remove(index);
    }
}
