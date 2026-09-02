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
    ForgetRemoteControlTargetOutcome, RemoteControlControllerGrant,
    RemoteControlControllerIdentity, RemoteControlPairingAttemptId, RemoteControlRequestSet,
    RemoteControlTargetAccess, RemoteControlTargetIdentity, RevokeRemoteControlControllerOutcome,
    SetRemoteControlControllerGrantOutcome, SetRemoteControlTargetAccessOutcome,
    DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS, DEFAULT_MAX_REMOTE_CONTROL_TARGET_ACCESSES,
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

enum ControllerGrantMutation {
    Added {
        controller: RemoteControlControllerIdentity,
    },
    Unchanged,
    Updated {
        previous: RemoteControlControllerGrant,
    },
}

enum TargetAccessMutation {
    Added {
        target_public_keys: IdentityPublicKeys,
    },
    Unchanged,
    Updated {
        previous: RemoteControlTargetAccess,
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
            | Journaled::RemoteControlControllerPairingConfirmationRequired(_)
            | Journaled::RemoteControlControllerPairingAuthorizationPersisted { .. }
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
                permitted_requests,
            } => {
                let access = match RemoteControlTargetAccess::new(
                    RemoteControlTargetIdentity::new(target_public_keys),
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
    let Some(persistence) = persistence else {
        return settle_controller_grant_persistence_failure(node, attempt_id).await;
    };
    let controller = *grant.controller();
    let mutation = match remote_control.set_controller_grant(grant) {
        Ok(SetRemoteControlControllerGrantOutcome::Added) => {
            ControllerGrantMutation::Added { controller }
        }
        Ok(SetRemoteControlControllerGrantOutcome::Unchanged) => ControllerGrantMutation::Unchanged,
        Ok(SetRemoteControlControllerGrantOutcome::Updated { previous }) => {
            ControllerGrantMutation::Updated { previous }
        }
        Err(_) => return settle_controller_grant_persistence_failure(node, attempt_id).await,
    };
    let snapshot = match controller_grants_snapshot(remote_control) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            rollback_controller_grant(remote_control, mutation)?;
            return Err(error);
        }
    };
    if persistence
        .store(SnapshotRegion::RemoteControlControllerGrants, snapshot)
        .await
        .is_err()
    {
        rollback_controller_grant(remote_control, mutation)?;
        return settle_controller_grant_persistence_failure(node, attempt_id).await;
    }
    let settled = settle_pairing_command(
        node,
        SettleRemoteControlTargetPairingAuthorization {
            attempt_id,
            persistence: RemoteControlTargetPairingAuthorizationPersistence::Persisted,
        },
    )
    .await
    .ok_or(RemoteControlAuthorizationPersistenceFailure::RuntimeState)?;
    match settled {
        Ok(RemoteControlTargetPairingFinalization::CompletionDispatched { .. }) => Ok(()),
        Ok(RemoteControlTargetPairingFinalization::AuthorizationRollbackRequired { .. }) => {
            rollback_controller_grant_durably(remote_control, persistence, mutation).await
        }
        Ok(RemoteControlTargetPairingFinalization::AuthorizationFailureRecorded { .. })
        | Err(_) => {
            rollback_controller_grant_durably(remote_control, persistence, mutation).await?;
            Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
        }
    }
}

async fn persist_target_access(
    remote_control: &mut AssembledRemoteControl,
    persistence: Option<&RemoteControlAuthorizationPersistence>,
    node: &PrnsNodeHandle,
    attempt_id: RemoteControlPairingAttemptId,
    access: RemoteControlTargetAccess,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    let Some(persistence) = persistence else {
        return settle_target_access_persistence_failure(node, attempt_id).await;
    };
    let target_public_keys = *access.target().public_keys();
    let mutation = match remote_control.set_target_access(access) {
        Ok(SetRemoteControlTargetAccessOutcome::Added) => {
            TargetAccessMutation::Added { target_public_keys }
        }
        Ok(SetRemoteControlTargetAccessOutcome::Unchanged) => TargetAccessMutation::Unchanged,
        Ok(SetRemoteControlTargetAccessOutcome::Updated { previous }) => {
            TargetAccessMutation::Updated { previous }
        }
        Err(_) => return settle_target_access_persistence_failure(node, attempt_id).await,
    };
    let snapshot = match target_accesses_snapshot(remote_control) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            rollback_target_access(remote_control, mutation)?;
            return Err(error);
        }
    };
    if persistence
        .store(SnapshotRegion::RemoteControlTargetAccesses, snapshot)
        .await
        .is_err()
    {
        rollback_target_access(remote_control, mutation)?;
        return settle_target_access_persistence_failure(node, attempt_id).await;
    }
    let settled = settle_pairing_command(
        node,
        SettleRemoteControlControllerPairingPersistence {
            attempt_id,
            persistence: RemoteControlControllerPairingPersistence::Persisted,
        },
    )
    .await
    .ok_or(RemoteControlAuthorizationPersistenceFailure::RuntimeState)?;
    match settled {
        Ok(RemoteControlControllerPairingFinalization::Completed { .. }) => Ok(()),
        Ok(RemoteControlControllerPairingFinalization::PersistenceFailureRecorded { .. })
        | Err(_) => Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState),
    }
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
        ControllerGrantMutation::Added { controller } => match remote_control
            .revoke_controller(&controller)
            .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
        {
            RevokeRemoteControlControllerOutcome::Revoked { .. } => Ok(()),
            RevokeRemoteControlControllerOutcome::NotFound => {
                Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
            }
        },
        ControllerGrantMutation::Unchanged => Ok(()),
        ControllerGrantMutation::Updated { previous } => match remote_control
            .set_controller_grant(previous)
            .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
        {
            SetRemoteControlControllerGrantOutcome::Updated { .. } => Ok(()),
            SetRemoteControlControllerGrantOutcome::Added
            | SetRemoteControlControllerGrantOutcome::Unchanged => {
                Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
            }
        },
    }
}

async fn rollback_controller_grant_durably(
    remote_control: &mut AssembledRemoteControl,
    persistence: &RemoteControlAuthorizationPersistence,
    mutation: ControllerGrantMutation,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    rollback_controller_grant(remote_control, mutation)?;
    let rollback = controller_grants_snapshot(remote_control)?;
    persistence
        .store(SnapshotRegion::RemoteControlControllerGrants, rollback)
        .await
        .map_err(|_| RemoteControlAuthorizationPersistenceFailure::DurableRollback)
}

fn rollback_target_access(
    remote_control: &mut AssembledRemoteControl,
    mutation: TargetAccessMutation,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    match mutation {
        TargetAccessMutation::Added { target_public_keys } => match remote_control
            .forget_target(&RemoteControlTargetIdentity::new(target_public_keys))
            .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
        {
            ForgetRemoteControlTargetOutcome::Forgotten { .. } => Ok(()),
            ForgetRemoteControlTargetOutcome::NotFound => {
                Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
            }
        },
        TargetAccessMutation::Unchanged => Ok(()),
        TargetAccessMutation::Updated { previous } => match remote_control
            .set_target_access(previous)
            .map_err(|_| RemoteControlAuthorizationPersistenceFailure::RuntimeState)?
        {
            SetRemoteControlTargetAccessOutcome::Updated { .. } => Ok(()),
            SetRemoteControlTargetAccessOutcome::Added
            | SetRemoteControlTargetAccessOutcome::Unchanged => {
                Err(RemoteControlAuthorizationPersistenceFailure::RuntimeState)
            }
        },
    }
}
