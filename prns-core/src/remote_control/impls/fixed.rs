use heapless::Vec;

use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlControllerGrant,
    RemoteControlControllerGrantTable, RemoteControlControllerIdentity, RemoteControlTargetAccess,
    RemoteControlTargetAccessTable, RemoteControlTargetIdentity,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantError,
    SetRemoteControlControllerGrantOutcome, SetRemoteControlTargetAccessError,
    SetRemoteControlTargetAccessOutcome,
};

#[derive(Debug)]
pub struct FixedRemoteControlControllerGrantTable<const CONTROLLER_SLOTS: usize> {
    grants: Vec<RemoteControlControllerGrant, CONTROLLER_SLOTS>,
}

impl<const CONTROLLER_SLOTS: usize> Default
    for FixedRemoteControlControllerGrantTable<CONTROLLER_SLOTS>
{
    fn default() -> Self {
        Self { grants: Vec::new() }
    }
}

impl<const CONTROLLER_SLOTS: usize> RemoteControlControllerGrantTable
    for FixedRemoteControlControllerGrantTable<CONTROLLER_SLOTS>
{
    fn capacity(&self) -> usize {
        CONTROLLER_SLOTS
    }

    fn len(&self) -> usize {
        self.grants.len()
    }

    fn grants_in_identity_hash_order(&self) -> &[RemoteControlControllerGrant] {
        self.grants.as_slice()
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
        self.grants
            .insert(insert_at, grant)
            .map_err(|_| SetRemoteControlControllerGrantError::CapacityExhausted)?;
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

#[derive(Debug)]
pub struct FixedRemoteControlTargetAccessTable<const TARGET_SLOTS: usize> {
    accesses: Vec<RemoteControlTargetAccess, TARGET_SLOTS>,
}

impl<const TARGET_SLOTS: usize> Default for FixedRemoteControlTargetAccessTable<TARGET_SLOTS> {
    fn default() -> Self {
        Self {
            accesses: Vec::new(),
        }
    }
}

impl<const TARGET_SLOTS: usize> RemoteControlTargetAccessTable
    for FixedRemoteControlTargetAccessTable<TARGET_SLOTS>
{
    fn capacity(&self) -> usize {
        TARGET_SLOTS
    }

    fn len(&self) -> usize {
        self.accesses.len()
    }

    fn accesses_in_identity_hash_order(&self) -> &[RemoteControlTargetAccess] {
        self.accesses.as_slice()
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
        self.accesses
            .insert(insert_at, access)
            .map_err(|_| SetRemoteControlTargetAccessError::CapacityExhausted)?;
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
