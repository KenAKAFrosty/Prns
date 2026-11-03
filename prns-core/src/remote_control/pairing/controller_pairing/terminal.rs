use super::RemoteControlControllerPairingAborted;
use crate::identity::IdentityHash;
use crate::remote_control::{RemoteControlPairingAttemptId, RemoteControlPairingContext};

/// The exchange link identifies an invitation before the authenticated offer
/// supplies its transcript-derived attempt id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingSubject {
    AwaitingOffer {
        context: RemoteControlPairingContext,
    },
    Attempt {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
}

impl From<RemoteControlControllerPairingAborted> for RemoteControlControllerPairingSubject {
    fn from(aborted: RemoteControlControllerPairingAborted) -> Self {
        match aborted {
            RemoteControlControllerPairingAborted::AwaitingOffer { context } => {
                Self::AwaitingOffer { context }
            }
            RemoteControlControllerPairingAborted::AwaitingApproval {
                attempt_id,
                context,
            }
            | RemoteControlControllerPairingAborted::AwaitingCompletion {
                attempt_id,
                context,
            } => Self::Attempt {
                attempt_id,
                context,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingFailureReason {
    LinkClosed,
    RequestFailed,
    PersistenceFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPairingTerminalOutcome {
    Paired {
        target: IdentityHash,
    },
    Rejected,
    Expired,
    Cancelled,
    Failed {
        reason: RemoteControlControllerPairingFailureReason,
    },
}

/// One volatile result, retained until the next admitted invitation. Paired
/// means the exact target access was persisted, not merely accepted remotely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerPairingTerminal {
    pub subject: RemoteControlControllerPairingSubject,
    pub outcome: RemoteControlControllerPairingTerminalOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelRemoteControlControllerPairingError {
    StaleSubject,
    AlreadyApproved,
    PersistenceInProgress,
    Expired,
}
