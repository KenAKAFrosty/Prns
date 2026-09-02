use crate::engine::{
    ApproveRemoteControlControllerPairing, ApproveRemoteControlControllerPairingFailure,
    ApproveRemoteControlTargetPairing, ApproveRemoteControlTargetPairingFailure,
    BeginRemoteControlControllerPairing, BeginRemoteControlControllerPairingFailure,
    RejectRemoteControlControllerPairing, RejectRemoteControlControllerPairingFailure,
    RejectRemoteControlTargetPairing, RejectRemoteControlTargetPairingFailure,
    RemoteControlControllerPairingRequestFailure, RemoteControlControllerPairingResponseReceived,
    RemoteControlTargetPairingApproval, RemoteControlTargetPairingRejection,
};

use super::{RemoteControlPairingControlError, SendError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingLinkCleanupOutcome {
    Queued,
    NotQueued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginRemoteControlControllerPairingControlFailure {
    Begin(BeginRemoteControlControllerPairingFailure),
    Identify {
        failure: SendError<crate::engine::IdentifyFailure>,
        cleanup: RemoteControlPairingLinkCleanupOutcome,
    },
    Request(RemoteControlControllerPairingRequestFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproveRemoteControlControllerPairingControlFailure {
    Approve(ApproveRemoteControlControllerPairingFailure),
    Request(RemoteControlControllerPairingRequestFailure),
}

pub type BeginRemoteControlControllerPairingControlError =
    RemoteControlPairingControlError<BeginRemoteControlControllerPairingControlFailure>;
pub type ApproveRemoteControlControllerPairingControlError =
    RemoteControlPairingControlError<ApproveRemoteControlControllerPairingControlFailure>;
pub type RejectRemoteControlControllerPairingControlError =
    RemoteControlPairingControlError<RejectRemoteControlControllerPairingFailure>;
pub type ApproveRemoteControlTargetPairingControlError =
    RemoteControlPairingControlError<ApproveRemoteControlTargetPairingFailure>;
pub type RejectRemoteControlTargetPairingControlError =
    RemoteControlPairingControlError<RejectRemoteControlTargetPairingFailure>;

pub trait RemoteControlPairingControl {
    fn begin_remote_control_controller_pairing(
        &self,
        begin: BeginRemoteControlControllerPairing,
    ) -> impl core::future::Future<
        Output = Result<
            RemoteControlControllerPairingResponseReceived,
            BeginRemoteControlControllerPairingControlError,
        >,
    > + Send;

    fn approve_remote_control_controller_pairing(
        &self,
        approve: ApproveRemoteControlControllerPairing,
    ) -> impl core::future::Future<
        Output = Result<
            RemoteControlControllerPairingResponseReceived,
            ApproveRemoteControlControllerPairingControlError,
        >,
    > + Send;

    fn reject_remote_control_controller_pairing(
        &self,
        reject: RejectRemoteControlControllerPairing,
    ) -> impl core::future::Future<
        Output = Result<
            crate::engine::RemoteControlControllerPairingRejection,
            RejectRemoteControlControllerPairingControlError,
        >,
    > + Send;

    fn approve_remote_control_target_pairing(
        &self,
        approve: ApproveRemoteControlTargetPairing,
    ) -> impl core::future::Future<
        Output = Result<
            RemoteControlTargetPairingApproval,
            ApproveRemoteControlTargetPairingControlError,
        >,
    > + Send;

    fn reject_remote_control_target_pairing(
        &self,
        reject: RejectRemoteControlTargetPairing,
    ) -> impl core::future::Future<
        Output = Result<
            RemoteControlTargetPairingRejection,
            RejectRemoteControlTargetPairingControlError,
        >,
    > + Send;
}
