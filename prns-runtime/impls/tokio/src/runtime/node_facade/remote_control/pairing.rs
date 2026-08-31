use crate::engine::{
    ApproveRemoteControlControllerPairing, ApproveRemoteControlTargetPairing,
    BeginRemoteControlControllerPairing, RejectRemoteControlControllerPairing,
    RejectRemoteControlTargetPairing, Settleable,
};
use crate::runtime::{
    ApproveRemoteControlControllerPairingControlError,
    ApproveRemoteControlControllerPairingControlFailure,
    ApproveRemoteControlTargetPairingControlError, BeginRemoteControlControllerPairingControlError,
    BeginRemoteControlControllerPairingControlFailure,
    RejectRemoteControlControllerPairingControlError, RejectRemoteControlTargetPairingControlError,
    RemoteControlPairingControl, RemoteControlPairingControlError,
    RemoteControlPairingLinkCleanupOutcome,
};

use super::super::PrnsNodeHandle;

async fn settle_pairing_command<C>(
    node: &PrnsNodeHandle,
    command: C,
) -> Result<C::Success, RemoteControlPairingControlError<C::Failure>>
where
    C: Settleable,
{
    let Some(settlement) = node.settle(command.into_command()).await else {
        return Err(RemoteControlPairingControlError::NodeStopped);
    };
    let Some(result) = C::from_settlement(settlement) else {
        return Err(RemoteControlPairingControlError::NodeStopped);
    };
    result.map_err(RemoteControlPairingControlError::Failed)
}

impl RemoteControlPairingControl for PrnsNodeHandle {
    async fn begin_remote_control_controller_pairing(
        &self,
        begin: BeginRemoteControlControllerPairing,
    ) -> Result<
        crate::engine::RemoteControlControllerPairingResponseReceived,
        BeginRemoteControlControllerPairingControlError,
    > {
        let link_id = begin.context.link_id();
        let begun = settle_pairing_command(self, begin).await.map_err(|error| {
            error.map_failure(BeginRemoteControlControllerPairingControlFailure::Begin)
        })?;
        if let Err(error) = self
            .identify(link_id, begun.controller_identity_hash())
            .await
        {
            let cleanup = match self.close_link(link_id) {
                true => RemoteControlPairingLinkCleanupOutcome::Queued,
                false => RemoteControlPairingLinkCleanupOutcome::NotQueued,
            };
            return Err(RemoteControlPairingControlError::Failed(
                BeginRemoteControlControllerPairingControlFailure::Identify {
                    failure: error,
                    cleanup,
                },
            ));
        }
        settle_pairing_command(self, begun.into_request())
            .await
            .map_err(|error| {
                error.map_failure(BeginRemoteControlControllerPairingControlFailure::Request)
            })
    }

    async fn approve_remote_control_controller_pairing(
        &self,
        approve: ApproveRemoteControlControllerPairing,
    ) -> Result<
        crate::engine::RemoteControlControllerPairingResponseReceived,
        ApproveRemoteControlControllerPairingControlError,
    > {
        let approval = settle_pairing_command(self, approve)
            .await
            .map_err(|error| {
                error.map_failure(ApproveRemoteControlControllerPairingControlFailure::Approve)
            })?;
        settle_pairing_command(self, approval.into_request())
            .await
            .map_err(|error| {
                error.map_failure(ApproveRemoteControlControllerPairingControlFailure::Request)
            })
    }

    async fn reject_remote_control_controller_pairing(
        &self,
        reject: RejectRemoteControlControllerPairing,
    ) -> Result<
        crate::engine::RemoteControlControllerPairingRejection,
        RejectRemoteControlControllerPairingControlError,
    > {
        settle_pairing_command(self, reject).await
    }

    async fn approve_remote_control_target_pairing(
        &self,
        approve: ApproveRemoteControlTargetPairing,
    ) -> Result<
        crate::engine::RemoteControlTargetPairingApproval,
        ApproveRemoteControlTargetPairingControlError,
    > {
        settle_pairing_command(self, approve).await
    }

    async fn reject_remote_control_target_pairing(
        &self,
        reject: RejectRemoteControlTargetPairing,
    ) -> Result<
        crate::engine::RemoteControlTargetPairingRejection,
        RejectRemoteControlTargetPairingControlError,
    > {
        settle_pairing_command(self, reject).await
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use crate::engine::{
        BeginRemoteControlControllerPairing, CloseLink, EngineReaction, EngineState, Identify,
        IdentifyFailure, IssuedCommand, Journaled, PrnsCommand, Settlement,
    };
    use crate::identity::IdentityHash;
    use crate::interfaces::AttachedInterfaces;
    use crate::manifold::driver::HostCommand;
    use crate::remote_control::{
        RemoteControlPairingContext, RemoteControlPairingIdentity,
        RemoteControlPairingInvitationCode,
    };
    use crate::routing::links::LinkId;
    use crate::runtime::{
        configure_remote_control_service, BeginRemoteControlControllerPairingControlFailure,
        RemoteControlPairingControl, RemoteControlPairingControlError,
        RemoteControlPairingLinkCleanupOutcome, SendError,
    };
    use crate::storage::GrowableHeap;
    use crate::units::InstantMillis;

    use super::PrnsNodeHandle;

    const LINK_ID: LinkId = LinkId::new([0x41; 16]);
    const CONTROLLER_PAIRING_NOW: InstantMillis = InstantMillis(1_000);

    fn begin() -> BeginRemoteControlControllerPairing {
        BeginRemoteControlControllerPairing {
            context: RemoteControlPairingContext::new(
                RemoteControlPairingIdentity::new(IdentityHash::new([0x51; 16])).endpoint(),
                LINK_ID,
            ),
            invitation_code: RemoteControlPairingInvitationCode::from_value(0x1234_ABCD),
            pairing_expires_at: InstantMillis(10_000),
        }
    }

    fn controller_pairing_engine() -> (EngineState<GrowableHeap>, IdentityHash) {
        let service = crate::runtime::node_facade::test_remote_control_service();
        let controller = service
            .configuration()
            .unwrap()
            .identity_secrets()
            .identities()
            .controller()
            .identity_hash();
        let mut engine = EngineState::default();
        configure_remote_control_service(&mut engine, service).unwrap();
        (engine, controller)
    }

    fn settle_begin(engine: &mut EngineState<GrowableHeap>, issued: IssuedCommand) -> Settlement {
        let mut settlement = None;
        engine.ingest_command_into(
            issued,
            AttachedInterfaces::new(&[]),
            CONTROLLER_PAIRING_NOW,
            &mut |_| panic!("controller pairing begin needs no entropy"),
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::CommandSettled {
                    settlement: settled,
                    ..
                }) => settlement = Some(settled),
                EngineReaction::Journaled(_) | EngineReaction::Directive(_) => {}
            },
        );
        settlement.unwrap()
    }

    #[tokio::test]
    async fn controller_pairing_begin_identifies_before_sending_its_request() {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let handle = PrnsNodeHandle::over(commands);
        let (mut engine, controller) = controller_pairing_engine();
        let expected_begin = begin();
        let pairing_handle = handle.clone();
        let pairing = tokio::spawn(async move {
            pairing_handle
                .begin_remote_control_controller_pairing(begin())
                .await
        });

        let Some(HostCommand::AwaitedEngine { issued, completion }) = command_rx.recv().await
        else {
            panic!("controller pairing begin command")
        };
        assert_eq!(
            issued.command,
            PrnsCommand::BeginRemoteControlControllerPairing(expected_begin)
        );
        assert!(completion.send(settle_begin(&mut engine, issued)).is_ok());

        let Some(HostCommand::AwaitedEngine { issued, completion }) = command_rx.recv().await
        else {
            panic!("controller pairing identify command")
        };
        assert_eq!(
            issued.command,
            PrnsCommand::Identify(Identify {
                link_id: LINK_ID,
                identity: controller,
            }),
        );
        assert!(completion.send(Settlement::Identify(Ok(()))).is_ok());

        let Some(HostCommand::AwaitedEngine { issued, completion }) = command_rx.recv().await
        else {
            panic!("controller pairing request command")
        };
        assert!(matches!(
            issued.command,
            PrnsCommand::RemoteControlControllerPairingRequest(_),
        ));
        assert!(completion.send(Settlement::Identify(Ok(()))).is_ok());
        assert_eq!(
            pairing.await.unwrap(),
            Err(RemoteControlPairingControlError::NodeStopped),
        );
    }

    #[tokio::test]
    async fn controller_pairing_identification_failure_closes_the_link_and_preserves_the_cause() {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let handle = PrnsNodeHandle::over(commands);
        let (mut engine, _) = controller_pairing_engine();
        let pairing_handle = handle.clone();
        let pairing = tokio::spawn(async move {
            pairing_handle
                .begin_remote_control_controller_pairing(begin())
                .await
        });

        let Some(HostCommand::AwaitedEngine { issued, completion }) = command_rx.recv().await
        else {
            panic!("controller pairing begin command")
        };
        assert!(completion.send(settle_begin(&mut engine, issued)).is_ok());

        let Some(HostCommand::AwaitedEngine { completion, .. }) = command_rx.recv().await else {
            panic!("controller pairing identify command")
        };
        assert!(completion
            .send(Settlement::Identify(Err(IdentifyFailure::WriteFailed)))
            .is_ok());

        let Some(HostCommand::Engine(issued)) = command_rx.recv().await else {
            panic!("controller pairing link cleanup command")
        };
        assert_eq!(
            issued.command,
            PrnsCommand::CloseLink(CloseLink { link_id: LINK_ID }),
        );
        assert_eq!(
            pairing.await.unwrap(),
            Err(RemoteControlPairingControlError::Failed(
                BeginRemoteControlControllerPairingControlFailure::Identify {
                    failure: SendError::Failed(IdentifyFailure::WriteFailed),
                    cleanup: RemoteControlPairingLinkCleanupOutcome::Queued,
                },
            )),
        );
    }
}
