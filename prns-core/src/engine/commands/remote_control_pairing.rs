use crate::identity::held::HoldIdentityError;
use crate::remote_control::{
    RemoteControlPairingAttemptTimeout, RemoteControlPairingAvailabilityWriteError,
    RemoteControlPairingEndpoint, RemoteControlPairingExpiresAfter,
    RemoteControlPairingPermissions, RemoteControlPairingPublicAppDataBytes,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingOpened {
    pub endpoint: RemoteControlPairingEndpoint,
    pub expires_at: InstantMillis,
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
            | Settlement::CloseRemoteControlPairing(_) => None,
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
            | Settlement::OpenRemoteControlPairing(_) => None,
        }
    }
}
