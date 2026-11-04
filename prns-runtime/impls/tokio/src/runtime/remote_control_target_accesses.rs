use std::sync::Arc;

use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::sync::{oneshot, Mutex, OwnedMutexGuard};

use crate::identity::IdentityHash;
use crate::persistence::SnapshotRegion;
use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlControllerIdentity, RemoteControlTargetAccess,
    RemoteControlTargetAccessCard, SetRemoteControlTargetAccessOutcome,
};

use super::node_facade::{PrnsNodeHandle, RemoteControlAuthorizationPersistence};
use super::remote_control_pairing_persistence::{
    activate_target_access, prepare_target_access_set, RemoteControlAuthorizationPersistenceFailure,
};
use super::{
    AssembledRemoteControl, ForgetRemoteControlTargetControlError,
    ForgetRemoteControlTargetServiceError, RemoteControlTargetAccessControl,
    RemoteControlTargetInventory, RemoteControlTargetInventoryControlError,
    RemoteControlTargetInventoryServiceError, ResolveRemoteControlTargetControlError,
    ResolveRemoteControlTargetServiceError, ResolvedRemoteControlTarget,
    SetRemoteControlTargetAccessControlError, SetRemoteControlTargetAccessServiceError,
};

const REMOTE_CONTROL_TARGET_ACCESS_QUEUE_DEPTH: usize = 1;

pub use prns_runtime::runtime::SaveRemoteControlTargetAccessError;

pub(super) enum RemoteControlTargetAccessCommand {
    ControllerIdentity {
        completion: oneshot::Sender<
            Result<RemoteControlControllerIdentity, RemoteControlTargetInventoryServiceError>,
        >,
    },
    Inventory {
        completion: oneshot::Sender<
            Result<RemoteControlTargetInventory, RemoteControlTargetInventoryServiceError>,
        >,
    },
    ResolveTarget {
        target: IdentityHash,
        completion: oneshot::Sender<
            Result<ResolvedRemoteControlTarget, ResolveRemoteControlTargetServiceError>,
        >,
    },
    SetTargetAccess {
        access: RemoteControlTargetAccess,
        completion: oneshot::Sender<
            Result<SetRemoteControlTargetAccessOutcome, SetRemoteControlTargetAccessServiceError>,
        >,
    },
    SaveTargetAccess {
        access: RemoteControlTargetAccess,
        completion: oneshot::Sender<
            Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError>,
        >,
    },
    SaveTargetAccessCard {
        card: RemoteControlTargetAccessCard,
        completion: oneshot::Sender<
            Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError>,
        >,
    },
    ForgetTarget {
        target: IdentityHash,
        completion: oneshot::Sender<
            Result<ForgetRemoteControlTargetOutcome, ForgetRemoteControlTargetServiceError>,
        >,
    },
    Snapshot {
        completion: oneshot::Sender<
            Result<Option<std::vec::Vec<u8>>, crate::persistence::SnapshotSealError>,
        >,
    },
}

#[derive(Clone)]
pub(super) struct RemoteControlTargetAccessSender {
    commands: mpsc::Sender<RemoteControlTargetAccessCommand>,
    operation: Arc<Mutex<()>>,
}

pub(super) struct RemoteControlTargetAccessReceiver {
    commands: mpsc::Receiver<RemoteControlTargetAccessCommand>,
}

enum RemoteControlTargetAccessSubmissionError {
    Busy,
    NodeStopped,
}

pub(super) fn remote_control_target_access_lane() -> (
    RemoteControlTargetAccessSender,
    RemoteControlTargetAccessReceiver,
) {
    let (commands, receiver) = mpsc::channel(REMOTE_CONTROL_TARGET_ACCESS_QUEUE_DEPTH);
    (
        RemoteControlTargetAccessSender {
            commands,
            operation: Arc::new(Mutex::new(())),
        },
        RemoteControlTargetAccessReceiver { commands: receiver },
    )
}

impl RemoteControlTargetAccessSender {
    fn submit(
        &self,
        command: RemoteControlTargetAccessCommand,
    ) -> Result<OwnedMutexGuard<()>, RemoteControlTargetAccessSubmissionError> {
        let operation = self
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| RemoteControlTargetAccessSubmissionError::Busy)?;
        match self.commands.try_send(command) {
            Ok(()) => Ok(operation),
            Err(TrySendError::Full(_)) => Err(RemoteControlTargetAccessSubmissionError::Busy),
            Err(TrySendError::Closed(_)) => {
                Err(RemoteControlTargetAccessSubmissionError::NodeStopped)
            }
        }
    }

    async fn snapshot(
        &self,
        completion: oneshot::Sender<
            Result<Option<std::vec::Vec<u8>>, crate::persistence::SnapshotSealError>,
        >,
    ) -> Result<OwnedMutexGuard<()>, RemoteControlTargetAccessSubmissionError> {
        let operation = self.operation.clone().lock_owned().await;
        self.commands
            .send(RemoteControlTargetAccessCommand::Snapshot { completion })
            .await
            .map_err(|_| RemoteControlTargetAccessSubmissionError::NodeStopped)?;
        Ok(operation)
    }
}

impl RemoteControlTargetAccessReceiver {
    pub(super) async fn receive(&mut self) -> Option<RemoteControlTargetAccessCommand> {
        self.commands.recv().await
    }
}

impl RemoteControlTargetAccessCommand {
    pub(super) async fn apply(
        self,
        remote_control: &mut AssembledRemoteControl,
        persistence: Option<&RemoteControlAuthorizationPersistence>,
    ) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
        match self {
            Self::ControllerIdentity { completion } => {
                let identity = remote_control
                    .identities()
                    .map(|identities| *identities.controller())
                    .ok_or(RemoteControlTargetInventoryServiceError::Unavailable);
                let _completion = completion.send(identity);
            }
            Self::Inventory { completion } => {
                if completion.is_closed() {
                    return Ok(());
                }
                let outcome = remote_control.target_inventory();
                let _completion = completion.send(outcome);
            }
            Self::ResolveTarget { target, completion } => {
                if completion.is_closed() {
                    return Ok(());
                }
                let outcome = remote_control.resolve_target(&target);
                let _completion = completion.send(outcome);
            }
            Self::SetTargetAccess { access, completion } => {
                if completion.is_closed() {
                    return Ok(());
                }
                let outcome = remote_control.set_target_access(access);
                let _completion = completion.send(outcome);
            }
            Self::SaveTargetAccess { access, completion } => {
                let outcome = save_target_access(remote_control, persistence, access, true).await?;
                let _completion = completion.send(outcome);
            }
            Self::SaveTargetAccessCard { card, completion } => {
                let Some(controller) = remote_control
                    .identities()
                    .map(|identities| *identities.controller())
                else {
                    let _completion =
                        completion.send(Err(SaveRemoteControlTargetAccessError::Unavailable));
                    return Ok(());
                };
                let access = match card.into_access_for(&controller) {
                    Ok(access) => access,
                    Err(_) => {
                        let _completion = completion
                            .send(Err(SaveRemoteControlTargetAccessError::RecipientMismatch));
                        return Ok(());
                    }
                };
                let outcome =
                    save_target_access(remote_control, persistence, access, false).await?;
                let _completion = completion.send(outcome);
            }
            Self::ForgetTarget { target, completion } => {
                if completion.is_closed() {
                    return Ok(());
                }
                let outcome = remote_control.forget_target_by_hash(target);
                let _completion = completion.send(outcome);
            }
            Self::Snapshot { completion } => {
                if completion.is_closed() {
                    return Ok(());
                }
                let mut snapshot = std::vec![
                    0;
                    crate::persistence::remote_control_target_accesses_snapshot_capacity(
                        crate::remote_control::DEFAULT_MAX_REMOTE_CONTROL_TARGET_ACCESSES,
                    )
                ];
                let outcome = remote_control
                    .write_target_accesses_snapshot(&mut snapshot)
                    .map(|written| {
                        written.map(|written| {
                            snapshot.truncate(written);
                            snapshot
                        })
                    });
                let _completion = completion.send(outcome);
            }
        }
        Ok(())
    }
}

async fn save_target_access(
    remote_control: &mut AssembledRemoteControl,
    persistence: Option<&RemoteControlAuthorizationPersistence>,
    access: RemoteControlTargetAccess,
    allow_replacement: bool,
) -> Result<
    Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError>,
    RemoteControlAuthorizationPersistenceFailure,
> {
    let Some(persistence) = persistence else {
        return Ok(Err(
            SaveRemoteControlTargetAccessError::PersistenceUnavailable,
        ));
    };
    let transaction = persistence.begin().await?;
    let prepared = match prepare_target_access_set(remote_control, access) {
        Ok(prepared) => prepared,
        Err(error) => {
            transaction.finish().await?;
            return Ok(Err(error.into()));
        }
    };
    if !allow_replacement && prepared.is_replacement() {
        transaction.finish().await?;
        return Ok(Err(SaveRemoteControlTargetAccessError::Conflict));
    }
    let (outcome, mutation, projected) = prepared.into_parts();
    // Write even an unchanged entry: it may have been installed by the in-memory setter.
    if transaction
        .store(SnapshotRegion::RemoteControlTargetAccesses, projected)
        .await?
        .is_err()
    {
        // Definitive storage errors leave the previous published snapshot intact.
        // The transaction keeps ownership and retries confirmation after an uncertain
        // publish, so that case cannot reach here. Rewriting the live baseline would
        // incorrectly promote any access installed only by the volatile setter.
        transaction.finish().await?;
        return Ok(Err(SaveRemoteControlTargetAccessError::PersistenceFailed));
    }
    activate_target_access(remote_control, &mutation)?;
    transaction.finish().await?;
    Ok(Ok(outcome))
}

impl PrnsNodeHandle {
    /// Read this running service's public controller identity. No target needs to be saved first.
    pub async fn remote_control_controller_identity(
        &self,
    ) -> Result<RemoteControlControllerIdentity, RemoteControlTargetInventoryControlError> {
        let (completion, settled) = oneshot::channel();
        let _operation = self
            .remote_control_target_accesses
            .submit(RemoteControlTargetAccessCommand::ControllerIdentity { completion })
            .map_err(|error| match error {
                RemoteControlTargetAccessSubmissionError::Busy => {
                    RemoteControlTargetInventoryControlError::Busy
                }
                RemoteControlTargetAccessSubmissionError::NodeStopped => {
                    RemoteControlTargetInventoryControlError::NodeStopped
                }
            })?;
        settled
            .await
            .map_err(|_| RemoteControlTargetInventoryControlError::NodeStopped)?
            .map_err(Into::into)
    }

    /// Save an explicitly selected access change durably before making it active.
    /// Unlike the in-memory setter, this requires authorization persistence.
    /// Once admitted, cancellation does not cancel or roll back the save.
    pub async fn save_remote_control_target_access(
        &self,
        access: RemoteControlTargetAccess,
    ) -> Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError> {
        let (completion, settled) = oneshot::channel();
        self.save_target_access_command(
            RemoteControlTargetAccessCommand::SaveTargetAccess { access, completion },
            settled,
        )
        .await
    }

    /// Save a card addressed to this running controller, without replacing existing access.
    /// The caller must verify the card's source; a valid public card is not proof of a target grant.
    /// `Conflict` requires a separately reviewed replacement via `save_remote_control_target_access`.
    /// Once admitted, cancellation does not cancel or roll back the save.
    pub async fn save_remote_control_target_access_card(
        &self,
        card: RemoteControlTargetAccessCard,
    ) -> Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError> {
        let (completion, settled) = oneshot::channel();
        self.save_target_access_command(
            RemoteControlTargetAccessCommand::SaveTargetAccessCard { card, completion },
            settled,
        )
        .await
    }

    async fn save_target_access_command(
        &self,
        command: RemoteControlTargetAccessCommand,
        settled: oneshot::Receiver<
            Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError>,
        >,
    ) -> Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError> {
        let _operation = self
            .remote_control_target_accesses
            .submit(command)
            .map_err(|error| match error {
                RemoteControlTargetAccessSubmissionError::Busy => {
                    SaveRemoteControlTargetAccessError::Busy
                }
                RemoteControlTargetAccessSubmissionError::NodeStopped => {
                    SaveRemoteControlTargetAccessError::NodeStopped
                }
            })?;
        settled
            .await
            .map_err(|_| SaveRemoteControlTargetAccessError::NodeStopped)?
    }

    pub async fn snapshot_remote_control_target_accesses(
        &self,
    ) -> Result<Option<std::vec::Vec<u8>>, super::PrepareFlushError> {
        let (completion, settled) = oneshot::channel();
        let _operation = self
            .remote_control_target_accesses
            .snapshot(completion)
            .await
            .map_err(|error| match error {
                RemoteControlTargetAccessSubmissionError::Busy => {
                    super::PrepareFlushError::NodeStopped
                }
                RemoteControlTargetAccessSubmissionError::NodeStopped => {
                    super::PrepareFlushError::NodeStopped
                }
            })?;
        settled
            .await
            .map_err(|_| super::PrepareFlushError::NodeStopped)?
            .map_err(super::PrepareFlushError::AuthorizationSnapshot)
    }
}

#[cfg(test)]
mod tests;

impl RemoteControlTargetAccessControl for PrnsNodeHandle {
    async fn remote_control_target_inventory(
        &self,
    ) -> Result<RemoteControlTargetInventory, RemoteControlTargetInventoryControlError> {
        let (completion, settled) = oneshot::channel();
        let _operation = match self
            .remote_control_target_accesses
            .submit(RemoteControlTargetAccessCommand::Inventory { completion })
        {
            Ok(operation) => operation,
            Err(RemoteControlTargetAccessSubmissionError::Busy) => {
                return Err(RemoteControlTargetInventoryControlError::Busy)
            }
            Err(RemoteControlTargetAccessSubmissionError::NodeStopped) => {
                return Err(RemoteControlTargetInventoryControlError::NodeStopped)
            }
        };
        match settled.await {
            Ok(outcome) => outcome.map_err(Into::into),
            Err(_) => Err(RemoteControlTargetInventoryControlError::NodeStopped),
        }
    }

    async fn resolve_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> Result<ResolvedRemoteControlTarget, ResolveRemoteControlTargetControlError> {
        let (completion, settled) = oneshot::channel();
        let _operation = match self
            .remote_control_target_accesses
            .submit(RemoteControlTargetAccessCommand::ResolveTarget { target, completion })
        {
            Ok(operation) => operation,
            Err(RemoteControlTargetAccessSubmissionError::Busy) => {
                return Err(ResolveRemoteControlTargetControlError::Busy)
            }
            Err(RemoteControlTargetAccessSubmissionError::NodeStopped) => {
                return Err(ResolveRemoteControlTargetControlError::NodeStopped)
            }
        };
        match settled.await {
            Ok(outcome) => outcome.map_err(Into::into),
            Err(_) => Err(ResolveRemoteControlTargetControlError::NodeStopped),
        }
    }

    async fn set_remote_control_target_access(
        &self,
        access: RemoteControlTargetAccess,
    ) -> Result<SetRemoteControlTargetAccessOutcome, SetRemoteControlTargetAccessControlError> {
        let (completion, settled) = oneshot::channel();
        let _operation = match self
            .remote_control_target_accesses
            .submit(RemoteControlTargetAccessCommand::SetTargetAccess { access, completion })
        {
            Ok(operation) => operation,
            Err(RemoteControlTargetAccessSubmissionError::Busy) => {
                return Err(SetRemoteControlTargetAccessControlError::Busy)
            }
            Err(RemoteControlTargetAccessSubmissionError::NodeStopped) => {
                return Err(SetRemoteControlTargetAccessControlError::NodeStopped)
            }
        };
        match settled.await {
            Ok(outcome) => outcome.map_err(Into::into),
            Err(_) => Err(SetRemoteControlTargetAccessControlError::NodeStopped),
        }
    }

    async fn forget_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> Result<ForgetRemoteControlTargetOutcome, ForgetRemoteControlTargetControlError> {
        let (completion, settled) = oneshot::channel();
        let _operation = match self
            .remote_control_target_accesses
            .submit(RemoteControlTargetAccessCommand::ForgetTarget { target, completion })
        {
            Ok(operation) => operation,
            Err(RemoteControlTargetAccessSubmissionError::Busy) => {
                return Err(ForgetRemoteControlTargetControlError::Busy)
            }
            Err(RemoteControlTargetAccessSubmissionError::NodeStopped) => {
                return Err(ForgetRemoteControlTargetControlError::NodeStopped)
            }
        };
        match settled.await {
            Ok(outcome) => outcome.map_err(Into::into),
            Err(_) => Err(ForgetRemoteControlTargetControlError::NodeStopped),
        }
    }
}

// Durable admission is a separate capability from the volatile access-control
// trait. This adapter uses the existing serialized persistence transaction lane.
impl super::RemoteControlTargetAccessPersistence for PrnsNodeHandle {
    async fn remote_control_controller_identity(
        &self,
    ) -> Result<RemoteControlControllerIdentity, RemoteControlTargetInventoryControlError> {
        PrnsNodeHandle::remote_control_controller_identity(self).await
    }

    async fn save_remote_control_target_access(
        &self,
        access: RemoteControlTargetAccess,
    ) -> Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError> {
        PrnsNodeHandle::save_remote_control_target_access(self, access).await
    }

    async fn save_remote_control_target_access_card(
        &self,
        card: RemoteControlTargetAccessCard,
    ) -> Result<SetRemoteControlTargetAccessOutcome, SaveRemoteControlTargetAccessError> {
        PrnsNodeHandle::save_remote_control_target_access_card(self, card).await
    }
}
