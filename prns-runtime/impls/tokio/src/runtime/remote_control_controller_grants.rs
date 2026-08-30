use std::sync::Arc;

use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::sync::{oneshot, Mutex, OwnedMutexGuard};

use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
};

use super::node_facade::PrnsNodeHandle;
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
    pub(super) fn apply(self, remote_control: &mut AssembledRemoteControl) {
        match self {
            Self::SetControllerGrant { grant, completion } => {
                if completion.is_closed() {
                    return;
                }
                let outcome = remote_control.set_controller_grant(grant);
                let _completion = completion.send(outcome);
            }
            Self::RevokeController {
                controller,
                completion,
            } => {
                if completion.is_closed() {
                    return;
                }
                let outcome = remote_control.revoke_controller(&controller);
                let _completion = completion.send(outcome);
            }
            Self::Snapshot { completion } => {
                if completion.is_closed() {
                    return;
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
    }
}

impl PrnsNodeHandle {
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
mod tests {
    use tokio::sync::{mpsc, oneshot};

    use crate::remote_control::{
        RemoteControlControllerGrantTable, RemoteControlRequestKind,
        RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
    };

    use super::super::node_facade::{test_remote_control_grant, PrnsNodeHandle};
    use super::super::{
        RemoteControlControllerGrantControl, RevokeRemoteControlControllerControlError,
        RevokeRemoteControlControllerServiceError, SetRemoteControlControllerGrantControlError,
        SetRemoteControlControllerGrantServiceError,
    };
    use super::RemoteControlControllerGrantCommand;

    #[tokio::test]
    async fn unavailable_service_rejects_controller_grant_changes() {
        let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
        let mut engine = crate::engine::EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote_control = crate::runtime::configure_remote_control_service(
            &mut engine,
            crate::remote_control::RemoteControlService::Unavailable,
        )
        .expect("unavailable RemoteControl requires no storage");

        let (completion, settled) = oneshot::channel();
        RemoteControlControllerGrantCommand::SetControllerGrant { grant, completion }
            .apply(&mut remote_control);
        assert_eq!(
            settled.await.expect("set completion remains connected"),
            Err(SetRemoteControlControllerGrantServiceError::Unavailable),
        );

        let (completion, settled) = oneshot::channel();
        RemoteControlControllerGrantCommand::RevokeController {
            controller: *grant.controller(),
            completion,
        }
        .apply(&mut remote_control);
        assert_eq!(
            settled.await.expect("revoke completion remains connected"),
            Err(RevokeRemoteControlControllerServiceError::Unavailable),
        );
    }

    #[tokio::test]
    async fn controller_grant_lane_preserves_exact_set_and_revoke_outcomes() {
        let (commands, _command_rx) = mpsc::unbounded_channel();
        let (handle, mut controller_grants) =
            PrnsNodeHandle::over_with_remote_control_controller_grant_lane(commands);
        let previous = test_remote_control_grant(RemoteControlRequestKind::Describe);
        let grant = test_remote_control_grant(RemoteControlRequestKind::AnnounceSelf);

        let (set, ()) = tokio::join!(handle.set_remote_control_controller_grant(grant), async {
            let Some(RemoteControlControllerGrantCommand::SetControllerGrant {
                grant: submitted,
                completion,
            }) = controller_grants.receive().await
            else {
                panic!("set controller grant command")
            };
            assert_eq!(submitted, grant);
            assert!(completion
                .send(Ok(SetRemoteControlControllerGrantOutcome::Updated {
                    previous,
                }))
                .is_ok());
        },);
        assert_eq!(
            set,
            Ok(SetRemoteControlControllerGrantOutcome::Updated { previous }),
        );

        let (revoke, ()) = tokio::join!(
            handle.revoke_remote_control_controller(*grant.controller()),
            async {
                let Some(RemoteControlControllerGrantCommand::RevokeController {
                    controller,
                    completion,
                }) = controller_grants.receive().await
                else {
                    panic!("revoke controller command")
                };
                assert_eq!(controller, *grant.controller());
                assert!(completion
                    .send(Ok(RevokeRemoteControlControllerOutcome::Revoked { grant }))
                    .is_ok());
            },
        );
        assert_eq!(
            revoke,
            Ok(RevokeRemoteControlControllerOutcome::Revoked { grant }),
        );
    }

    #[tokio::test]
    async fn controller_grant_lane_distinguishes_busy_capacity_and_stopped() {
        let (commands, _command_rx) = mpsc::unbounded_channel();
        let (handle, mut controller_grants) =
            PrnsNodeHandle::over_with_remote_control_controller_grant_lane(commands);
        let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
        let setting = handle.set_remote_control_controller_grant(grant);
        tokio::pin!(setting);
        tokio::select! {
            biased;
            outcome = &mut setting => panic!("unsettled controller_grants change returned: {outcome:?}"),
            () = tokio::task::yield_now() => {}
        }

        assert_eq!(
            handle.set_remote_control_controller_grant(grant).await,
            Err(SetRemoteControlControllerGrantControlError::Busy),
        );
        assert_eq!(
            handle
                .revoke_remote_control_controller(*grant.controller())
                .await,
            Err(RevokeRemoteControlControllerControlError::Busy),
        );
        let Some(RemoteControlControllerGrantCommand::SetControllerGrant { completion, .. }) =
            controller_grants.receive().await
        else {
            panic!("set controller grant command")
        };
        assert!(completion
            .send(Err(
                SetRemoteControlControllerGrantServiceError::CapacityExhausted,
            ))
            .is_ok());
        assert_eq!(
            setting.await,
            Err(SetRemoteControlControllerGrantControlError::CapacityExhausted),
        );

        drop(controller_grants);
        assert_eq!(
            handle.set_remote_control_controller_grant(grant).await,
            Err(SetRemoteControlControllerGrantControlError::NodeStopped),
        );
    }

    #[tokio::test]
    async fn a_cancelled_received_controller_grant_change_does_not_mutate_the_table() {
        let (commands, _command_rx) = mpsc::unbounded_channel();
        let (handle, mut controller_grants) =
            PrnsNodeHandle::over_with_remote_control_controller_grant_lane(commands);
        let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
        let changing =
            tokio::spawn(async move { handle.set_remote_control_controller_grant(grant).await });
        let Some(command) = controller_grants.receive().await else {
            panic!("set controller grant command")
        };
        changing.abort();
        assert!(changing.await.is_err());

        let service = super::super::node_facade::test_remote_control_service();
        let mut engine = crate::engine::EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote_control =
            crate::runtime::configure_remote_control_service(&mut engine, service)
                .expect("RemoteControl fits growable storage");
        command.apply(&mut remote_control);
        assert!(remote_control.controller_grants().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_abandoned_queued_controller_grant_change_holds_the_lane_until_drained() {
        let (commands, _command_rx) = mpsc::unbounded_channel();
        let (handle, mut controller_grants) =
            PrnsNodeHandle::over_with_remote_control_controller_grant_lane(commands);
        let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
        {
            let setting = handle.set_remote_control_controller_grant(grant);
            tokio::pin!(setting);
            tokio::select! {
                biased;
                outcome = &mut setting => panic!("unsettled controller_grants change returned: {outcome:?}"),
                () = tokio::task::yield_now() => {}
            }
        }

        assert_eq!(
            handle
                .revoke_remote_control_controller(*grant.controller())
                .await,
            Err(RevokeRemoteControlControllerControlError::Busy),
        );
        let Some(RemoteControlControllerGrantCommand::SetControllerGrant { completion, .. }) =
            controller_grants.receive().await
        else {
            panic!("set controller grant command")
        };
        assert!(completion.is_closed());

        let (revoke, ()) = tokio::join!(
            handle.revoke_remote_control_controller(*grant.controller()),
            async {
                let Some(RemoteControlControllerGrantCommand::RevokeController {
                    completion, ..
                }) = controller_grants.receive().await
                else {
                    panic!("revoke controller command")
                };
                assert!(completion
                    .send(Ok(RevokeRemoteControlControllerOutcome::NotFound))
                    .is_ok());
            },
        );
        assert_eq!(revoke, Ok(RevokeRemoteControlControllerOutcome::NotFound));
    }
}
