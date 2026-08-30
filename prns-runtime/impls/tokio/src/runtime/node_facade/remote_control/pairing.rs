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
        let begun = settle_pairing_command(self, begin).await.map_err(|error| {
            error.map_failure(BeginRemoteControlControllerPairingControlFailure::Begin)
        })?;
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
