use alloc::vec::Vec;

use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlControllerGrant,
    RemoteControlControllerGrantTable, RemoteControlControllerIdentity, RemoteControlTargetAccess,
    RemoteControlTargetAccessTable, RemoteControlTargetIdentity,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantError,
    SetRemoteControlControllerGrantOutcome, SetRemoteControlTargetAccessError,
    SetRemoteControlTargetAccessOutcome,
};

#[derive(Debug, Default)]
pub struct HeapRemoteControlControllerGrantTable {
    grants: Vec<RemoteControlControllerGrant>,
}

impl RemoteControlControllerGrantTable for HeapRemoteControlControllerGrantTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }

    fn len(&self) -> usize {
        self.grants.len()
    }

    fn grants_in_identity_hash_order(&self) -> &[RemoteControlControllerGrant] {
        &self.grants
    }

    fn set_controller_grant(
        &mut self,
        grant: RemoteControlControllerGrant,
    ) -> Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantError> {
        let identity_hash = grant.controller().identity_hash();
        if let Some(current) = self
            .grants
            .iter_mut()
            .find(|candidate| candidate.controller().identity_hash() == identity_hash)
        {
            if *current == grant {
                return Ok(SetRemoteControlControllerGrantOutcome::Unchanged);
            }
            let previous = core::mem::replace(current, grant);
            return Ok(SetRemoteControlControllerGrantOutcome::Updated { previous });
        }
        let insert_at = self
            .grants
            .iter()
            .position(|candidate| {
                candidate.controller().identity_hash().as_bytes() > identity_hash.as_bytes()
            })
            .unwrap_or(self.grants.len());
        self.grants.insert(insert_at, grant);
        Ok(SetRemoteControlControllerGrantOutcome::Added)
    }

    fn revoke_controller(
        &mut self,
        controller: &RemoteControlControllerIdentity,
    ) -> RevokeRemoteControlControllerOutcome {
        let identity_hash = controller.identity_hash();
        let Some(index) = self
            .grants
            .iter()
            .position(|grant| grant.controller().identity_hash() == identity_hash)
        else {
            return RevokeRemoteControlControllerOutcome::NotFound;
        };
        RevokeRemoteControlControllerOutcome::Revoked {
            grant: self.grants.remove(index),
        }
    }
}

#[derive(Debug, Default)]
pub struct HeapRemoteControlTargetAccessTable {
    accesses: Vec<RemoteControlTargetAccess>,
}

impl RemoteControlTargetAccessTable for HeapRemoteControlTargetAccessTable {
    fn capacity(&self) -> usize {
        usize::MAX
    }

    fn len(&self) -> usize {
        self.accesses.len()
    }

    fn accesses_in_identity_hash_order(&self) -> &[RemoteControlTargetAccess] {
        &self.accesses
    }

    fn set_target_access(
        &mut self,
        access: RemoteControlTargetAccess,
    ) -> Result<SetRemoteControlTargetAccessOutcome, SetRemoteControlTargetAccessError> {
        let identity_hash = access.target().identity_hash();
        if let Some(current) = self
            .accesses
            .iter_mut()
            .find(|candidate| candidate.target().identity_hash() == identity_hash)
        {
            if *current == access {
                return Ok(SetRemoteControlTargetAccessOutcome::Unchanged);
            }
            let previous = core::mem::replace(current, access);
            return Ok(SetRemoteControlTargetAccessOutcome::Updated { previous });
        }
        let insert_at = self
            .accesses
            .iter()
            .position(|candidate| {
                candidate.target().identity_hash().as_bytes() > identity_hash.as_bytes()
            })
            .unwrap_or(self.accesses.len());
        self.accesses.insert(insert_at, access);
        Ok(SetRemoteControlTargetAccessOutcome::Added)
    }

    fn forget_target(
        &mut self,
        target: &RemoteControlTargetIdentity,
    ) -> ForgetRemoteControlTargetOutcome {
        let identity_hash = target.identity_hash();
        let Some(index) = self
            .accesses
            .iter()
            .position(|access| access.target().identity_hash() == identity_hash)
        else {
            return ForgetRemoteControlTargetOutcome::NotFound;
        };
        ForgetRemoteControlTargetOutcome::Forgotten {
            access: self.accesses.remove(index),
        }
    }
}
