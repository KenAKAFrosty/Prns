use std::sync::Arc;

use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::sync::{oneshot, Mutex, OwnedMutexGuard};

use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity,
    RevokeRemoteControlControllerError, RevokeRemoteControlControllerOutcome,
    SetRemoteControlControllerGrantError, SetRemoteControlControllerGrantOutcome,
};

use super::node_facade::PrnsNodeHandle;
use super::{
    AssembledRemoteControl, RemoteControlAccessControl, RevokeRemoteControlControllerControlError,
    SetRemoteControlControllerGrantControlError,
};

const REMOTE_CONTROL_ACCESS_QUEUE_DEPTH: usize = 1;

pub(super) enum RemoteControlAccessCommand {
    SetControllerGrant {
        grant: RemoteControlControllerGrant,
        completion: oneshot::Sender<
            Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantError>,
        >,
    },
    RevokeController {
        controller: RemoteControlControllerIdentity,
        completion: oneshot::Sender<
            Result<RevokeRemoteControlControllerOutcome, RevokeRemoteControlControllerError>,
        >,
    },
}

#[derive(Clone)]
pub(super) struct RemoteControlAccessSender {
    commands: mpsc::Sender<RemoteControlAccessCommand>,
    operation: Arc<Mutex<()>>,
}

pub(super) struct RemoteControlAccessReceiver {
    commands: mpsc::Receiver<RemoteControlAccessCommand>,
}

enum RemoteControlAccessSubmissionError {
    Busy,
    NodeStopped,
}

pub(super) fn remote_control_access_lane(
) -> (RemoteControlAccessSender, RemoteControlAccessReceiver) {
    let (commands, receiver) = mpsc::channel(REMOTE_CONTROL_ACCESS_QUEUE_DEPTH);
    (
        RemoteControlAccessSender {
            commands,
            operation: Arc::new(Mutex::new(())),
        },
        RemoteControlAccessReceiver { commands: receiver },
    )
}

impl RemoteControlAccessSender {
    fn submit(
        &self,
        command: RemoteControlAccessCommand,
    ) -> Result<OwnedMutexGuard<()>, RemoteControlAccessSubmissionError> {
        let operation = self
            .operation
            .clone()
            .try_lock_owned()
            .map_err(|_| RemoteControlAccessSubmissionError::Busy)?;
        match self.commands.try_send(command) {
            Ok(()) => Ok(operation),
            Err(TrySendError::Full(_)) => Err(RemoteControlAccessSubmissionError::Busy),
            Err(TrySendError::Closed(_)) => Err(RemoteControlAccessSubmissionError::NodeStopped),
        }
    }
}

impl RemoteControlAccessReceiver {
    pub(super) async fn receive(&mut self) -> Option<RemoteControlAccessCommand> {
        self.commands.recv().await
    }
}

impl RemoteControlAccessCommand {
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
        }
    }
}

impl RemoteControlAccessControl for PrnsNodeHandle {
    async fn set_remote_control_controller_grant(
        &self,
        grant: RemoteControlControllerGrant,
    ) -> Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantControlError>
    {
        let (completion, settled) = oneshot::channel();
        let _operation = match self
            .remote_control_access
            .submit(RemoteControlAccessCommand::SetControllerGrant { grant, completion })
        {
            Ok(operation) => operation,
            Err(RemoteControlAccessSubmissionError::Busy) => {
                return Err(SetRemoteControlControllerGrantControlError::Busy)
            }
            Err(RemoteControlAccessSubmissionError::NodeStopped) => {
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
        let _operation =
            match self
                .remote_control_access
                .submit(RemoteControlAccessCommand::RevokeController {
                    controller,
                    completion,
                }) {
                Ok(operation) => operation,
                Err(RemoteControlAccessSubmissionError::Busy) => {
                    return Err(RevokeRemoteControlControllerControlError::Busy)
                }
                Err(RemoteControlAccessSubmissionError::NodeStopped) => {
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
        RemoteControlAccessTable, RemoteControlRequestKind, RevokeRemoteControlControllerOutcome,
        SetRemoteControlControllerGrantError, SetRemoteControlControllerGrantOutcome,
    };

    use super::super::node_facade::{test_remote_control_grant, PrnsNodeHandle};
    use super::super::{
        RemoteControlAccessControl, RevokeRemoteControlControllerControlError,
        SetRemoteControlControllerGrantControlError,
    };
    use super::RemoteControlAccessCommand;

    #[tokio::test]
    async fn unavailable_service_rejects_access_changes() {
        let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
        let mut engine = crate::engine::EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote_control = crate::runtime::configure_remote_control_service(
            &mut engine,
            crate::remote_control::RemoteControlService::Unavailable,
        )
        .expect("unavailable RemoteControl requires no storage");

        let (completion, settled) = oneshot::channel();
        RemoteControlAccessCommand::SetControllerGrant { grant, completion }
            .apply(&mut remote_control);
        assert_eq!(
            settled.await.expect("set completion remains connected"),
            Err(SetRemoteControlControllerGrantError::Unavailable),
        );

        let (completion, settled) = oneshot::channel();
        RemoteControlAccessCommand::RevokeController {
            controller: *grant.controller(),
            completion,
        }
        .apply(&mut remote_control);
        assert_eq!(
            settled.await.expect("revoke completion remains connected"),
            Err(crate::remote_control::RevokeRemoteControlControllerError::Unavailable,),
        );
    }

    #[tokio::test]
    async fn access_lane_preserves_exact_set_and_revoke_outcomes() {
        let (commands, _command_rx) = mpsc::unbounded_channel();
        let (handle, mut access) = PrnsNodeHandle::over_with_remote_control_access(commands);
        let previous = test_remote_control_grant(RemoteControlRequestKind::Describe);
        let grant = test_remote_control_grant(RemoteControlRequestKind::AnnounceSelf);

        let (set, ()) = tokio::join!(handle.set_remote_control_controller_grant(grant), async {
            let Some(RemoteControlAccessCommand::SetControllerGrant {
                grant: submitted,
                completion,
            }) = access.receive().await
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
                let Some(RemoteControlAccessCommand::RevokeController {
                    controller,
                    completion,
                }) = access.receive().await
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
    async fn access_lane_distinguishes_busy_capacity_and_stopped() {
        let (commands, _command_rx) = mpsc::unbounded_channel();
        let (handle, mut access) = PrnsNodeHandle::over_with_remote_control_access(commands);
        let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
        let setting = handle.set_remote_control_controller_grant(grant);
        tokio::pin!(setting);
        tokio::select! {
            biased;
            outcome = &mut setting => panic!("unsettled access change returned: {outcome:?}"),
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
        let Some(RemoteControlAccessCommand::SetControllerGrant { completion, .. }) =
            access.receive().await
        else {
            panic!("set controller grant command")
        };
        assert!(completion
            .send(Err(SetRemoteControlControllerGrantError::CapacityExhausted))
            .is_ok());
        assert_eq!(
            setting.await,
            Err(SetRemoteControlControllerGrantControlError::CapacityExhausted),
        );

        drop(access);
        assert_eq!(
            handle.set_remote_control_controller_grant(grant).await,
            Err(SetRemoteControlControllerGrantControlError::NodeStopped),
        );
    }

    #[tokio::test]
    async fn a_cancelled_received_access_change_does_not_mutate_the_table() {
        let (commands, _command_rx) = mpsc::unbounded_channel();
        let (handle, mut access) = PrnsNodeHandle::over_with_remote_control_access(commands);
        let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
        let changing =
            tokio::spawn(async move { handle.set_remote_control_controller_grant(grant).await });
        let Some(command) = access.receive().await else {
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
        assert!(remote_control.access().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_abandoned_queued_access_change_holds_the_lane_until_drained() {
        let (commands, _command_rx) = mpsc::unbounded_channel();
        let (handle, mut access) = PrnsNodeHandle::over_with_remote_control_access(commands);
        let grant = test_remote_control_grant(RemoteControlRequestKind::Describe);
        {
            let setting = handle.set_remote_control_controller_grant(grant);
            tokio::pin!(setting);
            tokio::select! {
                biased;
                outcome = &mut setting => panic!("unsettled access change returned: {outcome:?}"),
                () = tokio::task::yield_now() => {}
            }
        }

        assert_eq!(
            handle
                .revoke_remote_control_controller(*grant.controller())
                .await,
            Err(RevokeRemoteControlControllerControlError::Busy),
        );
        let Some(RemoteControlAccessCommand::SetControllerGrant { completion, .. }) =
            access.receive().await
        else {
            panic!("set controller grant command")
        };
        assert!(completion.is_closed());

        let (revoke, ()) = tokio::join!(
            handle.revoke_remote_control_controller(*grant.controller()),
            async {
                let Some(RemoteControlAccessCommand::RevokeController { completion, .. }) =
                    access.receive().await
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
