use crate::engine::{
    ApproveRemoteControlControllerPairing, ApproveRemoteControlTargetPairing,
    RejectRemoteControlControllerPairing, RejectRemoteControlTargetPairing,
};
use crate::remote_control::{
    RemoteControlControllerIdentity, RemoteControlControllerPairingAttemptView,
    RemoteControlControllerPairingAttemptWindow, RemoteControlPairingAttemptId,
    RemoteControlPairingConfirmationCode, RemoteControlPairingContext,
    RemoteControlPairingPermissions, RemoteControlTargetIdentity,
    RemoteControlTargetPairingAttemptView, RemoteControlTargetPairingAttemptWindow,
};

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingConfirmation {
    attempt_id: RemoteControlPairingAttemptId,
    context: RemoteControlPairingContext,
    controller: RemoteControlControllerIdentity,
    target: RemoteControlTargetIdentity,
    permissions: RemoteControlPairingPermissions,
}

impl RemoteControlPairingConfirmation {
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
