use tokio::sync::mpsc;

use crate::engine::{
    Journaled, RemoteControlControllerPairingFinalization,
    RemoteControlControllerPairingPersistence, RemoteControlTargetPairingAuthorizationPersistence,
    RemoteControlTargetPairingFinalization, SettleRemoteControlControllerPairingPersistence,
    SettleRemoteControlTargetPairingAuthorization, Settleable,
};
use crate::identity::IdentityPublicKeys;
use crate::persistence::{
    remote_control_controller_grants_snapshot_capacity,
    remote_control_target_accesses_snapshot_capacity, SnapshotRegion, SnapshotSealError,
};
use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlControllerAuthority,
    RemoteControlControllerGrant, RemoteControlControllerIdentity, RemoteControlPairingAttemptId,
    RemoteControlRequestSet, RemoteControlTargetAccess, RemoteControlTargetIdentity,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
    SetRemoteControlTargetAccessOutcome, DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS,
    DEFAULT_MAX_REMOTE_CONTROL_TARGET_ACCESSES,
};

use super::node_facade::{PrnsNodeHandle, RemoteControlAuthorizationPersistence};
use super::AssembledRemoteControl;

pub(super) enum RemoteControlPairingPersistenceCommand {
    ControllerGrant {
        attempt_id: RemoteControlPairingAttemptId,
        grant: RemoteControlControllerGrant,
    },
    TargetAccess {
        attempt_id: RemoteControlPairingAttemptId,
        target_public_keys: IdentityPublicKeys,
        authority: RemoteControlControllerAuthority,
        permitted_requests: RemoteControlRequestSet,
    },
}

#[derive(Clone)]
pub(super) struct RemoteControlPairingPersistenceSender {
    commands: mpsc::UnboundedSender<RemoteControlPairingPersistenceCommand>,
}

pub(super) struct RemoteControlPairingPersistenceReceiver {
    commands: mpsc::UnboundedReceiver<RemoteControlPairingPersistenceCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlAuthorizationPersistenceFailure {
    SnapshotUnavailable,
    SnapshotSeal(SnapshotSealError),
    RuntimeState,
    DurableRollback,
}

impl std::fmt::Display for RemoteControlAuthorizationPersistenceFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SnapshotUnavailable => {
                formatter.write_str("the remote-control service became unavailable")
            }
            Self::SnapshotSeal(SnapshotSealError::BufferTooShort) => {
                formatter.write_str("the authorization snapshot buffer was too short")
            }
            Self::RuntimeState => {
                formatter.write_str("the runtime rejected a required state transition")
            }
            Self::DurableRollback => {
                formatter.write_str("the authorization rollback could not be persisted")
            }
        }
    }
}

impl std::error::Error for RemoteControlAuthorizationPersistenceFailure {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ControllerGrantMutation {
    Added {
        desired: RemoteControlControllerGrant,
    },
    Unchanged,
    Updated {
        desired: RemoteControlControllerGrant,
        previous: RemoteControlControllerGrant,
    },
    Revoked {
        previous: RemoteControlControllerGrant,
    },
    NotFound,
}

pub(super) struct PreparedControllerGrantChange {
    mutation: ControllerGrantMutation,
    projected: Vec<u8>,
    rollback: Vec<u8>,
}

impl PreparedControllerGrantChange {
    pub(super) fn into_parts(self) -> (ControllerGrantMutation, Vec<u8>, Vec<u8>) {
        (self.mutation, self.projected, self.rollback)
    }

    pub(super) fn is_unchanged(&self) -> bool {
        matches!(
            self.mutation,
            ControllerGrantMutation::Unchanged | ControllerGrantMutation::NotFound
        )
    }

    pub(super) fn set_outcome(&self) -> Option<SetRemoteControlControllerGrantOutcome> {
        match self.mutation {
            ControllerGrantMutation::Added { .. } => {
                Some(SetRemoteControlControllerGrantOutcome::Added)
            }
            ControllerGrantMutation::Unchanged => {
                Some(SetRemoteControlControllerGrantOutcome::Unchanged)
            }
            ControllerGrantMutation::Updated { previous, .. } => {
                Some(SetRemoteControlControllerGrantOutcome::Updated { previous })
            }
            ControllerGrantMutation::Revoked { .. } | ControllerGrantMutation::NotFound => None,
        }
    }

    pub(super) fn revoke_outcome(&self) -> Option<RevokeRemoteControlControllerOutcome> {
        match self.mutation {
            ControllerGrantMutation::Revoked { previous } => {
                Some(RevokeRemoteControlControllerOutcome::Revoked { grant: previous })
            }
            ControllerGrantMutation::NotFound => {
                Some(RevokeRemoteControlControllerOutcome::NotFound)
            }
            ControllerGrantMutation::Added { .. }
            | ControllerGrantMutation::Unchanged
            | ControllerGrantMutation::Updated { .. } => None,
        }
    }
}

pub(super) fn prepare_controller_grant_set(
    remote_control: &mut AssembledRemoteControl,
    grant: RemoteControlControllerGrant,
) -> Result<PreparedControllerGrantChange, super::SetRemoteControlControllerGrantServiceError> {
    let mutation = match remote_control.set_controller_grant(grant)? {
        SetRemoteControlControllerGrantOutcome::Added => {
            ControllerGrantMutation::Added { desired: grant }
        }
        SetRemoteControlControllerGrantOutcome::Unchanged => ControllerGrantMutation::Unchanged,
        SetRemoteControlControllerGrantOutcome::Updated { previous } => {
            ControllerGrantMutation::Updated {
                desired: grant,
                previous,
            }
        }
    };
    prepare_controller_grant_change(remote_control, mutation)
        .map_err(|()| super::SetRemoteControlControllerGrantServiceError::Unavailable)
}

pub(super) fn prepare_controller_revocation(
    remote_control: &mut AssembledRemoteControl,
    controller: RemoteControlControllerIdentity,
) -> Result<PreparedControllerGrantChange, super::RevokeRemoteControlControllerServiceError> {
    let mutation = match remote_control.revoke_controller(&controller)? {
        RevokeRemoteControlControllerOutcome::Revoked { grant } => {
            ControllerGrantMutation::Revoked { previous: grant }
        }
        RevokeRemoteControlControllerOutcome::NotFound => ControllerGrantMutation::NotFound,
    };
    prepare_controller_grant_change(remote_control, mutation)
        .map_err(|()| super::RevokeRemoteControlControllerServiceError::Unavailable)
}

fn prepare_controller_grant_change(
    remote_control: &mut AssembledRemoteControl,
    mutation: ControllerGrantMutation,
) -> Result<PreparedControllerGrantChange, ()> {
    let projected = match controller_grants_snapshot(remote_control) {
        Ok(projected) => projected,
        Err(_) => {
            rollback_controller_grant(remote_control, mutation).map_err(|_| ())?;
            return Err(());
        }
    };
    rollback_controller_grant(remote_control, mutation).map_err(|_| ())?;
    let rollback = controller_grants_snapshot(remote_control).map_err(|_| ())?;
    Ok(PreparedControllerGrantChange {
        mutation,
        projected,
        rollback,
    })
}

pub(super) fn activate_controller_grant_change(
    remote_control: &mut AssembledRemoteControl,
    mutation: ControllerGrantMutation,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    match mutation {
        ControllerGrantMutation::Added { desired } => match remote_control
            .set_controller_grant(desired)
            .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
        {
            SetRemoteControlControllerGrantOutcome::Added => Ok(()),
            SetRemoteControlControllerGrantOutcome::Unchanged
            | SetRemoteControlControllerGrantOutcome::Updated { .. } => {
                Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
            }
        },
        ControllerGrantMutation::Updated { desired, previous } => match remote_control
            .set_controller_grant(desired)
            .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
        {
            SetRemoteControlControllerGrantOutcome::Updated { previous: replaced }
                if replaced == previous =>
            {
                Ok(())
            }
            SetRemoteControlControllerGrantOutcome::Added
            | SetRemoteControlControllerGrantOutcome::Unchanged
            | SetRemoteControlControllerGrantOutcome::Updated { .. } => {
                Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
            }
        },
        ControllerGrantMutation::Revoked { previous } => match remote_control
            .revoke_controller(previous.controller())
            .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
        {
            RevokeRemoteControlControllerOutcome::Revoked { grant } if grant == previous => Ok(()),
            RevokeRemoteControlControllerOutcome::Revoked { .. }
            | RevokeRemoteControlControllerOutcome::NotFound => {
                Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
            }
        },
        ControllerGrantMutation::Unchanged | ControllerGrantMutation::NotFound => Ok(()),
    }
}

pub(super) fn rollback_controller_grant_change(
    remote_control: &mut AssembledRemoteControl,
    mutation: ControllerGrantMutation,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    rollback_controller_grant(remote_control, mutation)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TargetAccessSpec {
    target_public_keys: IdentityPublicKeys,
    authority: RemoteControlControllerAuthority,
    permitted_requests: RemoteControlRequestSet,
}

impl TargetAccessSpec {
    fn from_access(access: &RemoteControlTargetAccess) -> Self {
        Self {
            target_public_keys: *access.target().public_keys(),
            authority: access.authority(),
            permitted_requests: *access.permitted_requests(),
        }
    }

    fn into_access(self) -> Result<RemoteControlTargetAccess, ()> {
        RemoteControlTargetAccess::new(
            RemoteControlTargetIdentity::new(self.target_public_keys),
            self.authority,
            self.permitted_requests,
        )
        .map_err(|_| ())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetAccessMutation {
    Added {
        desired: TargetAccessSpec,
    },
    Unchanged,
    Updated {
        desired: TargetAccessSpec,
        previous: TargetAccessSpec,
    },
}

pub(super) fn remote_control_pairing_persistence_lane() -> (
    RemoteControlPairingPersistenceSender,
    RemoteControlPairingPersistenceReceiver,
) {
    let (commands, receiver) = mpsc::unbounded_channel();
    (
        RemoteControlPairingPersistenceSender { commands },
        RemoteControlPairingPersistenceReceiver { commands: receiver },
    )
}

impl RemoteControlPairingPersistenceSender {
    pub(super) fn observe(&self, journaled: &Journaled<'_>) {
        let command = match journaled {
            Journaled::RemoteControlTargetPairingAuthorizationRequired { attempt_id, grant } => {
                RemoteControlPairingPersistenceCommand::ControllerGrant {
                    attempt_id: *attempt_id,
                    grant: *grant,
                }
            }
            Journaled::RemoteControlControllerPairingPersistenceRequired(pairing) => {
                RemoteControlPairingPersistenceCommand::TargetAccess {
                    attempt_id: pairing.attempt_id(),
                    target_public_keys: *pairing.access().target().public_keys(),
                    authority: pairing.access().authority(),
                    permitted_requests: *pairing.access().permitted_requests(),
                }
            }
            Journaled::AnnounceHeldDropped { .. }
            | Journaled::Delivered(_)
            | Journaled::CommandSettled { .. }
            | Journaled::PersistenceFlushed { .. }
            | Journaled::PersistenceFlushFailed { .. }
            | Journaled::SelfRatchetRotated { .. }
            | Journaled::AnnounceHeard { .. }
            | Journaled::LinkEstablished(_)
            | Journaled::PeerIdentified { .. }
            | Journaled::RequestReceived { .. }
            | Journaled::ResponseReceived { .. }
            | Journaled::ResponseSegmentReceived { .. }
            | Journaled::ChannelMessageReceived { .. }
            | Journaled::LinkClosed { .. }
            | Journaled::LinkInterfaceMismatch { .. }
            | Journaled::ResourceReceived { .. }
            | Journaled::ResourceFailed { .. }
            | Journaled::ResourceSegmentReceived { .. }
            | Journaled::ResourceAssembled { .. }
            | Journaled::RouteRemoved { .. }
            | Journaled::RemoteControlPairingExpired { .. }
            | Journaled::RemoteControlPairingAvailabilityObserved(_)
            | Journaled::RemoteControlTargetPairingConfirmationRequired(_)
            | Journaled::RemoteControlTargetPairingControllerCommitted { .. }
            | Journaled::RemoteControlTargetPairingAuthorizationPersisted { .. }
            | Journaled::RemoteControlTargetPairingExpiredDuringAuthorization { .. }
            | Journaled::RemoteControlControllerPairingConfirmationRequired(_)
            | Journaled::RemoteControlControllerPairingAuthorizationPersisted { .. }
            | Journaled::RemoteControlControllerPairingAuthorizationPersistenceFailed { .. }
            | Journaled::RemoteControlControllerPairingExpired { .. }
            | Journaled::RemoteControlControllerPairingLinkClosed { .. }
            | Journaled::RemoteControlTargetPairingExpired { .. }
            | Journaled::RemoteControlTargetPairingLinkClosed { .. }
            | Journaled::RemoteControlTargetPairingCompletionRetentionExpired { .. }
            | Journaled::RemoteControlTargetPairingCompletionLinkClosed { .. }
            | Journaled::RemoteControlPairingExpiryFailed { .. } => return,
        };
        let _submitted = self.commands.send(command);
    }
}

impl RemoteControlPairingPersistenceReceiver {
    pub(super) async fn receive(&mut self) -> Option<RemoteControlPairingPersistenceCommand> {
        self.commands.recv().await
    }
}

impl RemoteControlPairingPersistenceCommand {
    pub(super) async fn apply(
        self,
        remote_control: &mut AssembledRemoteControl,
        persistence: Option<&RemoteControlAuthorizationPersistence>,
        node: &PrnsNodeHandle,
    ) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
        match self {
            Self::ControllerGrant { attempt_id, grant } => {
                persist_controller_grant(remote_control, persistence, node, attempt_id, grant).await
            }
            Self::TargetAccess {
                attempt_id,
                target_public_keys,
                authority,
                permitted_requests,
            } => {
                let access = match RemoteControlTargetAccess::new(
                    RemoteControlTargetIdentity::new(target_public_keys),
                    authority,
                    permitted_requests,
                ) {
                    Ok(access) => access,
                    Err(_) => {
                        return Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
                    }
                };
                persist_target_access(remote_control, persistence, node, attempt_id, access).await
            }
        }
    }
}

async fn persist_controller_grant(
    remote_control: &mut AssembledRemoteControl,
    persistence: Option<&RemoteControlAuthorizationPersistence>,
    node: &PrnsNodeHandle,
    attempt_id: RemoteControlPairingAttemptId,
    grant: RemoteControlControllerGrant,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    let prepared = match prepare_controller_grant_set(remote_control, grant) {
        Ok(prepared) => prepared,
        Err(_) => return settle_controller_grant_persistence_failure(node, attempt_id).await,
    };
    let unchanged = prepared.is_unchanged();
    let (mutation, projected, rollback) = prepared.into_parts();
    if persistence.is_none() && !unchanged {
        return settle_controller_grant_persistence_failure(node, attempt_id).await;
    }
    if let Some(persistence) = persistence {
        if persistence
            .store(SnapshotRegion::RemoteControlControllerGrants, projected)
            .await
            .is_err()
        {
            restore_controller_grants_snapshot(persistence, rollback).await?;
            return settle_controller_grant_persistence_failure(node, attempt_id).await;
        }
    }
    if activate_controller_grant_change(remote_control, mutation).is_err() {
        if let Some(persistence) = persistence {
            restore_controller_grants_snapshot(persistence, rollback).await?;
        }
        return settle_controller_grant_persistence_failure(node, attempt_id).await;
    }
    let settled = settle_pairing_command(
        node,
        SettleRemoteControlTargetPairingAuthorization {
            attempt_id,
            persistence: RemoteControlTargetPairingAuthorizationPersistence::Persisted,
        },
    )
    .await;
    let Some(settled) = settled else {
        rollback_controller_grant(remote_control, mutation)?;
        if let Some(persistence) = persistence {
            restore_controller_grants_snapshot(persistence, rollback).await?;
        }
        return Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState);
    };
    match settled {
        Ok(RemoteControlTargetPairingFinalization::CompletionDispatched { .. }) => Ok(()),
        Ok(RemoteControlTargetPairingFinalization::AuthorizationRollbackRequired { .. }) => {
            rollback_controller_grant(remote_control, mutation)?;
            if let Some(persistence) = persistence {
                restore_controller_grants_snapshot(persistence, rollback).await?;
            }
            Ok(())
        }
        Ok(RemoteControlTargetPairingFinalization::AuthorizationFailureRecorded { .. })
        | Err(_) => {
            rollback_controller_grant(remote_control, mutation)?;
            if let Some(persistence) = persistence {
                restore_controller_grants_snapshot(persistence, rollback).await?;
            }
            Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
        }
    }
}

pub(super) async fn restore_controller_grants_snapshot(
    persistence: &RemoteControlAuthorizationPersistence,
    rollback: Vec<u8>,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    persistence
        .store(SnapshotRegion::RemoteControlControllerGrants, rollback)
        .await
        .map_err(|_| RemoteControlAuthorizationPersistenceFailure::DurableRollback)
}

async fn persist_target_access(
    remote_control: &mut AssembledRemoteControl,
    persistence: Option<&RemoteControlAuthorizationPersistence>,
    node: &PrnsNodeHandle,
    attempt_id: RemoteControlPairingAttemptId,
    access: RemoteControlTargetAccess,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    let desired = TargetAccessSpec::from_access(&access);
    let mutation = match remote_control.set_target_access(access) {
        Ok(SetRemoteControlTargetAccessOutcome::Added) => TargetAccessMutation::Added { desired },
        Ok(SetRemoteControlTargetAccessOutcome::Unchanged) => TargetAccessMutation::Unchanged,
        Ok(SetRemoteControlTargetAccessOutcome::Updated { previous }) => {
            TargetAccessMutation::Updated {
                desired,
                previous: TargetAccessSpec::from_access(&previous),
            }
        }
        Err(_) => return settle_target_access_persistence_failure(node, attempt_id).await,
    };
    let projected = match target_accesses_snapshot(remote_control) {
        Ok(projected) => projected,
        Err(error) => {
            rollback_target_access(remote_control, mutation)?;
            return Err(error);
        }
    };
    rollback_target_access(remote_control, mutation)?;
    let rollback = target_accesses_snapshot(remote_control)?;
    if persistence.is_none() && mutation != TargetAccessMutation::Unchanged {
        return settle_target_access_persistence_failure(node, attempt_id).await;
    }
    if let Some(persistence) = persistence {
        if persistence
            .store(SnapshotRegion::RemoteControlTargetAccesses, projected)
            .await
            .is_err()
        {
            restore_target_accesses_snapshot(persistence, rollback).await?;
            return settle_target_access_persistence_failure(node, attempt_id).await;
        }
    }
    if activate_target_access(remote_control, &mutation).is_err() {
        if let Some(persistence) = persistence {
            restore_target_accesses_snapshot(persistence, rollback).await?;
        }
        return settle_target_access_persistence_failure(node, attempt_id).await;
    }
    let settled = settle_pairing_command(
        node,
        SettleRemoteControlControllerPairingPersistence {
            attempt_id,
            persistence: RemoteControlControllerPairingPersistence::Persisted,
        },
    )
    .await;
    let Some(settled) = settled else {
        rollback_target_access(remote_control, mutation)?;
        if let Some(persistence) = persistence {
            restore_target_accesses_snapshot(persistence, rollback).await?;
        }
        return Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState);
    };
    match settled {
        Ok(RemoteControlControllerPairingFinalization::Completed { .. }) => Ok(()),
        Ok(RemoteControlControllerPairingFinalization::PersistenceFailureRecorded { .. })
        | Err(_) => {
            rollback_target_access(remote_control, mutation)?;
            if let Some(persistence) = persistence {
                restore_target_accesses_snapshot(persistence, rollback).await?;
            }
            Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
        }
    }
}

async fn restore_target_accesses_snapshot(
    persistence: &RemoteControlAuthorizationPersistence,
    rollback: Vec<u8>,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    persistence
        .store(SnapshotRegion::RemoteControlTargetAccesses, rollback)
        .await
        .map_err(|_| RemoteControlAuthorizationPersistenceFailure::DurableRollback)
}

async fn settle_controller_grant_persistence_failure(
    node: &PrnsNodeHandle,
    attempt_id: RemoteControlPairingAttemptId,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    let settled = settle_pairing_command(
        node,
        SettleRemoteControlTargetPairingAuthorization {
            attempt_id,
            persistence: RemoteControlTargetPairingAuthorizationPersistence::Failed,
        },
    )
    .await
    .ok_or(RemoteControlAuthorizationPersistenceFailure::RuntimeState)?;
    match settled {
        Ok(RemoteControlTargetPairingFinalization::AuthorizationFailureRecorded { .. }) => Ok(()),
        Ok(
            RemoteControlTargetPairingFinalization::CompletionDispatched { .. }
            | RemoteControlTargetPairingFinalization::AuthorizationRollbackRequired { .. },
        )
        | Err(_) => Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState),
    }
}

async fn settle_target_access_persistence_failure(
    node: &PrnsNodeHandle,
    attempt_id: RemoteControlPairingAttemptId,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    let settled = settle_pairing_command(
        node,
        SettleRemoteControlControllerPairingPersistence {
            attempt_id,
            persistence: RemoteControlControllerPairingPersistence::Failed,
        },
    )
    .await
    .ok_or(RemoteControlAuthorizationPersistenceFailure::RuntimeState)?;
    match settled {
        Ok(RemoteControlControllerPairingFinalization::PersistenceFailureRecorded { .. }) => Ok(()),
        Ok(RemoteControlControllerPairingFinalization::Completed { .. }) | Err(_) => {
            Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
        }
    }
}

async fn settle_pairing_command<C>(
    node: &PrnsNodeHandle,
    command: C,
) -> Option<Result<C::Success, C::Failure>>
where
    C: Settleable,
{
    C::from_settlement(node.settle(command.into_command()).await?)
}

fn controller_grants_snapshot(
    remote_control: &AssembledRemoteControl,
) -> Result<Vec<u8>, RemoteControlAuthorizationPersistenceFailure> {
    let mut snapshot = vec![
        0;
        remote_control_controller_grants_snapshot_capacity(
            DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS,
        )
    ];
    let written = remote_control
        .write_controller_grants_snapshot(&mut snapshot)
        .map_err(RemoteControlAuthorizationPersistenceFailure::SnapshotSeal)?
        .ok_or(RemoteControlAuthorizationPersistenceFailure::SnapshotUnavailable)?;
    snapshot.truncate(written);
    Ok(snapshot)
}

fn target_accesses_snapshot(
    remote_control: &AssembledRemoteControl,
) -> Result<Vec<u8>, RemoteControlAuthorizationPersistenceFailure> {
    let mut snapshot = vec![
        0;
        remote_control_target_accesses_snapshot_capacity(
            DEFAULT_MAX_REMOTE_CONTROL_TARGET_ACCESSES,
        )
    ];
    let written = remote_control
        .write_target_accesses_snapshot(&mut snapshot)
        .map_err(RemoteControlAuthorizationPersistenceFailure::SnapshotSeal)?
        .ok_or(RemoteControlAuthorizationPersistenceFailure::SnapshotUnavailable)?;
    snapshot.truncate(written);
    Ok(snapshot)
}

fn rollback_controller_grant(
    remote_control: &mut AssembledRemoteControl,
    mutation: ControllerGrantMutation,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    match mutation {
        ControllerGrantMutation::Added { desired } => match remote_control
            .revoke_controller(desired.controller())
            .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
        {
            RevokeRemoteControlControllerOutcome::Revoked { grant } if grant == desired => Ok(()),
            RevokeRemoteControlControllerOutcome::Revoked { .. }
            | RevokeRemoteControlControllerOutcome::NotFound => {
                Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
            }
        },
        ControllerGrantMutation::Revoked { previous } => match remote_control
            .set_controller_grant(previous)
            .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
        {
            SetRemoteControlControllerGrantOutcome::Added => Ok(()),
            SetRemoteControlControllerGrantOutcome::Unchanged
            | SetRemoteControlControllerGrantOutcome::Updated { .. } => {
                Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
            }
        },
        ControllerGrantMutation::NotFound | ControllerGrantMutation::Unchanged => Ok(()),
        ControllerGrantMutation::Updated { desired, previous } => match remote_control
            .set_controller_grant(previous)
            .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
        {
            SetRemoteControlControllerGrantOutcome::Updated { previous: replaced }
                if replaced == desired =>
            {
                Ok(())
            }
            SetRemoteControlControllerGrantOutcome::Added
            | SetRemoteControlControllerGrantOutcome::Unchanged
            | SetRemoteControlControllerGrantOutcome::Updated { .. } => {
                Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
            }
        },
    }
}

fn activate_target_access(
    remote_control: &mut AssembledRemoteControl,
    mutation: &TargetAccessMutation,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    match mutation {
        TargetAccessMutation::Added { desired } => {
            let access = desired
                .into_access()
                .map_err(|()| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?;
            match remote_control
                .set_target_access(access)
                .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
            {
                SetRemoteControlTargetAccessOutcome::Added => Ok(()),
                SetRemoteControlTargetAccessOutcome::Unchanged
                | SetRemoteControlTargetAccessOutcome::Updated { .. } => {
                    Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
                }
            }
        }
        TargetAccessMutation::Unchanged => Ok(()),
        TargetAccessMutation::Updated { desired, previous } => {
            let access = desired
                .into_access()
                .map_err(|()| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?;
            match remote_control
                .set_target_access(access)
                .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
            {
                SetRemoteControlTargetAccessOutcome::Updated { previous: replaced }
                    if TargetAccessSpec::from_access(&replaced) == *previous =>
                {
                    Ok(())
                }
                SetRemoteControlTargetAccessOutcome::Added
                | SetRemoteControlTargetAccessOutcome::Unchanged
                | SetRemoteControlTargetAccessOutcome::Updated { .. } => {
                    Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
                }
            }
        }
    }
}

fn rollback_target_access(
    remote_control: &mut AssembledRemoteControl,
    mutation: TargetAccessMutation,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    match mutation {
        TargetAccessMutation::Added { desired } => {
            let target = RemoteControlTargetIdentity::new(desired.target_public_keys);
            match remote_control
                .forget_target(&target)
                .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
            {
                ForgetRemoteControlTargetOutcome::Forgotten { access }
                    if TargetAccessSpec::from_access(&access) == desired =>
                {
                    Ok(())
                }
                ForgetRemoteControlTargetOutcome::Forgotten { .. }
                | ForgetRemoteControlTargetOutcome::NotFound => {
                    Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
                }
            }
        }
        TargetAccessMutation::Unchanged => Ok(()),
        TargetAccessMutation::Updated { desired, previous } => {
            let previous = previous
                .into_access()
                .map_err(|()| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?;
            match remote_control
                .set_target_access(previous)
                .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
            {
                SetRemoteControlTargetAccessOutcome::Updated { previous: replaced }
                    if TargetAccessSpec::from_access(&replaced) == desired =>
                {
                    Ok(())
                }
                SetRemoteControlTargetAccessOutcome::Added
                | SetRemoteControlTargetAccessOutcome::Unchanged
                | SetRemoteControlTargetAccessOutcome::Updated { .. } => {
                    Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::EngineState;
    use crate::remote_control::{
        RemoteControlControllerGrantTable, RemoteControlRequestKind,
        RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
    };
    use crate::storage::GrowableHeap;

    use super::*;

    fn remote_control() -> AssembledRemoteControl {
        let mut engine = EngineState::<GrowableHeap>::default();
        crate::runtime::configure_remote_control_service(
            &mut engine,
            super::super::node_facade::test_remote_control_service(),
        )
        .expect("RemoteControl fits growable storage")
    }

    #[test]
    fn controller_grant_prepare_is_inert_until_activation_and_exactly_reversible() {
        let mut remote_control = remote_control();
        let grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );

        let prepared = prepare_controller_grant_set(&mut remote_control, grant).unwrap();
        assert_eq!(
            prepared.set_outcome(),
            Some(SetRemoteControlControllerGrantOutcome::Added)
        );
        assert!(remote_control.controller_grants().unwrap().is_empty());
        let (mutation, projected, rollback) = prepared.into_parts();
        assert_ne!(projected, rollback);

        activate_controller_grant_change(&mut remote_control, mutation).unwrap();
        assert_eq!(
            remote_control
                .controller_grants()
                .unwrap()
                .grants_in_identity_hash_order(),
            &[grant]
        );
        rollback_controller_grant_change(&mut remote_control, mutation).unwrap();
        assert!(remote_control.controller_grants().unwrap().is_empty());
    }

    #[test]
    fn controller_revocation_prepare_is_inert_until_activation_and_exactly_reversible() {
        let mut remote_control = remote_control();
        let grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        assert_eq!(
            remote_control.set_controller_grant(grant),
            Ok(SetRemoteControlControllerGrantOutcome::Added)
        );

        let prepared = prepare_controller_revocation(&mut remote_control, *grant.controller())
            .expect("revocation can be prepared");
        assert_eq!(
            prepared.revoke_outcome(),
            Some(RevokeRemoteControlControllerOutcome::Revoked { grant })
        );
        assert_eq!(
            remote_control
                .controller_grants()
                .unwrap()
                .grants_in_identity_hash_order(),
            &[grant]
        );
        let (mutation, projected, rollback) = prepared.into_parts();
        assert_ne!(projected, rollback);

        activate_controller_grant_change(&mut remote_control, mutation).unwrap();
        assert!(remote_control.controller_grants().unwrap().is_empty());
        rollback_controller_grant_change(&mut remote_control, mutation).unwrap();
        assert_eq!(
            remote_control
                .controller_grants()
                .unwrap()
                .grants_in_identity_hash_order(),
            &[grant]
        );
    }
}
