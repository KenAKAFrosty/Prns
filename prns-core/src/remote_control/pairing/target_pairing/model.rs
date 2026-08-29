use crate::identity::IdentityHash;
use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity, RemoteControlTargetIdentity,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::units::InstantMillis;

use super::super::{
    RemoteControlPairingAttemptId, RemoteControlPairingAttemptTimeout, RemoteControlPairingBegin,
    RemoteControlPairingCommit, RemoteControlPairingCompleted,
    RemoteControlPairingCompletionSigningError, RemoteControlPairingConfirmationCode,
    RemoteControlPairingContext, RemoteControlPairingOffer, RemoteControlPairingPermissions,
    RemoteControlPairingTranscript, RemoteControlPairingTranscriptDigest,
    RemoteControlPairingWindow,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlTargetPairingAttemptView<'a> {
    pub(super) transcript: &'a RemoteControlPairingTranscript,
    pub(super) window: RemoteControlTargetPairingAttemptWindow,
}

impl<'a> RemoteControlTargetPairingAttemptView<'a> {
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
    pub const fn window(self) -> RemoteControlTargetPairingAttemptWindow {
        self.window
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetPairingView<'a> {
    Idle,
    OfferPrepared(RemoteControlTargetPairingAttemptView<'a>),
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

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlTargetPairingBeginArrival {
    pub(super) begin: RemoteControlPairingBegin,
    pub(super) responder: RemoteControlTargetPairingResponder,
    pub(super) identified_controller: IdentityHash,
}

impl RemoteControlTargetPairingBeginArrival {
    #[must_use]
    pub const fn new(
        begin: RemoteControlPairingBegin,
        responder: RemoteControlTargetPairingResponder,
        identified_controller: IdentityHash,
    ) -> Self {
        Self {
            begin,
            responder,
            identified_controller,
        }
    }

    #[must_use]
    pub const fn begin(&self) -> &RemoteControlPairingBegin {
        &self.begin
    }

    #[must_use]
    pub const fn responder(&self) -> RemoteControlTargetPairingResponder {
        self.responder
    }

    #[must_use]
    pub const fn identified_controller(&self) -> IdentityHash {
        self.identified_controller
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
    OfferPrepared {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
    AwaitingBoth {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
    AwaitingTargetApproval {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
        responder: RemoteControlTargetPairingResponder,
    },
    AwaitingControllerCommit {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
}

impl RemoteControlTargetPairingAborted {
    #[must_use]
    pub const fn attempt_id(self) -> RemoteControlPairingAttemptId {
        match self {
            Self::OfferPrepared { attempt_id, .. }
            | Self::AwaitingBoth { attempt_id, .. }
            | Self::AwaitingTargetApproval { attempt_id, .. }
            | Self::AwaitingControllerCommit { attempt_id, .. } => attempt_id,
        }
    }

    #[must_use]
    pub const fn context(self) -> RemoteControlPairingContext {
        match self {
            Self::OfferPrepared { context, .. }
            | Self::AwaitingBoth { context, .. }
            | Self::AwaitingTargetApproval { context, .. }
            | Self::AwaitingControllerCommit { context, .. } => context,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BeginRemoteControlTargetPairingOutcome {
    OfferPrepared {
        attempt_id: RemoteControlPairingAttemptId,
        responder: RemoteControlTargetPairingResponder,
        offer: RemoteControlPairingOffer,
    },
    Expired {
        expired: RemoteControlTargetPairingAborted,
    },
    Busy {
        active: RemoteControlPairingAttemptId,
    },
    PairingUnavailable {
        reason: RemoteControlTargetPairingAttemptWindowError,
    },
    Rejected {
        rejected: RemoteControlTargetPairingResponder,
        reason: RemoteControlTargetPairingBeginRejection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchRemoteControlTargetPairingOfferOutcome<'a> {
    AwaitingConfirmation {
        attempt: RemoteControlTargetPairingAttemptView<'a>,
    },
    AttemptMismatch {
        dispatched: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    NoOfferPrepared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailRemoteControlTargetPairingOfferDispatchOutcome {
    Aborted {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AttemptMismatch {
        failed: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    NoOfferPrepared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetPairingBeginRejection {
    ControllerIdentityMismatch {
        claimed: IdentityHash,
        identified: IdentityHash,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproveRemoteControlTargetPairingOutcome {
    AwaitingControllerCommit {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AuthorizationOwed {
        attempt_id: RemoteControlPairingAttemptId,
        grant: RemoteControlControllerGrant,
    },
    Expired {
        expired: RemoteControlTargetPairingAborted,
    },
    NoActiveAttempt,
    AttemptMismatch {
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    OfferPendingDispatch {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AlreadyApproved {
        attempt_id: RemoteControlPairingAttemptId,
    },
    FinalizationInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetPairingCommitRejection {
    NoActiveAttempt,
    OfferPendingDispatch,
    WrongLink {
        expected: LinkId,
        found: LinkId,
    },
    TranscriptMismatch {
        expected: RemoteControlPairingAttemptId,
        found: RemoteControlPairingTranscriptDigest,
    },
    AlreadyCommitted,
    FinalizationInProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitRemoteControlTargetPairingOutcome {
    AwaitingTargetApproval {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AuthorizationOwed {
        attempt_id: RemoteControlPairingAttemptId,
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
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    OfferPendingDispatch {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AlreadyApproved {
        attempt_id: RemoteControlPairingAttemptId,
    },
    FinalizationInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistRemoteControlTargetPairingAuthorizationOutcome {
    CompletionOwed {
        attempt_id: RemoteControlPairingAttemptId,
        responder: RemoteControlTargetPairingResponder,
        completed: RemoteControlPairingCompleted,
    },
    SigningFailed {
        attempt_id: RemoteControlPairingAttemptId,
        error: RemoteControlPairingCompletionSigningError,
    },
    NoAuthorizationOwed,
    AttemptMismatch {
        settled: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailRemoteControlTargetPairingAuthorizationOutcome {
    Aborted {
        attempt_id: RemoteControlPairingAttemptId,
        responder: RemoteControlTargetPairingResponder,
    },
    NoAuthorizationOwed,
    AttemptMismatch {
        settled: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteRemoteControlTargetPairingOutcome {
    Completed {
        attempt_id: RemoteControlPairingAttemptId,
    },
    NoCompletionOwed,
    AttemptMismatch {
        settled: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
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
        attempt_id: RemoteControlPairingAttemptId,
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
        attempt_id: RemoteControlPairingAttemptId,
    },
}
