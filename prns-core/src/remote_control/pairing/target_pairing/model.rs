use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity, RemoteControlTargetIdentity,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::units::InstantMillis;

use super::super::{
    RemoteControlPairingAttemptTimeout, RemoteControlPairingCommit, RemoteControlPairingCompleted,
    RemoteControlPairingCompletionSigningError, RemoteControlPairingConfirmationCode,
    RemoteControlPairingContext, RemoteControlPairingPermissions, RemoteControlPairingTranscript,
    RemoteControlPairingTranscriptDigest, RemoteControlPairingWindow,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetPairingAttemptWindowError {
    PairingWindowElapsed {
        started_at: InstantMillis,
        pairing_expires_at: InstantMillis,
    },
    DeadlineOverflow {
        started_at: InstantMillis,
        attempt_timeout: RemoteControlPairingAttemptTimeout,
    },
    ExceedsPairingWindow {
        attempt_expires_at: InstantMillis,
        pairing_expires_at: InstantMillis,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlTargetPairingAttemptWindow {
    pub(super) started_at: InstantMillis,
    pub(super) attempt_timeout: RemoteControlPairingAttemptTimeout,
    pub(super) expires_at: InstantMillis,
}

impl RemoteControlTargetPairingAttemptWindow {
    pub fn new(
        started_at: InstantMillis,
        attempt_timeout: RemoteControlPairingAttemptTimeout,
        pairing_window: &RemoteControlPairingWindow,
    ) -> Result<Self, RemoteControlTargetPairingAttemptWindowError> {
        let pairing_expires_at = pairing_window.expires_at();
        if started_at >= pairing_expires_at {
            return Err(
                RemoteControlTargetPairingAttemptWindowError::PairingWindowElapsed {
                    started_at,
                    pairing_expires_at,
                },
            );
        }
        let Some(expires_at) = started_at
            .0
            .checked_add(attempt_timeout.duration().0)
            .map(InstantMillis)
        else {
            return Err(
                RemoteControlTargetPairingAttemptWindowError::DeadlineOverflow {
                    started_at,
                    attempt_timeout,
                },
            );
        };
        if expires_at > pairing_expires_at {
            return Err(
                RemoteControlTargetPairingAttemptWindowError::ExceedsPairingWindow {
                    attempt_expires_at: expires_at,
                    pairing_expires_at,
                },
            );
        }
        Ok(Self {
            started_at,
            attempt_timeout,
            expires_at,
        })
    }

    #[must_use]
    pub const fn started_at(self) -> InstantMillis {
        self.started_at
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RemoteControlTargetPairingAttemptId(RemoteControlPairingTranscriptDigest);

impl RemoteControlTargetPairingAttemptId {
    #[must_use]
    pub const fn transcript(self) -> RemoteControlPairingTranscriptDigest {
        self.0
    }
}

impl From<&RemoteControlPairingTranscript> for RemoteControlTargetPairingAttemptId {
    fn from(transcript: &RemoteControlPairingTranscript) -> Self {
        Self(transcript.digest())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlTargetPairingAttemptView<'a> {
    pub(super) transcript: &'a RemoteControlPairingTranscript,
    pub(super) window: RemoteControlTargetPairingAttemptWindow,
}

impl<'a> RemoteControlTargetPairingAttemptView<'a> {
    #[must_use]
    pub fn attempt_id(self) -> RemoteControlTargetPairingAttemptId {
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
    pub const fn window(self) -> RemoteControlTargetPairingAttemptWindow {
        self.window
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetPairingView<'a> {
    Idle,
    AwaitingBoth(RemoteControlTargetPairingAttemptView<'a>),
    AwaitingTargetApproval(RemoteControlTargetPairingAttemptView<'a>),
    AwaitingControllerCommit(RemoteControlTargetPairingAttemptView<'a>),
    Authorizing(RemoteControlTargetPairingAttemptView<'a>),
    Completing(RemoteControlTargetPairingAttemptView<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlTargetPairingResponder {
    link_id: LinkId,
    request_id: RequestId,
}

impl RemoteControlTargetPairingResponder {
    #[must_use]
    pub const fn new(link_id: LinkId, request_id: RequestId) -> Self {
        Self {
            link_id,
            request_id,
        }
    }

    #[must_use]
    pub const fn link_id(self) -> LinkId {
        self.link_id
    }

    #[must_use]
    pub const fn request_id(self) -> RequestId {
        self.request_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlTargetPairingCommitArrival {
    pub(super) commit: RemoteControlPairingCommit,
    pub(super) responder: RemoteControlTargetPairingResponder,
}

impl RemoteControlTargetPairingCommitArrival {
    #[must_use]
    pub const fn new(
        commit: RemoteControlPairingCommit,
        responder: RemoteControlTargetPairingResponder,
    ) -> Self {
        Self { commit, responder }
    }

    #[must_use]
    pub const fn commit(self) -> RemoteControlPairingCommit {
        self.commit
    }

    #[must_use]
    pub const fn responder(self) -> RemoteControlTargetPairingResponder {
        self.responder
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetPairingAborted {
    AwaitingBoth {
        attempt_id: RemoteControlTargetPairingAttemptId,
        context: RemoteControlPairingContext,
    },
    AwaitingTargetApproval {
        attempt_id: RemoteControlTargetPairingAttemptId,
        context: RemoteControlPairingContext,
        responder: RemoteControlTargetPairingResponder,
    },
    AwaitingControllerCommit {
        attempt_id: RemoteControlTargetPairingAttemptId,
        context: RemoteControlPairingContext,
    },
}

impl RemoteControlTargetPairingAborted {
    #[must_use]
    pub const fn attempt_id(self) -> RemoteControlTargetPairingAttemptId {
        match self {
            Self::AwaitingBoth { attempt_id, .. }
            | Self::AwaitingTargetApproval { attempt_id, .. }
            | Self::AwaitingControllerCommit { attempt_id, .. } => attempt_id,
        }
    }

    #[must_use]
    pub const fn context(self) -> RemoteControlPairingContext {
        match self {
            Self::AwaitingBoth { context, .. }
            | Self::AwaitingTargetApproval { context, .. }
            | Self::AwaitingControllerCommit { context, .. } => context,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginRemoteControlTargetPairingOutcome {
    Offered {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
    Expired {
        expired: RemoteControlTargetPairingAborted,
    },
    Busy {
        active: RemoteControlTargetPairingAttemptId,
    },
    PairingUnavailable {
        reason: RemoteControlTargetPairingAttemptWindowError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproveRemoteControlTargetPairingOutcome {
    AwaitingControllerCommit {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
    AuthorizationOwed {
        attempt_id: RemoteControlTargetPairingAttemptId,
        grant: RemoteControlControllerGrant,
    },
    Expired {
        expired: RemoteControlTargetPairingAborted,
    },
    NoActiveAttempt,
    AttemptMismatch {
        requested: RemoteControlTargetPairingAttemptId,
        active: RemoteControlTargetPairingAttemptId,
    },
    AlreadyApproved {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
    FinalizationInProgress {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetPairingCommitRejection {
    NoActiveAttempt,
    WrongLink {
        expected: LinkId,
        found: LinkId,
    },
    TranscriptMismatch {
        expected: RemoteControlTargetPairingAttemptId,
        found: RemoteControlPairingTranscriptDigest,
    },
    AlreadyCommitted,
    FinalizationInProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitRemoteControlTargetPairingOutcome {
    AwaitingTargetApproval {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
    AuthorizationOwed {
        attempt_id: RemoteControlTargetPairingAttemptId,
        grant: RemoteControlControllerGrant,
    },
    Expired {
        expired: RemoteControlTargetPairingAborted,
        rejected: RemoteControlTargetPairingResponder,
    },
    Rejected {
        rejected: RemoteControlTargetPairingResponder,
        reason: RemoteControlTargetPairingCommitRejection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectRemoteControlTargetPairingOutcome {
    Rejected {
        aborted: RemoteControlTargetPairingAborted,
    },
    Expired {
        expired: RemoteControlTargetPairingAborted,
    },
    NoActiveAttempt,
    AttemptMismatch {
        requested: RemoteControlTargetPairingAttemptId,
        active: RemoteControlTargetPairingAttemptId,
    },
    AlreadyApproved {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
    FinalizationInProgress {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistRemoteControlTargetPairingAuthorizationOutcome {
    CompletionOwed {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
    SigningFailed {
        attempt_id: RemoteControlTargetPairingAttemptId,
        error: RemoteControlPairingCompletionSigningError,
    },
    NoAuthorizationOwed,
    AttemptMismatch {
        settled: RemoteControlTargetPairingAttemptId,
        active: RemoteControlTargetPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailRemoteControlTargetPairingAuthorizationOutcome {
    Aborted {
        attempt_id: RemoteControlTargetPairingAttemptId,
        responder: RemoteControlTargetPairingResponder,
    },
    NoAuthorizationOwed,
    AttemptMismatch {
        settled: RemoteControlTargetPairingAttemptId,
        active: RemoteControlTargetPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlTargetPairingCompletionView<'a> {
    pub(super) attempt: RemoteControlTargetPairingAttemptView<'a>,
    pub(super) responder: RemoteControlTargetPairingResponder,
    pub(super) completed: &'a RemoteControlPairingCompleted,
}

impl<'a> RemoteControlTargetPairingCompletionView<'a> {
    #[must_use]
    pub const fn attempt(self) -> RemoteControlTargetPairingAttemptView<'a> {
        self.attempt
    }

    #[must_use]
    pub const fn responder(self) -> RemoteControlTargetPairingResponder {
        self.responder
    }

    #[must_use]
    pub const fn completed(self) -> &'a RemoteControlPairingCompleted {
        self.completed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteRemoteControlTargetPairingOutcome {
    Completed {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
    NoCompletionOwed,
    AttemptMismatch {
        settled: RemoteControlTargetPairingAttemptId,
        active: RemoteControlTargetPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpireRemoteControlTargetPairingOutcome {
    Expired {
        aborted: RemoteControlTargetPairingAborted,
    },
    NotDue {
        expires_at: InstantMillis,
    },
    NoActiveAttempt,
    FinalizationInProgress {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseRemoteControlTargetPairingLinkOutcome {
    Aborted {
        aborted: RemoteControlTargetPairingAborted,
    },
    UnrelatedLink,
    NoActiveAttempt,
    FinalizationInProgress {
        attempt_id: RemoteControlTargetPairingAttemptId,
    },
}
