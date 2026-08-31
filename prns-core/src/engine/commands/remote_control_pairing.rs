use crate::identity::held::HoldIdentityError;
use crate::remote_control::{
    RemoteControlControllerGrant, RemoteControlPairingAttemptId,
    RemoteControlPairingAttemptTimeout, RemoteControlPairingAvailabilityWriteError,
    RemoteControlPairingCompletionSigningError, RemoteControlPairingEndpoint,
    RemoteControlPairingExpiresAfter, RemoteControlPairingInvitationCode,
    RemoteControlPairingPermissions, RemoteControlPairingPublicAppDataBytes,
    RemoteControlTargetPairingAborted, RemoteControlTargetPairingResponder,
};
use crate::routing::delivery::send_plain::SendPlainPacketWriteError;
use crate::routing::links::LinkId;
use crate::routing::upstream_app_destinations::RegisterDestinationError;
use crate::units::InstantMillis;
use crate::units::LinkCount;

use super::{EgressTarget, EgressTargetRejection, PrnsCommand, Settleable, Settlement};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRemoteControlPairing {
    pub target: EgressTarget,
    pub expires_after: RemoteControlPairingExpiresAfter,
    pub attempt_timeout: RemoteControlPairingAttemptTimeout,
    pub permissions: RemoteControlPairingPermissions,
    pub public_app_data: RemoteControlPairingPublicAppDataBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlPairingOpened {
    pub endpoint: RemoteControlPairingEndpoint,
    pub expires_at: InstantMillis,
    pub invitation_code: RemoteControlPairingInvitationCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenRemoteControlPairingRejection {
    Unavailable,
    AlreadyOpen,
    NoTransmittingInterfaces,
    EgressTarget(EgressTargetRejection),
    AttemptTimeoutExceedsWindow {
        attempt_timeout: RemoteControlPairingAttemptTimeout,
        pairing_expires_after: RemoteControlPairingExpiresAfter,
    },
    DeadlineOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenRemoteControlPairingFailure {
    Rejected(OpenRemoteControlPairingRejection),
    IdentityGenerationExhausted,
    HoldIdentity(HoldIdentityError),
    RegisterEndpoint(RegisterDestinationError),
    ConfigureRequestLimit,
    RegisterRequestEndpoint(crate::storage::TablePushError),
    WriteAvailability(RemoteControlPairingAvailabilityWriteError),
    PayloadCapacity,
    WritePacket(SendPlainPacketWriteError),
}

impl Settleable for OpenRemoteControlPairing {
    type Success = RemoteControlPairingOpened;
    type Failure = OpenRemoteControlPairingFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::OpenRemoteControlPairing(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<RemoteControlPairingOpened, OpenRemoteControlPairingFailure>> {
        match settlement {
            Settlement::OpenRemoteControlPairing(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_)
            | Settlement::CloseRemoteControlPairing(_)
            | Settlement::ApproveRemoteControlTargetPairing(_)
            | Settlement::RejectRemoteControlTargetPairing(_)
            | Settlement::SettleRemoteControlTargetPairingAuthorization(_)
            | Settlement::BeginRemoteControlControllerPairing(_)
            | Settlement::ApproveRemoteControlControllerPairing(_)
            | Settlement::RejectRemoteControlControllerPairing(_)
            | Settlement::RemoteControlControllerPairingRequest(_)
            | Settlement::SettleRemoteControlControllerPairingPersistence(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseRemoteControlPairing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseRemoteControlPairingOutcome {
    Closed {
        endpoint: RemoteControlPairingEndpoint,
    },
    AlreadyClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseRemoteControlPairingFailure {
    Unavailable,
    RetirementIncomplete {
        first_remaining_link: LinkId,
        retired_links: LinkCount,
    },
    EndpointNotRegistered,
    IdentityNotHeld,
}

impl Settleable for CloseRemoteControlPairing {
    type Success = CloseRemoteControlPairingOutcome;
    type Failure = CloseRemoteControlPairingFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::CloseRemoteControlPairing(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<CloseRemoteControlPairingOutcome, CloseRemoteControlPairingFailure>> {
        match settlement {
            Settlement::CloseRemoteControlPairing(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_)
            | Settlement::OpenRemoteControlPairing(_)
            | Settlement::ApproveRemoteControlTargetPairing(_)
            | Settlement::RejectRemoteControlTargetPairing(_)
            | Settlement::SettleRemoteControlTargetPairingAuthorization(_)
            | Settlement::BeginRemoteControlControllerPairing(_)
            | Settlement::ApproveRemoteControlControllerPairing(_)
            | Settlement::RejectRemoteControlControllerPairing(_)
            | Settlement::RemoteControlControllerPairingRequest(_)
            | Settlement::SettleRemoteControlControllerPairingPersistence(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApproveRemoteControlTargetPairing {
    pub attempt_id: RemoteControlPairingAttemptId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetPairingApproval {
    AwaitingControllerCommit {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AuthorizationOwed {
        attempt_id: RemoteControlPairingAttemptId,
        grant: RemoteControlControllerGrant,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproveRemoteControlTargetPairingFailure {
    Expired {
        expired: RemoteControlTargetPairingAborted,
        retired_link: LinkId,
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
    CompletionRetentionExpired {
        attempt_id: RemoteControlPairingAttemptId,
        retired_link: LinkId,
    },
}

impl Settleable for ApproveRemoteControlTargetPairing {
    type Success = RemoteControlTargetPairingApproval;
    type Failure = ApproveRemoteControlTargetPairingFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::ApproveRemoteControlTargetPairing(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<RemoteControlTargetPairingApproval, ApproveRemoteControlTargetPairingFailure>>
    {
        match settlement {
            Settlement::ApproveRemoteControlTargetPairing(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_)
            | Settlement::OpenRemoteControlPairing(_)
            | Settlement::CloseRemoteControlPairing(_)
            | Settlement::RejectRemoteControlTargetPairing(_)
            | Settlement::SettleRemoteControlTargetPairingAuthorization(_)
            | Settlement::BeginRemoteControlControllerPairing(_)
            | Settlement::ApproveRemoteControlControllerPairing(_)
            | Settlement::RejectRemoteControlControllerPairing(_)
            | Settlement::RemoteControlControllerPairingRequest(_)
            | Settlement::SettleRemoteControlControllerPairingPersistence(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RejectRemoteControlTargetPairing {
    pub attempt_id: RemoteControlPairingAttemptId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlTargetPairingRejection {
    pub aborted: RemoteControlTargetPairingAborted,
    pub retired_link: LinkId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectRemoteControlTargetPairingFailure {
    Expired {
        expired: RemoteControlTargetPairingAborted,
        retired_link: LinkId,
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
    CompletionRetentionExpired {
        attempt_id: RemoteControlPairingAttemptId,
        retired_link: LinkId,
    },
}

impl Settleable for RejectRemoteControlTargetPairing {
    type Success = RemoteControlTargetPairingRejection;
    type Failure = RejectRemoteControlTargetPairingFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::RejectRemoteControlTargetPairing(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<RemoteControlTargetPairingRejection, RejectRemoteControlTargetPairingFailure>>
    {
        match settlement {
            Settlement::RejectRemoteControlTargetPairing(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_)
            | Settlement::OpenRemoteControlPairing(_)
            | Settlement::CloseRemoteControlPairing(_)
            | Settlement::ApproveRemoteControlTargetPairing(_)
            | Settlement::SettleRemoteControlTargetPairingAuthorization(_)
            | Settlement::BeginRemoteControlControllerPairing(_)
            | Settlement::ApproveRemoteControlControllerPairing(_)
            | Settlement::RejectRemoteControlControllerPairing(_)
            | Settlement::RemoteControlControllerPairingRequest(_)
            | Settlement::SettleRemoteControlControllerPairingPersistence(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetPairingAuthorizationPersistence {
    Persisted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettleRemoteControlTargetPairingAuthorization {
    pub attempt_id: RemoteControlPairingAttemptId,
    pub persistence: RemoteControlTargetPairingAuthorizationPersistence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlTargetPairingFinalization {
    CompletionDispatched {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AuthorizationRollbackRequired {
        attempt_id: RemoteControlPairingAttemptId,
        grant: RemoteControlControllerGrant,
    },
    AuthorizationFailureRecorded {
        attempt_id: RemoteControlPairingAttemptId,
        responder: RemoteControlTargetPairingResponder,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleRemoteControlTargetPairingAuthorizationFailure {
    NoAuthorizationOwed {
        settled: RemoteControlPairingAttemptId,
    },
    AttemptMismatch {
        settled: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    TargetSignerUnavailable {
        attempt_id: RemoteControlPairingAttemptId,
        target_identity: crate::identity::IdentityHash,
    },
    CompletionSigningFailed {
        attempt_id: RemoteControlPairingAttemptId,
        error: RemoteControlPairingCompletionSigningError,
    },
    CompletionRetentionExpired {
        attempt_id: RemoteControlPairingAttemptId,
        retired_link: LinkId,
    },
    CompletionDispatchFailed {
        attempt_id: RemoteControlPairingAttemptId,
        failure: crate::engine::RemoteControlPairingResponseDispatchFailure,
    },
}

impl Settleable for SettleRemoteControlTargetPairingAuthorization {
    type Success = RemoteControlTargetPairingFinalization;
    type Failure = SettleRemoteControlTargetPairingAuthorizationFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::SettleRemoteControlTargetPairingAuthorization(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<RemoteControlTargetPairingFinalization, Self::Failure>> {
        match settlement {
            Settlement::SettleRemoteControlTargetPairingAuthorization(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SetRegisteredAnnounceAppData(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_)
            | Settlement::OpenRemoteControlPairing(_)
            | Settlement::CloseRemoteControlPairing(_)
            | Settlement::ApproveRemoteControlTargetPairing(_)
            | Settlement::RejectRemoteControlTargetPairing(_)
            | Settlement::BeginRemoteControlControllerPairing(_)
            | Settlement::ApproveRemoteControlControllerPairing(_)
            | Settlement::RejectRemoteControlControllerPairing(_)
            | Settlement::RemoteControlControllerPairingRequest(_)
            | Settlement::SettleRemoteControlControllerPairingPersistence(_) => None,
        }
    }
}
