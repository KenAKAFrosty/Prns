use crate::remote_control::{
    RemoteControlControllerIdentity, RemoteControlTargetAccess, RemoteControlTargetIdentity,
};
use crate::units::InstantMillis;

use super::super::{
    RemoteControlPairingAttemptId, RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin,
    RemoteControlPairingCommit, RemoteControlPairingCompletedVerificationError,
    RemoteControlPairingConfirmationCode, RemoteControlPairingContext,
    RemoteControlPairingOfferVerificationError, RemoteControlPairingPermissions,
    RemoteControlPairingTranscript,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingWindowError {
    DeadlineNotFuture {
        started_at: InstantMillis,
        expires_at: InstantMillis,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerPairingWindow {
    started_at: InstantMillis,
    expires_at: InstantMillis,
}

impl RemoteControlControllerPairingWindow {
    pub fn new(
        started_at: InstantMillis,
        expires_at: InstantMillis,
    ) -> Result<Self, RemoteControlControllerPairingWindowError> {
        if expires_at <= started_at {
            return Err(
                RemoteControlControllerPairingWindowError::DeadlineNotFuture {
                    started_at,
                    expires_at,
                },
            );
        }
        Ok(Self {
            started_at,
            expires_at,
        })
    }

    #[must_use]
    pub const fn started_at(self) -> InstantMillis {
        self.started_at
    }

    #[must_use]
    pub const fn expires_at(self) -> InstantMillis {
        self.expires_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingAttemptWindowError {
    PairingWindowElapsed {
        offered_at: InstantMillis,
        pairing_expires_at: InstantMillis,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerPairingAttemptWindow {
    offered_at: InstantMillis,
    attempt_timeout: RemoteControlPairingAttemptTimeout,
    expires_at: InstantMillis,
}

impl RemoteControlControllerPairingAttemptWindow {
    pub fn new(
        offered_at: InstantMillis,
        attempt_timeout: RemoteControlPairingAttemptTimeout,
        pairing_window: RemoteControlControllerPairingWindow,
    ) -> Result<Self, RemoteControlControllerPairingAttemptWindowError> {
        if offered_at >= pairing_window.expires_at() {
            return Err(
                RemoteControlControllerPairingAttemptWindowError::PairingWindowElapsed {
                    offered_at,
                    pairing_expires_at: pairing_window.expires_at(),
                },
            );
        }
        let timeout_expires_at = offered_at.saturating_add(attempt_timeout.duration());
        let expires_at = core::cmp::min(timeout_expires_at, pairing_window.expires_at());
        Ok(Self {
            offered_at,
            attempt_timeout,
            expires_at,
        })
    }

    #[must_use]
    pub const fn offered_at(self) -> InstantMillis {
        self.offered_at
    }

    #[must_use]
    pub const fn attempt_timeout(self) -> RemoteControlPairingAttemptTimeout {
        self.attempt_timeout
    }

    #[must_use]
    pub const fn expires_at(self) -> InstantMillis {
        self.expires_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingActivity {
    AwaitingOffer,
    AwaitingApproval {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AwaitingCompletion {
        attempt_id: RemoteControlPairingAttemptId,
    },
    Persisting {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerPairingBeginView<'a> {
    pub(super) context: RemoteControlPairingContext,
    pub(super) begin: &'a RemoteControlPairingBegin,
    pub(super) window: RemoteControlControllerPairingWindow,
}

impl<'a> RemoteControlControllerPairingBeginView<'a> {
    #[must_use]
    pub const fn context(self) -> RemoteControlPairingContext {
        self.context
    }

    #[must_use]
    pub const fn controller(self) -> &'a RemoteControlControllerIdentity {
        self.begin.controller()
    }

    #[must_use]
    pub const fn begin(self) -> &'a RemoteControlPairingBegin {
        self.begin
    }

    #[must_use]
    pub const fn window(self) -> RemoteControlControllerPairingWindow {
        self.window
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerPairingAttemptView<'a> {
    pub(super) transcript: &'a RemoteControlPairingTranscript,
    pub(super) window: RemoteControlControllerPairingAttemptWindow,
}

impl<'a> RemoteControlControllerPairingAttemptView<'a> {
    #[must_use]
    pub fn attempt_id(self) -> RemoteControlPairingAttemptId {
        self.transcript.into()
    }

    #[must_use]
    pub const fn context(self) -> RemoteControlPairingContext {
        self.transcript.context()
    }

    #[must_use]
    pub const fn controller(self) -> &'a RemoteControlControllerIdentity {
        self.transcript.controller()
    }

    #[must_use]
    pub const fn target(self) -> &'a RemoteControlTargetIdentity {
        self.transcript.target()
    }

    #[must_use]
    pub const fn permissions(self) -> &'a RemoteControlPairingPermissions {
        self.transcript.permissions()
    }

    #[must_use]
    pub fn confirmation_code(self) -> RemoteControlPairingConfirmationCode {
        self.transcript.confirmation_code()
    }

    #[must_use]
    pub const fn window(self) -> RemoteControlControllerPairingAttemptWindow {
        self.window
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerPairingPersistenceView<'a> {
    pub(super) attempt_id: RemoteControlPairingAttemptId,
    pub(super) access: &'a RemoteControlTargetAccess,
}

impl<'a> RemoteControlControllerPairingPersistenceView<'a> {
    #[must_use]
    pub const fn attempt_id(self) -> RemoteControlPairingAttemptId {
        self.attempt_id
    }

    #[must_use]
    pub const fn access(self) -> &'a RemoteControlTargetAccess {
        self.access
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingView<'a> {
    Idle,
    AwaitingOffer(RemoteControlControllerPairingBeginView<'a>),
    AwaitingApproval(RemoteControlControllerPairingAttemptView<'a>),
    AwaitingCompletion(RemoteControlControllerPairingAttemptView<'a>),
    Persisting(RemoteControlControllerPairingPersistenceView<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingAborted {
    AwaitingOffer {
        context: RemoteControlPairingContext,
    },
    AwaitingApproval {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
    AwaitingCompletion {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
}

impl RemoteControlControllerPairingAborted {
    #[must_use]
    pub const fn context(self) -> RemoteControlPairingContext {
        match self {
            Self::AwaitingOffer { context }
            | Self::AwaitingApproval { context, .. }
            | Self::AwaitingCompletion { context, .. } => context,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BeginRemoteControlControllerPairingOutcome {
    BeginOwed {
        begin: RemoteControlPairingBegin,
    },
    Busy {
        active: RemoteControlControllerPairingActivity,
    },
    PairingUnavailable {
        reason: RemoteControlControllerPairingWindowError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveRemoteControlControllerPairingOfferOutcome {
    ConfirmationRequired {
        attempt_id: RemoteControlPairingAttemptId,
    },
    Expired {
        expired: RemoteControlControllerPairingAborted,
    },
    Rejected {
        reason: RemoteControlPairingOfferVerificationError,
    },
    NoActiveAttempt,
    Unexpected {
        active: RemoteControlControllerPairingActivity,
    },
    PairingUnavailable {
        reason: RemoteControlControllerPairingAttemptWindowError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproveRemoteControlControllerPairingOutcome {
    CommitOwed {
        attempt_id: RemoteControlPairingAttemptId,
        commit: RemoteControlPairingCommit,
    },
    Expired {
        expired: RemoteControlControllerPairingAborted,
    },
    NoActiveAttempt,
    OfferNotReceived,
    AttemptMismatch {
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    AlreadyApproved {
        attempt_id: RemoteControlPairingAttemptId,
    },
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectRemoteControlControllerPairingOutcome {
    Rejected {
        aborted: RemoteControlControllerPairingAborted,
    },
    Expired {
        expired: RemoteControlControllerPairingAborted,
    },
    NoActiveAttempt,
    OfferNotReceived,
    AttemptMismatch {
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    AlreadyApproved {
        attempt_id: RemoteControlPairingAttemptId,
    },
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveRemoteControlControllerPairingCompletedOutcome {
    PersistenceOwed {
        attempt_id: RemoteControlPairingAttemptId,
    },
    Expired {
        expired: RemoteControlControllerPairingAborted,
    },
    Rejected {
        attempt_id: RemoteControlPairingAttemptId,
        reason: RemoteControlPairingCompletedVerificationError,
    },
    NoActiveAttempt,
    OfferNotReceived,
    ApprovalRequired {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AlreadyReceived {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum PersistRemoteControlControllerPairingOutcome {
    Completed {
        attempt_id: RemoteControlPairingAttemptId,
        access: RemoteControlTargetAccess,
    },
    NoPersistenceOwed,
    AttemptMismatch {
        persisted: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum FailRemoteControlControllerPairingPersistenceOutcome {
    Failed {
        attempt_id: RemoteControlPairingAttemptId,
        access: RemoteControlTargetAccess,
    },
    NoPersistenceOwed,
    AttemptMismatch {
        failed: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpireRemoteControlControllerPairingOutcome {
    Expired {
        aborted: RemoteControlControllerPairingAborted,
    },
    NotDue {
        expires_at: InstantMillis,
    },
    NoActiveAttempt,
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseRemoteControlControllerPairingLinkOutcome {
    Aborted {
        aborted: RemoteControlControllerPairingAborted,
    },
    UnrelatedLink,
    NoActiveAttempt,
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailRemoteControlControllerPairingRequestOutcome {
    Aborted {
        aborted: RemoteControlControllerPairingAborted,
    },
    UnrelatedLink,
    NoActiveAttempt,
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}
