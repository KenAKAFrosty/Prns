use crate::engine::{
    ApproveRemoteControlControllerPairing, ApproveRemoteControlTargetPairing,
    RejectRemoteControlControllerPairing, RejectRemoteControlTargetPairing,
};
use crate::remote_control::{
    CancelRemoteControlControllerPairingError, RemoteControlControllerPairingAborted,
    RemoteControlControllerPairingSubject, RemoteControlControllerPairingTerminal,
};
use crate::remote_control::{
    RemoteControlControllerAuthority, RemoteControlControllerIdentity,
    RemoteControlControllerPairingAttemptView, RemoteControlControllerPairingAttemptWindow,
    RemoteControlControllerPairingView, RemoteControlControllerPairingWindow,
    RemoteControlPairingAttemptId, RemoteControlPairingConfirmationCode,
    RemoteControlPairingContext, RemoteControlPairingPermissions, RemoteControlRequestSet,
    RemoteControlTargetIdentity, RemoteControlTargetPairingAttemptView,
    RemoteControlTargetPairingAttemptWindow,
};

/// An owned observation of the engine's current controller attempt. Reading it
/// neither consumes events nor grants approval; decisions still carry the exact
/// attempt id and the engine rechecks its deadline and state at admission.
#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlControllerPairingSnapshot {
    Idle,
    AwaitingOffer {
        context: RemoteControlPairingContext,
        controller: RemoteControlControllerIdentity,
        window: RemoteControlControllerPairingWindow,
    },
    AwaitingApproval {
        confirmation: RemoteControlControllerPairingConfirmation,
    },
    AwaitingCompletion {
        confirmation: RemoteControlControllerPairingConfirmation,
    },
    Persisting {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
        target: RemoteControlTargetIdentity,
        authority: RemoteControlControllerAuthority,
        permitted_requests: RemoteControlRequestSet,
    },
}

impl From<RemoteControlControllerPairingView<'_>> for RemoteControlControllerPairingSnapshot {
    fn from(view: RemoteControlControllerPairingView<'_>) -> Self {
        match view {
            RemoteControlControllerPairingView::Idle => Self::Idle,
            RemoteControlControllerPairingView::AwaitingOffer(begin) => Self::AwaitingOffer {
                context: begin.context(),
                controller: *begin.controller(),
                window: begin.window(),
            },
            RemoteControlControllerPairingView::AwaitingApproval(attempt) => {
                Self::AwaitingApproval {
                    confirmation: attempt.into(),
                }
            }
            RemoteControlControllerPairingView::AwaitingCompletion(attempt) => {
                Self::AwaitingCompletion {
                    confirmation: attempt.into(),
                }
            }
            RemoteControlControllerPairingView::Persisting(persisting) => Self::Persisting {
                attempt_id: persisting.attempt_id(),
                context: persisting.context(),
                target: RemoteControlTargetIdentity::new(
                    *persisting.access().target().public_keys(),
                ),
                authority: persisting.access().authority(),
                permitted_requests: *persisting.access().permitted_requests(),
            },
        }
    }
}

/// Live ceremony and retained result from one engine-lane observation. Results
/// are bounded to one and reset by a newly admitted invitation or host restart.
#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlControllerPairingStatus {
    pub current: RemoteControlControllerPairingSnapshot,
    pub terminal: Option<RemoteControlControllerPairingTerminal>,
}

pub trait RemoteControlPairingStatusInspection {
    fn remote_control_controller_pairing_status(
        &self,
    ) -> impl core::future::Future<
        Output = Result<RemoteControlControllerPairingStatus, RemoteControlPairingInspectionError>,
    > + Send;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelRemoteControlControllerPairingControlError {
    NodeStopped,
    Unavailable,
    Rejected {
        reason: CancelRemoteControlControllerPairingError,
    },
}

pub trait RemoteControlControllerPairingCancellation {
    fn cancel_remote_control_controller_pairing(
        &self,
        subject: RemoteControlControllerPairingSubject,
    ) -> impl core::future::Future<
        Output = Result<
            RemoteControlControllerPairingAborted,
            CancelRemoteControlControllerPairingControlError,
        >,
    > + Send;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingInspectionError {
    NodeStopped,
    Unavailable,
}

/// Optional asynchronous inspection for runtimes that own an engine lane.
/// A new observation after a lost callback or UI reload uses current authority.
pub trait RemoteControlPairingInspection {
    fn remote_control_controller_pairing_snapshot(
        &self,
    ) -> impl core::future::Future<
        Output = Result<
            RemoteControlControllerPairingSnapshot,
            RemoteControlPairingInspectionError,
        >,
    > + Send;
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingConfirmation {
    attempt_id: RemoteControlPairingAttemptId,
    context: RemoteControlPairingContext,
    controller: RemoteControlControllerIdentity,
    target: RemoteControlTargetIdentity,
    permissions: RemoteControlPairingPermissions,
}

impl RemoteControlPairingConfirmation {
    /// Move the public transcript facts without copying an owned confirmation.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RemoteControlPairingAttemptId,
        RemoteControlPairingContext,
        RemoteControlControllerIdentity,
        RemoteControlTargetIdentity,
        RemoteControlPairingPermissions,
    ) {
        (
            self.attempt_id,
            self.context,
            self.controller,
            self.target,
            self.permissions,
        )
    }

    fn from_controller(attempt: RemoteControlControllerPairingAttemptView<'_>) -> Self {
        Self::from_parts(
            attempt.attempt_id(),
            attempt.context(),
            attempt.controller(),
            attempt.target(),
            attempt.permissions(),
        )
    }

    fn from_target(attempt: RemoteControlTargetPairingAttemptView<'_>) -> Self {
        Self::from_parts(
            attempt.attempt_id(),
            attempt.context(),
            attempt.controller(),
            attempt.target(),
            attempt.permissions(),
        )
    }

    fn from_parts(
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
        controller: &RemoteControlControllerIdentity,
        target: &RemoteControlTargetIdentity,
        permissions: &RemoteControlPairingPermissions,
    ) -> Self {
        Self {
            attempt_id,
            context,
            controller: *controller,
            target: RemoteControlTargetIdentity::new(*target.public_keys()),
            permissions: permissions.clone(),
        }
    }

    #[must_use]
    pub const fn attempt_id(&self) -> RemoteControlPairingAttemptId {
        self.attempt_id
    }

    #[must_use]
    pub const fn context(&self) -> RemoteControlPairingContext {
        self.context
    }

    #[must_use]
    pub const fn controller(&self) -> &RemoteControlControllerIdentity {
        &self.controller
    }

    #[must_use]
    pub const fn target(&self) -> &RemoteControlTargetIdentity {
        &self.target
    }

    #[must_use]
    pub const fn permissions(&self) -> &RemoteControlPairingPermissions {
        &self.permissions
    }

    #[must_use]
    pub const fn confirmation_code(&self) -> RemoteControlPairingConfirmationCode {
        self.attempt_id.confirmation_code()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlControllerPairingConfirmation {
    confirmation: RemoteControlPairingConfirmation,
    window: RemoteControlControllerPairingAttemptWindow,
}

impl RemoteControlControllerPairingConfirmation {
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RemoteControlPairingConfirmation,
        RemoteControlControllerPairingAttemptWindow,
    ) {
        (self.confirmation, self.window)
    }

    #[must_use]
    pub const fn confirmation(&self) -> &RemoteControlPairingConfirmation {
        &self.confirmation
    }

    #[must_use]
    pub const fn window(&self) -> RemoteControlControllerPairingAttemptWindow {
        self.window
    }

    #[must_use]
    pub const fn approval(&self) -> ApproveRemoteControlControllerPairing {
        ApproveRemoteControlControllerPairing {
            attempt_id: self.confirmation.attempt_id(),
        }
    }

    #[must_use]
    pub const fn rejection(&self) -> RejectRemoteControlControllerPairing {
        RejectRemoteControlControllerPairing {
            attempt_id: self.confirmation.attempt_id(),
        }
    }
}

impl From<RemoteControlControllerPairingAttemptView<'_>>
    for RemoteControlControllerPairingConfirmation
{
    fn from(attempt: RemoteControlControllerPairingAttemptView<'_>) -> Self {
        Self {
            confirmation: RemoteControlPairingConfirmation::from_controller(attempt),
            window: attempt.window(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlTargetPairingConfirmation {
    confirmation: RemoteControlPairingConfirmation,
    window: RemoteControlTargetPairingAttemptWindow,
}

impl RemoteControlTargetPairingConfirmation {
    #[must_use]
    pub const fn confirmation(&self) -> &RemoteControlPairingConfirmation {
        &self.confirmation
    }

    #[must_use]
    pub const fn window(&self) -> RemoteControlTargetPairingAttemptWindow {
        self.window
    }

    #[must_use]
    pub const fn approval(&self) -> ApproveRemoteControlTargetPairing {
        ApproveRemoteControlTargetPairing {
            attempt_id: self.confirmation.attempt_id(),
        }
    }

    #[must_use]
    pub const fn rejection(&self) -> RejectRemoteControlTargetPairing {
        RejectRemoteControlTargetPairing {
            attempt_id: self.confirmation.attempt_id(),
        }
    }
}

impl From<RemoteControlTargetPairingAttemptView<'_>> for RemoteControlTargetPairingConfirmation {
    fn from(attempt: RemoteControlTargetPairingAttemptView<'_>) -> Self {
        Self {
            confirmation: RemoteControlPairingConfirmation::from_target(attempt),
            window: attempt.window(),
        }
    }
}
