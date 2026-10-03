use super::node_facade::AuthorizationTransaction;
use std::sync::Arc;

use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::sync::{oneshot, Mutex, OwnedMutexGuard};

use crate::persistence::SnapshotRegion;
use crate::remote_control::{
    RemoteControlAuthorizeControllerOutcome, RemoteControlControllerGrant,
    RemoteControlControllerIdentity, RemoteControlResponse, RemoteControlRevokeControllerOutcome,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
};

use super::node_facade::PrnsNodeHandle;
use super::node_facade::RemoteControlAuthorizationPersistence;
use super::remote_control_pairing_persistence::{
    activate_controller_grant_change, prepare_controller_grant_set, prepare_controller_revocation,
    restore_controller_grants_snapshot, RemoteControlAuthorizationPersistenceFailure,
};
use super::request_endpoints::RespondToken;
use super::{
    AssembledRemoteControl, RemoteControlControllerGrantControl,
    RevokeRemoteControlControllerControlError, RevokeRemoteControlControllerServiceError,
    SetRemoteControlControllerGrantControlError, SetRemoteControlControllerGrantServiceError,
};

const REMOTE_CONTROL_CONTROLLER_GRANT_QUEUE_DEPTH: usize = 1;

pub(super) enum RemoteControlControllerGrantCommand {
    SetControllerGrant {
        grant: RemoteControlControllerGrant,
        completion: oneshot::Sender<
            Result<
                SetRemoteControlControllerGrantOutcome,
                SetRemoteControlControllerGrantServiceError,
            >,
        >,
    },
    RevokeController {
        controller: RemoteControlControllerIdentity,
        completion: oneshot::Sender<
            Result<RevokeRemoteControlControllerOutcome, RevokeRemoteControlControllerServiceError>,
        >,
    },
    AuthorizeControllerAndRespond {
        grant: RemoteControlControllerGrant,
        responder: RespondToken,
        node: PrnsNodeHandle,
        completion: oneshot::Sender<()>,
    },
    RevokeControllerAndRespond {
        controller: RemoteControlControllerIdentity,
        responder: RespondToken,
        node: PrnsNodeHandle,
        completion: oneshot::Sender<()>,
    },
    Snapshot {
        completion: oneshot::Sender<
            Result<Option<std::vec::Vec<u8>>, crate::persistence::SnapshotSealError>,
        >,
    },
}

#[derive(Clone)]
pub(super) struct RemoteControlControllerGrantSender {
    commands: mpsc::Sender<RemoteControlControllerGrantCommand>,
    operation: Arc<Mutex<()>>,
}

pub(super) struct RemoteControlControllerGrantReceiver {
    commands: mpsc::Receiver<RemoteControlControllerGrantCommand>,
}

enum RemoteControlControllerGrantSubmissionError {
    Busy,
    NodeStopped,
}

pub(super) fn remote_control_controller_grant_lane() -> (
    RemoteControlControllerGrantSender,
    RemoteControlControllerGrantReceiver,
) {
    let (commands, receiver) = mpsc::channel(REMOTE_CONTROL_CONTROLLER_GRANT_QUEUE_DEPTH);
    (
        RemoteControlControllerGrantSender {
            commands,
            operation: Arc::new(Mutex::new(())),
        },
        RemoteControlControllerGrantReceiver { commands: receiver },
    )
}

impl RemoteControlControllerGrantSender {
    fn submit(
        &self,
        command: RemoteControlControllerGrantCommand,
    ) -> Result<OwnedMutexGuard<()>, RemoteControlControllerGrantSubmissionError> {
        let operation = self
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| RemoteControlControllerGrantSubmissionError::Busy)?;
        match self.commands.try_send(command) {
            Ok(()) => Ok(operation),
            Err(TrySendError::Full(_)) => Err(RemoteControlControllerGrantSubmissionError::Busy),
            Err(TrySendError::Closed(_)) => {
                Err(RemoteControlControllerGrantSubmissionError::NodeStopped)
            }
        }
    }

    async fn snapshot(
        &self,
        completion: oneshot::Sender<
            Result<Option<std::vec::Vec<u8>>, crate::persistence::SnapshotSealError>,
        >,
    ) -> Result<OwnedMutexGuard<()>, RemoteControlControllerGrantSubmissionError> {
        let operation = self.operation.clone().lock_owned().await;
        self.commands
            .send(RemoteControlControllerGrantCommand::Snapshot { completion })
            .await
            .map_err(|_| RemoteControlControllerGrantSubmissionError::NodeStopped)?;
        Ok(operation)
    }
}

impl RemoteControlControllerGrantReceiver {
    pub(super) async fn receive(&mut self) -> Option<RemoteControlControllerGrantCommand> {
        self.commands.recv().await
    }
}

impl RemoteControlControllerGrantCommand {
    pub(super) async fn apply(
        self,
        remote_control: &mut AssembledRemoteControl,
        persistence: Option<&RemoteControlAuthorizationPersistence>,
    ) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
        if matches!(&self, Self::Snapshot { .. }) {
            return self.apply_owned(remote_control, None).await;
        }
        match &self {
            Self::SetControllerGrant { completion, .. } if completion.is_closed() => return Ok(()),
            Self::RevokeController { completion, .. } if completion.is_closed() => return Ok(()),
            _ => {}
        }
        let transaction = match persistence {
            Some(persistence) => Some(persistence.begin().await?),
            None => None,
        };
        self.apply_owned(remote_control, transaction.as_ref())
            .await?;
        if let Some(transaction) = transaction {
            transaction.finish().await?;
        }
        Ok(())
    }

    async fn apply_owned(
        self,
        remote_control: &mut AssembledRemoteControl,
        persistence: Option<&AuthorizationTransaction>,
    ) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
        match self {
            Self::SetControllerGrant { grant, completion } => {
                let prepared = match prepare_controller_grant_set(remote_control, grant) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let _completion = completion.send(Err(error));
                        return Ok(());
                    }
                };
                let Some(outcome) = prepared.set_outcome() else {
                    let _completion = completion.send(Err(
                        SetRemoteControlControllerGrantServiceError::Unavailable,
                    ));
                    return Ok(());
                };
                if prepared.is_unchanged() {
                    let _completion = completion.send(Ok(outcome));
                    return Ok(());
                }
                let Some(persistence) = persistence else {
                    let _completion = completion.send(Err(
                        SetRemoteControlControllerGrantServiceError::Unavailable,
                    ));
                    return Ok(());
                };
                let (mutation, projected, rollback) = prepared.into_parts();
                if persistence
                    .store(SnapshotRegion::RemoteControlControllerGrants, projected)
                    .await?
                    .is_err()
                {
                    restore_controller_grants_snapshot(persistence, rollback).await?;
                    let _completion = completion.send(Err(
                        SetRemoteControlControllerGrantServiceError::Unavailable,
                    ));
                    return Ok(());
                }
                activate_controller_grant_change(remote_control, mutation)?;
                let _completion = completion.send(Ok(outcome));
            }
            Self::RevokeController {
                controller,
                completion,
            } => {
                let prepared = match prepare_controller_revocation(remote_control, controller) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let _completion = completion.send(Err(error));
                        return Ok(());
                    }
                };
                let Some(outcome) = prepared.revoke_outcome() else {
                    let _completion = completion
                        .send(Err(RevokeRemoteControlControllerServiceError::Unavailable));
                    return Ok(());
                };
                if prepared.is_unchanged() {
                    let _completion = completion.send(Ok(outcome));
                    return Ok(());
                }
                let Some(persistence) = persistence else {
                    let _completion = completion
                        .send(Err(RevokeRemoteControlControllerServiceError::Unavailable));
                    return Ok(());
                };
                let (mutation, projected, rollback) = prepared.into_parts();
                if persistence
                    .store(SnapshotRegion::RemoteControlControllerGrants, projected)
                    .await?
                    .is_err()
                {
                    restore_controller_grants_snapshot(persistence, rollback).await?;
                    let _completion = completion
                        .send(Err(RevokeRemoteControlControllerServiceError::Unavailable));
                    return Ok(());
                }
                activate_controller_grant_change(remote_control, mutation)?;
                let _completion = completion.send(Ok(outcome));
            }
            Self::AuthorizeControllerAndRespond {
                grant,
                responder,
                node,
                completion,
            } => {
                let prepared = match prepare_controller_grant_set(remote_control, grant) {
                    Ok(prepared) => prepared,
                    Err(SetRemoteControlControllerGrantServiceError::CapacityExhausted) => {
                        respond_to_remote_controller_grant(
                            &node,
                            responder,
                            RemoteControlResponse::AuthorizeController(
                                RemoteControlAuthorizeControllerOutcome::CapacityExhausted,
                            ),
                        )
                        .await;
                        let _completed = completion.send(());
                        return Ok(());
                    }
                    Err(
                        SetRemoteControlControllerGrantServiceError::Unavailable
                        | SetRemoteControlControllerGrantServiceError::TransactionInProgress,
                    ) => {
                        respond_to_remote_controller_grant(
                            &node,
                            responder,
                            RemoteControlResponse::AuthorizeController(
                                RemoteControlAuthorizeControllerOutcome::Failed,
                            ),
                        )
                        .await;
                        let _completed = completion.send(());
                        return Ok(());
                    }
                };
                let success = RemoteControlResponse::AuthorizeController(
                    RemoteControlAuthorizeControllerOutcome::Applied,
                );
                let failure = RemoteControlResponse::AuthorizeController(
                    RemoteControlAuthorizeControllerOutcome::Failed,
                );
                apply_remote_controller_grant_transaction(
                    remote_control,
                    persistence,
                    &node,
                    responder,
                    prepared,
                    success,
                    failure,
                )
                .await?;
                let _completed = completion.send(());
            }
            Self::RevokeControllerAndRespond {
                controller,
                responder,
                node,
                completion,
            } => {
                let prepared = match prepare_controller_revocation(remote_control, controller) {
                    Ok(prepared) => prepared,
                    Err(
                        RevokeRemoteControlControllerServiceError::Unavailable
                        | RevokeRemoteControlControllerServiceError::TransactionInProgress,
                    ) => {
                        respond_to_remote_controller_grant(
                            &node,
                            responder,
                            RemoteControlResponse::RevokeController(
                                RemoteControlRevokeControllerOutcome::Failed,
                            ),
                        )
                        .await;
                        let _completed = completion.send(());
                        return Ok(());
                    }
                };
                let success = match prepared.revoke_outcome() {
                    Some(RevokeRemoteControlControllerOutcome::Revoked { .. }) => {
                        RemoteControlResponse::RevokeController(
                            RemoteControlRevokeControllerOutcome::Applied,
                        )
                    }
                    Some(RevokeRemoteControlControllerOutcome::NotFound) => {
                        RemoteControlResponse::RevokeController(
                            RemoteControlRevokeControllerOutcome::NotFound,
                        )
                    }
                    None => RemoteControlResponse::RevokeController(
                        RemoteControlRevokeControllerOutcome::Failed,
                    ),
                };
                let failure = RemoteControlResponse::RevokeController(
                    RemoteControlRevokeControllerOutcome::Failed,
                );
                apply_remote_controller_grant_transaction(
                    remote_control,
                    persistence,
                    &node,
                    responder,
                    prepared,
                    success,
                    failure,
                )
                .await?;
                let _completed = completion.send(());
            }
            Self::Snapshot { completion } => {
                if completion.is_closed() {
                    return Ok(());
                }
                let mut snapshot = std::vec![
                    0;
                    crate::persistence::remote_control_controller_grants_snapshot_capacity(
                        crate::remote_control::DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS,
                    )
                ];
                let outcome = remote_control
                    .write_controller_grants_snapshot(&mut snapshot)
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

async fn apply_remote_controller_grant_transaction(
    remote_control: &mut AssembledRemoteControl,
    persistence: Option<&AuthorizationTransaction>,
    node: &PrnsNodeHandle,
    responder: RespondToken,
    prepared: super::remote_control_pairing_persistence::PreparedControllerGrantChange,
    success: RemoteControlResponse,
    failure: RemoteControlResponse,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    if prepared.is_unchanged() {
        respond_to_remote_controller_grant(node, responder, success).await;
        return Ok(());
    }
    let Some(persistence) = persistence else {
        respond_to_remote_controller_grant(node, responder, failure).await;
        return Ok(());
    };
    let (mutation, projected, rollback) = prepared.into_parts();
    if persistence
        .store(SnapshotRegion::RemoteControlControllerGrants, projected)
        .await?
        .is_err()
    {
        restore_controller_grants_snapshot(persistence, rollback).await?;
        respond_to_remote_controller_grant(node, responder, failure).await;
        return Ok(());
    }
    activate_controller_grant_change(remote_control, mutation)?;
    let _responded = respond_to_remote_controller_grant(node, responder, success).await;
    Ok(())
}

async fn respond_to_remote_controller_grant(
    node: &PrnsNodeHandle,
    responder: RespondToken,
    response: RemoteControlResponse,
) -> bool {
    let mut encoded = std::vec![0; RemoteControlResponse::MAX_ENCODED_LEN];
    let Ok(encoded_len) = response.write_into(encoded.as_mut_slice()) else {
        let _closed = node.close_link(responder.link_id);
        return false;
    };
    encoded.truncate(encoded_len);
    if node
        .respond_owned_packed_settled(responder, encoded)
        .await
        .is_ok()
    {
        return true;
    }
    let _closed = node.close_link(responder.link_id);
    false
}

impl PrnsNodeHandle {
    pub(super) async fn authorize_remote_control_controller_and_respond(
        &self,
        grant: RemoteControlControllerGrant,
        responder: RespondToken,
    ) {
        let (completion, settled) = oneshot::channel();
        let operation = self.remote_control_controller_grants.submit(
            RemoteControlControllerGrantCommand::AuthorizeControllerAndRespond {
                grant,
                responder,
                node: self.clone(),
                completion,
            },
        );
        let _operation = match operation {
            Ok(operation) => operation,
            Err(RemoteControlControllerGrantSubmissionError::Busy) => {
                respond_to_remote_controller_grant(
                    self,
                    responder,
                    RemoteControlResponse::AuthorizeController(
                        RemoteControlAuthorizeControllerOutcome::Busy,
                    ),
                )
                .await;
                return;
            }
            Err(RemoteControlControllerGrantSubmissionError::NodeStopped) => {
                respond_to_remote_controller_grant(
                    self,
                    responder,
                    RemoteControlResponse::AuthorizeController(
                        RemoteControlAuthorizeControllerOutcome::Failed,
                    ),
                )
                .await;
                return;
            }
        };
        if settled.await.is_err() {
            let _closed = self.close_link(responder.link_id);
        }
    }

    pub(super) async fn revoke_remote_control_controller_and_respond(
        &self,
        controller: RemoteControlControllerIdentity,
        responder: RespondToken,
    ) {
        let (completion, settled) = oneshot::channel();
        let operation = self.remote_control_controller_grants.submit(
            RemoteControlControllerGrantCommand::RevokeControllerAndRespond {
                controller,
                responder,
                node: self.clone(),
                completion,
            },
        );
        let _operation = match operation {
            Ok(operation) => operation,
            Err(RemoteControlControllerGrantSubmissionError::Busy) => {
                respond_to_remote_controller_grant(
                    self,
                    responder,
                    RemoteControlResponse::RevokeController(
                        RemoteControlRevokeControllerOutcome::Busy,
                    ),
                )
                .await;
                return;
            }
            Err(RemoteControlControllerGrantSubmissionError::NodeStopped) => {
                respond_to_remote_controller_grant(
                    self,
                    responder,
                    RemoteControlResponse::RevokeController(
                        RemoteControlRevokeControllerOutcome::Failed,
                    ),
                )
                .await;
                return;
            }
        };
        if settled.await.is_err() {
            let _closed = self.close_link(responder.link_id);
        }
    }

    pub(super) async fn snapshot_remote_control_controller_grants(
        &self,
    ) -> Result<Option<std::vec::Vec<u8>>, super::PrepareFlushError> {
        let (completion, settled) = oneshot::channel();
        let _operation = self
            .remote_control_controller_grants
            .snapshot(completion)
            .await
            .map_err(|error| match error {
                RemoteControlControllerGrantSubmissionError::Busy => {
                    super::PrepareFlushError::NodeStopped
                }
                RemoteControlControllerGrantSubmissionError::NodeStopped => {
                    super::PrepareFlushError::NodeStopped
                }
            })?;
        settled
            .await
            .map_err(|_| super::PrepareFlushError::NodeStopped)?
            .map_err(super::PrepareFlushError::AuthorizationSnapshot)
    }
}

impl RemoteControlControllerGrantControl for PrnsNodeHandle {
    async fn set_remote_control_controller_grant(
        &self,
        grant: RemoteControlControllerGrant,
    ) -> Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantControlError>
    {
        let (completion, settled) = oneshot::channel();
        let _operation = match self
            .remote_control_controller_grants
            .submit(RemoteControlControllerGrantCommand::SetControllerGrant { grant, completion })
        {
            Ok(operation) => operation,
            Err(RemoteControlControllerGrantSubmissionError::Busy) => {
                return Err(SetRemoteControlControllerGrantControlError::Busy)
            }
            Err(RemoteControlControllerGrantSubmissionError::NodeStopped) => {
                return Err(SetRemoteControlControllerGrantControlError::NodeStopped)
            }
        };
        match settled.await {
            Ok(outcome) => outcome.map_err(Into::into),
            Err(_) => Err(SetRemoteControlControllerGrantControlError::NodeStopped),
        }
    }

    async fn revoke_remote_control_controller(
        &self,
        controller: RemoteControlControllerIdentity,
    ) -> Result<RevokeRemoteControlControllerOutcome, RevokeRemoteControlControllerControlError>
    {
        let (completion, settled) = oneshot::channel();
        let _operation = match self.remote_control_controller_grants.submit(
            RemoteControlControllerGrantCommand::RevokeController {
                controller,
                completion,
            },
        ) {
            Ok(operation) => operation,
            Err(RemoteControlControllerGrantSubmissionError::Busy) => {
                return Err(RevokeRemoteControlControllerControlError::Busy)
            }
            Err(RemoteControlControllerGrantSubmissionError::NodeStopped) => {
                return Err(RevokeRemoteControlControllerControlError::NodeStopped)
            }
        };
        match settled.await {
            Ok(outcome) => outcome.map_err(Into::into),
            Err(_) => Err(RevokeRemoteControlControllerControlError::NodeStopped),
        }
    }
}

#[cfg(test)]
mod tests;
