use std::sync::Arc;

use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::sync::{oneshot, Mutex, OwnedMutexGuard};

use crate::identity::IdentityHash;
use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlTargetAccess, RemoteControlTargetIdentity,
    SetRemoteControlTargetAccessOutcome,
};

use super::node_facade::PrnsNodeHandle;
use super::{
    AssembledRemoteControl, ForgetRemoteControlTargetControlError,
    ForgetRemoteControlTargetServiceError, RemoteControlTargetAccessControl,
    RemoteControlTargetInventory, RemoteControlTargetInventoryControlError,
    RemoteControlTargetInventoryServiceError, ResolveRemoteControlTargetControlError,
    ResolveRemoteControlTargetServiceError, ResolvedRemoteControlTarget,
    SetRemoteControlTargetAccessControlError, SetRemoteControlTargetAccessServiceError,
};

const REMOTE_CONTROL_TARGET_ACCESS_QUEUE_DEPTH: usize = 1;

pub(super) enum RemoteControlTargetAccessCommand {
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
    ForgetTarget {
        target: RemoteControlTargetIdentity,
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
    pub(super) fn apply(self, remote_control: &mut AssembledRemoteControl) {
        match self {
            Self::Inventory { completion } => {
                if completion.is_closed() {
                    return;
                }
                let outcome = remote_control.target_inventory();
                let _completion = completion.send(outcome);
            }
            Self::ResolveTarget { target, completion } => {
                if completion.is_closed() {
                    return;
                }
                let outcome = remote_control.resolve_target(&target);
                let _completion = completion.send(outcome);
            }
            Self::SetTargetAccess { access, completion } => {
                if completion.is_closed() {
                    return;
                }
                let outcome = remote_control.set_target_access(access);
                let _completion = completion.send(outcome);
            }
            Self::ForgetTarget { target, completion } => {
                if completion.is_closed() {
                    return;
                }
                let outcome = remote_control.forget_target(&target);
                let _completion = completion.send(outcome);
            }
            Self::Snapshot { completion } => {
                if completion.is_closed() {
                    return;
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
    }
}

impl PrnsNodeHandle {
    pub(super) async fn snapshot_remote_control_target_accesses(
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
        target: RemoteControlTargetIdentity,
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
