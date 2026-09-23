use crate::crypto::{sha256, SHA256_OUTPUT_LEN};
use crate::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, InstantMillis, SendRequestFailure,
};
use crate::identity::IdentityHash;
use crate::interfaces::{DiscoveryGroupId, InterfaceId, InterfaceMode};
use crate::remote_control::{
    RemoteControlAnnounceSelfOutcome, RemoteControlApplyOutcome,
    RemoteControlAuthorizeControllerOutcome, RemoteControlBuildVersion,
    RemoteControlControllerAuthority, RemoteControlControllerGrant,
    RemoteControlControllerGrantTable, RemoteControlControllerIdentity,
    RemoteControlControllerInventory, RemoteControlControllerPage, RemoteControlDescription,
    RemoteControlDescriptionError, RemoteControlDiscoveryGroups,
    RemoteControlDiscoveryGroupsInventoryOutcome, RemoteControlDiscoveryGroupsReplaceOutcome,
    RemoteControlDisplayAutoOff, RemoteControlDisplayVisibility, RemoteControlEspRadioMode,
    RemoteControlGnssPower, RemoteControlGroupOutcome, RemoteControlInterfaceConfigOutcome,
    RemoteControlInterfaceGroup, RemoteControlInterfaceInventory, RemoteControlInterfacePage,
    RemoteControlInterfacePeersOutcome, RemoteControlInterfacePower, RemoteControlLoRaOutcome,
    RemoteControlLoRaProfile, RemoteControlMessageWriteError, RemoteControlModeOutcome,
    RemoteControlPathInventory, RemoteControlPathPage, RemoteControlPeerPage,
    RemoteControlPowerOutcome, RemoteControlProtocolError, RemoteControlRequest,
    RemoteControlRequestKind, RemoteControlRequestParseError, RemoteControlRequestSet,
    RemoteControlResponse, RemoteControlResponseKind, RemoteControlResponseParseError,
    RemoteControlRevokeControllerOutcome, RemoteControlSelfAnnouncement, RemoteControlSleepOutcome,
    RemoteControlStationUplink, RemoteControlSystemPower, RemoteControlWifiCredentialRevision,
    RemoteControlWifiStageOutcome, RemoteControlWifiStation, RemoteControlWifiStationOutcome,
    RemoteControlWifiTransactionStatus, RevokeRemoteControlControllerOutcome,
    SetRemoteControlControllerGrantOutcome, REMOTE_CONTROL_REQUEST_ENDPOINT_ID,
};
use crate::routing::links::request::REQUEST_WIRE_OVERHEAD;
use crate::units::ByteLimit;
use crate::wire::DestinationHash;
use prns_core::capabilities::power::PowerSnapshot;

use super::request_endpoints::{
    Decline, InboundRequest, RequestContext, RequestEndpoint, RequestEndpointPolicy, RespondToken,
    ResponseSink,
};
use super::{
    AnnounceNowError, PrnsNodeApi, RevokeRemoteControlControllerControlError, SendError,
    SetRemoteControlControllerGrantControlError,
};

pub(super) const REMOTE_CONTROL_REQUEST_PLAINTEXT_MAX: usize =
    REQUEST_WIRE_OVERHEAD.saturating_add(RemoteControlRequest::MAX_ENCODED_LEN);

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlError {
    Encode(RemoteControlMessageWriteError),
    Request(SendError<SendRequestFailure>),
    Response(RemoteControlResponseParseError),
    Remote(RemoteControlProtocolError),
    UnexpectedResponse {
        expected: RemoteControlResponseKind,
        found: RemoteControlResponseKind,
    },
    AnnounceSelf(RemoteControlAnnounceSelfFailure),
}

impl core::fmt::Display for RemoteControlError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Encode(error) => write!(
                formatter,
                "remote control request encoding failed: {error:?}"
            ),
            Self::Request(error) => write!(formatter, "remote control request failed: {error:?}"),
            Self::Response(error) => {
                write!(formatter, "remote control response was invalid: {error:?}")
            }
            Self::Remote(error) => write!(
                formatter,
                "remote control peer refused the request: {error:?}"
            ),
            Self::UnexpectedResponse { expected, found } => write!(
                formatter,
                "remote control response kind was {found:?}, expected {expected:?}"
            ),
            Self::AnnounceSelf(failure) => {
                write!(
                    formatter,
                    "remote control self-announcement failed: {failure:?}"
                )
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RemoteControlError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlAnnounceSelfFailure {
    Unavailable,
    Rejected,
    WriteFailed,
}

pub struct RemoteControlDescribe;

impl RemoteControlDescribe {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::Describe;
    pub const RESPONSE_CAPACITY: usize = RemoteControlResponse::MAX_ENCODED_LEN;
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlDescription, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::Describe(description) => Ok(description),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::Describe,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlAnnounceSelf;

impl RemoteControlAnnounceSelf {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::AnnounceSelf;
    pub const RESPONSE_CAPACITY: usize = Self::REQUEST.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<(), RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::AnnounceSelf(RemoteControlAnnounceSelfOutcome::Announced) => {
                Ok(())
            }
            RemoteControlResponse::AnnounceSelf(RemoteControlAnnounceSelfOutcome::Unavailable) => {
                Err(RemoteControlError::AnnounceSelf(
                    RemoteControlAnnounceSelfFailure::Unavailable,
                ))
            }
            RemoteControlResponse::AnnounceSelf(RemoteControlAnnounceSelfOutcome::Rejected) => Err(
                RemoteControlError::AnnounceSelf(RemoteControlAnnounceSelfFailure::Rejected),
            ),
            RemoteControlResponse::AnnounceSelf(RemoteControlAnnounceSelfOutcome::WriteFailed) => {
                Err(RemoteControlError::AnnounceSelf(
                    RemoteControlAnnounceSelfFailure::WriteFailed,
                ))
            }
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::AnnounceSelf,
                found: response.kind(),
            }),
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlHostCommand {
    InventoryInterfaces {
        page: RemoteControlInterfacePage,
    },
    SetInterfacePower {
        id: InterfaceId,
        power: RemoteControlInterfacePower,
    },
    SetInterfaceMode {
        id: InterfaceId,
        mode: InterfaceMode,
    },
    SetInterfaceGroup {
        id: InterfaceId,
        group: DiscoveryGroupId,
    },
    InventoryInterfaceDiscoveryGroups {
        id: InterfaceId,
    },
    ReplaceInterfaceDiscoveryGroups {
        id: InterfaceId,
        groups: RemoteControlDiscoveryGroups,
    },
    InventoryInterfacePeers {
        id: InterfaceId,
        page: RemoteControlPeerPage,
    },
    InventoryInterfaceConfig {
        id: InterfaceId,
    },
    SetInterfaceLoRaProfile {
        id: InterfaceId,
        profile: RemoteControlLoRaProfile,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    SetInterfaceWifiStation {
        id: InterfaceId,
        station: RemoteControlWifiStation,
    },
    DescribeBuild,
    DescribePower,
    SleepRadios,
    WakeRadios,
    SetSystemPower {
        power: RemoteControlSystemPower,
    },
    SetGnssPower {
        power: RemoteControlGnssPower,
    },
    SetDisplayVisibility {
        visibility: RemoteControlDisplayVisibility,
    },
    SetDisplayAutoOff {
        auto_off: RemoteControlDisplayAutoOff,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    SetStationUplink {
        id: InterfaceId,
        uplink: RemoteControlStationUplink,
    },
    SetEspRadioMode {
        mode: RemoteControlEspRadioMode,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    StageWifiCredentials {
        controller: IdentityHash,
        station: RemoteControlWifiStation,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    ActivateWifiCredentials {
        controller: IdentityHash,
        revision: RemoteControlWifiCredentialRevision,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    ConfirmWifiCredentials {
        controller: IdentityHash,
        revision: RemoteControlWifiCredentialRevision,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    CancelWifiCredentials {
        controller: IdentityHash,
        revision: RemoteControlWifiCredentialRevision,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    InspectWifiTransaction {
        controller: IdentityHash,
    },
}

impl RemoteControlHostCommand {
    #[must_use]
    pub const fn request_kind(&self) -> RemoteControlRequestKind {
        match self {
            Self::InventoryInterfaces { .. } => RemoteControlRequestKind::InventoryInterfaces,
            Self::SetInterfacePower { .. } => RemoteControlRequestKind::SetInterfacePower,
            Self::SetInterfaceMode { .. } => RemoteControlRequestKind::SetInterfaceMode,
            Self::SetInterfaceGroup { .. } => RemoteControlRequestKind::SetInterfaceGroup,
            Self::InventoryInterfaceDiscoveryGroups { .. } => {
                RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups
            }
            Self::ReplaceInterfaceDiscoveryGroups { .. } => {
                RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups
            }
            Self::InventoryInterfacePeers { .. } => {
                RemoteControlRequestKind::InventoryInterfacePeers
            }
            Self::InventoryInterfaceConfig { .. } => {
                RemoteControlRequestKind::InventoryInterfaceConfig
            }
            Self::SetInterfaceLoRaProfile { .. } => {
                RemoteControlRequestKind::SetInterfaceLoRaProfile
            }
            #[cfg(feature = "remote-control-wifi-host")]
            Self::SetInterfaceWifiStation { .. } => {
                RemoteControlRequestKind::SetInterfaceWifiStation
            }
            Self::DescribeBuild => RemoteControlRequestKind::DescribeBuild,
            Self::DescribePower => RemoteControlRequestKind::DescribePower,
            Self::SleepRadios => RemoteControlRequestKind::SleepRadios,
            Self::WakeRadios => RemoteControlRequestKind::WakeRadios,
            Self::SetSystemPower { .. } => RemoteControlRequestKind::SetSystemPower,
            Self::SetGnssPower { .. } => RemoteControlRequestKind::SetGnssPower,
            Self::SetDisplayVisibility { .. } => RemoteControlRequestKind::SetDisplayVisibility,
            Self::SetDisplayAutoOff { .. } => RemoteControlRequestKind::SetDisplayAutoOff,
            #[cfg(feature = "remote-control-wifi-host")]
            Self::SetStationUplink { .. } => RemoteControlRequestKind::SetStationUplink,
            Self::SetEspRadioMode { .. } => RemoteControlRequestKind::SetEspRadioMode,
            #[cfg(feature = "remote-control-wifi-host")]
            Self::StageWifiCredentials { .. } => RemoteControlRequestKind::StageWifiCredentials,
            #[cfg(feature = "remote-control-wifi-host")]
            Self::ActivateWifiCredentials { .. } => {
                RemoteControlRequestKind::ActivateWifiCredentials
            }
            #[cfg(feature = "remote-control-wifi-host")]
            Self::ConfirmWifiCredentials { .. } => RemoteControlRequestKind::ConfirmWifiCredentials,
            #[cfg(feature = "remote-control-wifi-host")]
            Self::CancelWifiCredentials { .. } => RemoteControlRequestKind::CancelWifiCredentials,
            #[cfg(feature = "remote-control-wifi-host")]
            Self::InspectWifiTransaction { .. } => RemoteControlRequestKind::InspectWifiTransaction,
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteControlHostResponse {
    InventoryInterfaces(RemoteControlInterfaceInventory),
    SetInterfacePower(RemoteControlPowerOutcome),
    SetInterfaceMode(RemoteControlModeOutcome),
    SetInterfaceGroup(RemoteControlGroupOutcome),
    InventoryInterfaceDiscoveryGroups(RemoteControlDiscoveryGroupsInventoryOutcome),
    ReplaceInterfaceDiscoveryGroups(RemoteControlDiscoveryGroupsReplaceOutcome),
    InventoryInterfacePeers(RemoteControlInterfacePeersOutcome),
    InventoryInterfaceConfig(RemoteControlInterfaceConfigOutcome),
    SetInterfaceLoRaProfile(RemoteControlLoRaOutcome),
    SetInterfaceWifiStation(RemoteControlWifiStationOutcome),
    DescribeBuild(RemoteControlBuildVersion),
    DescribePower(PowerSnapshot),
    SleepRadios(RemoteControlSleepOutcome),
    WakeRadios(RemoteControlSleepOutcome),
    SetSystemPower(RemoteControlApplyOutcome),
    SetGnssPower(RemoteControlApplyOutcome),
    SetDisplayVisibility(RemoteControlApplyOutcome),
    SetDisplayAutoOff(RemoteControlApplyOutcome),
    SetStationUplink(RemoteControlApplyOutcome),
    SetEspRadioMode(RemoteControlApplyOutcome),
    StageWifiCredentials(RemoteControlWifiStageOutcome),
    ActivateWifiCredentials(RemoteControlApplyOutcome),
    ConfirmWifiCredentials(RemoteControlApplyOutcome),
    CancelWifiCredentials(RemoteControlApplyOutcome),
    InspectWifiTransaction(RemoteControlWifiTransactionStatus),
}

impl RemoteControlHostResponse {
    #[must_use]
    pub const fn request_kind(&self) -> RemoteControlRequestKind {
        match self {
            Self::InventoryInterfaces(_) => RemoteControlRequestKind::InventoryInterfaces,
            Self::SetInterfacePower(_) => RemoteControlRequestKind::SetInterfacePower,
            Self::SetInterfaceMode(_) => RemoteControlRequestKind::SetInterfaceMode,
            Self::SetInterfaceGroup(_) => RemoteControlRequestKind::SetInterfaceGroup,
            Self::InventoryInterfaceDiscoveryGroups(_) => {
                RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups
            }
            Self::ReplaceInterfaceDiscoveryGroups(_) => {
                RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups
            }
            Self::InventoryInterfacePeers(_) => RemoteControlRequestKind::InventoryInterfacePeers,
            Self::InventoryInterfaceConfig(_) => RemoteControlRequestKind::InventoryInterfaceConfig,
            Self::SetInterfaceLoRaProfile(_) => RemoteControlRequestKind::SetInterfaceLoRaProfile,
            Self::SetInterfaceWifiStation(_) => RemoteControlRequestKind::SetInterfaceWifiStation,
            Self::DescribeBuild(_) => RemoteControlRequestKind::DescribeBuild,
            Self::DescribePower(_) => RemoteControlRequestKind::DescribePower,
            Self::SleepRadios(_) => RemoteControlRequestKind::SleepRadios,
            Self::WakeRadios(_) => RemoteControlRequestKind::WakeRadios,
            Self::SetSystemPower(_) => RemoteControlRequestKind::SetSystemPower,
            Self::SetGnssPower(_) => RemoteControlRequestKind::SetGnssPower,
            Self::SetDisplayVisibility(_) => RemoteControlRequestKind::SetDisplayVisibility,
            Self::SetDisplayAutoOff(_) => RemoteControlRequestKind::SetDisplayAutoOff,
            Self::SetStationUplink(_) => RemoteControlRequestKind::SetStationUplink,
            Self::SetEspRadioMode(_) => RemoteControlRequestKind::SetEspRadioMode,
            Self::StageWifiCredentials(_) => RemoteControlRequestKind::StageWifiCredentials,
            Self::ActivateWifiCredentials(_) => RemoteControlRequestKind::ActivateWifiCredentials,
            Self::ConfirmWifiCredentials(_) => RemoteControlRequestKind::ConfirmWifiCredentials,
            Self::CancelWifiCredentials(_) => RemoteControlRequestKind::CancelWifiCredentials,
            Self::InspectWifiTransaction(_) => RemoteControlRequestKind::InspectWifiTransaction,
        }
    }

    fn into_wire_response(self) -> RemoteControlResponse {
        match self {
            Self::InventoryInterfaces(inventory) => {
                RemoteControlResponse::InventoryInterfaces(inventory)
            }
            Self::SetInterfacePower(outcome) => RemoteControlResponse::SetInterfacePower(outcome),
            Self::SetInterfaceMode(outcome) => RemoteControlResponse::SetInterfaceMode(outcome),
            Self::SetInterfaceGroup(outcome) => RemoteControlResponse::SetInterfaceGroup(outcome),
            Self::InventoryInterfaceDiscoveryGroups(outcome) => {
                RemoteControlResponse::InventoryInterfaceDiscoveryGroups(outcome)
            }
            Self::ReplaceInterfaceDiscoveryGroups(outcome) => {
                RemoteControlResponse::ReplaceInterfaceDiscoveryGroups(outcome)
            }
            Self::InventoryInterfacePeers(outcome) => {
                RemoteControlResponse::InventoryInterfacePeers(outcome)
            }
            Self::InventoryInterfaceConfig(outcome) => {
                RemoteControlResponse::InventoryInterfaceConfig(outcome)
            }
            Self::SetInterfaceLoRaProfile(outcome) => {
                RemoteControlResponse::SetInterfaceLoRaProfile(outcome)
            }
            Self::SetInterfaceWifiStation(outcome) => {
                RemoteControlResponse::SetInterfaceWifiStation(outcome)
            }
            Self::DescribeBuild(version) => RemoteControlResponse::DescribeBuild(version),
            Self::DescribePower(snapshot) => RemoteControlResponse::DescribePower(snapshot),
            Self::SleepRadios(outcome) => RemoteControlResponse::SleepRadios(outcome),
            Self::WakeRadios(outcome) => RemoteControlResponse::WakeRadios(outcome),
            Self::SetSystemPower(outcome) => RemoteControlResponse::SetSystemPower(outcome),
            Self::SetGnssPower(outcome) => RemoteControlResponse::SetGnssPower(outcome),
            Self::SetDisplayVisibility(outcome) => {
                RemoteControlResponse::SetDisplayVisibility(outcome)
            }
            Self::SetDisplayAutoOff(outcome) => RemoteControlResponse::SetDisplayAutoOff(outcome),
            Self::SetStationUplink(outcome) => RemoteControlResponse::SetStationUplink(outcome),
            Self::SetEspRadioMode(outcome) => RemoteControlResponse::SetEspRadioMode(outcome),
            Self::StageWifiCredentials(outcome) => {
                RemoteControlResponse::StageWifiCredentials(outcome)
            }
            Self::ActivateWifiCredentials(outcome) => {
                RemoteControlResponse::ActivateWifiCredentials(outcome)
            }
            Self::ConfirmWifiCredentials(outcome) => {
                RemoteControlResponse::ConfirmWifiCredentials(outcome)
            }
            Self::CancelWifiCredentials(outcome) => {
                RemoteControlResponse::CancelWifiCredentials(outcome)
            }
            Self::InspectWifiTransaction(status) => {
                RemoteControlResponse::InspectWifiTransaction(status)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlHostCommandError {
    Unsupported,
    Busy,
    ApplyFailed,
    PersistenceFailed,
    RollbackFailed,
}

impl RemoteControlHostCommandError {
    fn into_protocol_error(self, request: RemoteControlRequestKind) -> RemoteControlProtocolError {
        match self {
            Self::Unsupported => RemoteControlProtocolError::UnsupportedRequest { request },
            Self::Busy => RemoteControlProtocolError::Busy { request },
            Self::ApplyFailed => RemoteControlProtocolError::ApplyFailed { request },
            Self::PersistenceFailed => RemoteControlProtocolError::PersistenceFailed { request },
            Self::RollbackFailed => RemoteControlProtocolError::RollbackFailed { request },
        }
    }
}

/// Executes Remote Control effects against state owned by the host.
///
/// Implementations must settle each command exactly once and return only the response variant
/// belonging to that command. Hardware-owning runtimes should enqueue commands into one bounded
/// executor rather than touching peripherals from the request task.
#[allow(async_fn_in_trait)]
pub trait RemoteControlHostControls {
    async fn execute_remote_control(
        &self,
        command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError>;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoRemoteControlHostControls;

impl RemoteControlHostControls for NoRemoteControlHostControls {
    async fn execute_remote_control(
        &self,
        _command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
        Err(RemoteControlHostCommandError::Unsupported)
    }
}

async fn execute_remote_control_host(
    state: &impl RemoteControlHostControls,
    command: RemoteControlHostCommand,
) -> RemoteControlResponse {
    let request = command.request_kind();
    match state.execute_remote_control(command).await {
        Ok(response) if response.request_kind() == request => response.into_wire_response(),
        Ok(_) => {
            RemoteControlResponse::ProtocolError(RemoteControlProtocolError::InternalFailure {
                request,
            })
        }
        Err(error) => RemoteControlResponse::ProtocolError(error.into_protocol_error(request)),
    }
}

pub struct RemoteControlInventoryInterfaces;

impl RemoteControlInventoryInterfaces {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::InventoryInterfaces {
        page: RemoteControlInterfacePage::First,
    };
    pub const RESPONSE_CAPACITY: usize = Self::REQUEST.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn write_page_request(
        page: RemoteControlInterfacePage,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::InventoryInterfaces { page }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlInterfaceInventory, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::InventoryInterfaces(inventory) => Ok(inventory),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::InventoryInterfaces,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlInventoryPathTable;

impl RemoteControlInventoryPathTable {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::InventoryPathTable {
        page: RemoteControlPathPage::First,
    };
    pub const RESPONSE_CAPACITY: usize = Self::REQUEST.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_page_request(
        page: RemoteControlPathPage,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::InventoryPathTable { page }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlPathInventory, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::InventoryPathTable(inventory) => Ok(inventory),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::InventoryPathTable,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlSetInterfacePower;

impl RemoteControlSetInterfacePower {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::SetInterfacePower.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(
        id: InterfaceId,
        power: RemoteControlInterfacePower,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::SetInterfacePower { id, power }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlPowerOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::SetInterfacePower(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::SetInterfacePower,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlSetInterfaceMode;

impl RemoteControlSetInterfaceMode {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::SetInterfaceMode.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(
        id: InterfaceId,
        mode: InterfaceMode,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::SetInterfaceMode { id, mode }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlModeOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::SetInterfaceMode(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::SetInterfaceMode,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlSetInterfaceGroup;

impl RemoteControlSetInterfaceGroup {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::SetInterfaceGroup.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(
        id: InterfaceId,
        group: RemoteControlInterfaceGroup,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::SetInterfaceGroup { id, group }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlGroupOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::SetInterfaceGroup(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::SetInterfaceGroup,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlInventoryInterfaceDiscoveryGroups;

impl RemoteControlInventoryInterfaceDiscoveryGroups {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(id: InterfaceId, out: &mut [u8]) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::InventoryInterfaceDiscoveryGroups { id }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlDiscoveryGroupsInventoryOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::InventoryInterfaceDiscoveryGroups(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::InventoryInterfaceDiscoveryGroups,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlReplaceInterfaceDiscoveryGroups;

impl RemoteControlReplaceInterfaceDiscoveryGroups {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(
        id: InterfaceId,
        groups: RemoteControlDiscoveryGroups,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::ReplaceInterfaceDiscoveryGroups { id, groups }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlDiscoveryGroupsReplaceOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::ReplaceInterfaceDiscoveryGroups(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::ReplaceInterfaceDiscoveryGroups,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlSetInterfaceWifiStation;

impl RemoteControlSetInterfaceWifiStation {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::SetInterfaceWifiStation.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(
        id: InterfaceId,
        station: RemoteControlWifiStation,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::SetInterfaceWifiStation { id, station }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlWifiStationOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::SetInterfaceWifiStation(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::SetInterfaceWifiStation,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlInventoryControllers;

impl RemoteControlInventoryControllers {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::InventoryControllers {
        page: RemoteControlControllerPage::First,
    };
    pub const RESPONSE_CAPACITY: usize = Self::REQUEST.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn write_page_request(
        page: RemoteControlControllerPage,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::InventoryControllers { page }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlControllerInventory, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::InventoryControllers(inventory) => Ok(inventory),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::InventoryControllers,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlAuthorizeController;

impl RemoteControlAuthorizeController {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::AuthorizeController.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(
        controller: RemoteControlControllerIdentity,
        permitted_requests: RemoteControlRequestSet,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::AuthorizeController {
            controller,
            permitted_requests,
        }
        .write_into(out)
        .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlAuthorizeControllerOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::AuthorizeController(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::AuthorizeController,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlRevokeController;

impl RemoteControlRevokeController {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::RevokeController.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(hash: IdentityHash, out: &mut [u8]) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::RevokeController { hash }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlRevokeControllerOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::RevokeController(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::RevokeController,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlSetInterfaceLoRaProfile;

impl RemoteControlSetInterfaceLoRaProfile {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::SetInterfaceLoRaProfile.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(
        id: InterfaceId,
        profile: RemoteControlLoRaProfile,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::SetInterfaceLoRaProfile { id, profile }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlLoRaOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::SetInterfaceLoRaProfile(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::SetInterfaceLoRaProfile,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlInventoryInterfacePeers;

impl RemoteControlInventoryInterfacePeers {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::InventoryInterfacePeers.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(
        id: InterfaceId,
        page: RemoteControlPeerPage,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::InventoryInterfacePeers { id, page }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlInterfacePeersOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::InventoryInterfacePeers(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::InventoryInterfacePeers,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlInventoryInterfaceConfig;

impl RemoteControlInventoryInterfaceConfig {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::InventoryInterfaceConfig.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(id: InterfaceId, out: &mut [u8]) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::InventoryInterfaceConfig { id }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlInterfaceConfigOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::InventoryInterfaceConfig(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::InventoryInterfaceConfig,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlDescribeBuild;

impl RemoteControlDescribeBuild {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::DescribeBuild;
    pub const RESPONSE_CAPACITY: usize = Self::REQUEST.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlBuildVersion, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::DescribeBuild(version) => Ok(version),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::DescribeBuild,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlDescribePower;

impl RemoteControlDescribePower {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::DescribePower;
    pub const RESPONSE_CAPACITY: usize = Self::REQUEST.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<PowerSnapshot, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::DescribePower(snapshot) => Ok(snapshot),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::DescribePower,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlSleepRadios;

impl RemoteControlSleepRadios {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::SleepRadios;
    pub const RESPONSE_CAPACITY: usize = Self::REQUEST.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlSleepOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::SleepRadios(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::SleepRadios,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlWakeRadios;

impl RemoteControlWakeRadios {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::WakeRadios;
    pub const RESPONSE_CAPACITY: usize = Self::REQUEST.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlSleepOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::WakeRadios(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::WakeRadios,
                found: response.kind(),
            }),
        }
    }
}

macro_rules! remote_control_apply_exchange {
    ($type_name:ident, $variant:ident, $field:ident, $field_type:ty) => {
        pub struct $type_name;

        impl $type_name {
            pub const RESPONSE_CAPACITY: usize =
                RemoteControlRequestKind::$variant.maximum_response_encoded_len();
            pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
                ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

            pub fn write_request(
                $field: $field_type,
                out: &mut [u8],
            ) -> Result<usize, RemoteControlError> {
                RemoteControlRequest::$variant { $field }
                    .write_into(out)
                    .map_err(RemoteControlError::Encode)
            }

            pub fn parse_response(
                bytes: &[u8],
            ) -> Result<RemoteControlApplyOutcome, RemoteControlError> {
                match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
                    RemoteControlResponse::$variant(outcome) => Ok(outcome),
                    RemoteControlResponse::ProtocolError(error) => {
                        Err(RemoteControlError::Remote(error))
                    }
                    response => Err(RemoteControlError::UnexpectedResponse {
                        expected: RemoteControlResponseKind::$variant,
                        found: response.kind(),
                    }),
                }
            }
        }
    };
}

remote_control_apply_exchange!(
    RemoteControlSetSystemPower,
    SetSystemPower,
    power,
    RemoteControlSystemPower
);
remote_control_apply_exchange!(
    RemoteControlSetGnssPower,
    SetGnssPower,
    power,
    RemoteControlGnssPower
);
remote_control_apply_exchange!(
    RemoteControlSetDisplayVisibility,
    SetDisplayVisibility,
    visibility,
    RemoteControlDisplayVisibility
);
remote_control_apply_exchange!(
    RemoteControlSetDisplayAutoOff,
    SetDisplayAutoOff,
    auto_off,
    RemoteControlDisplayAutoOff
);
remote_control_apply_exchange!(
    RemoteControlSetEspRadioMode,
    SetEspRadioMode,
    mode,
    RemoteControlEspRadioMode
);
remote_control_apply_exchange!(
    RemoteControlActivateWifiCredentials,
    ActivateWifiCredentials,
    revision,
    RemoteControlWifiCredentialRevision
);
remote_control_apply_exchange!(
    RemoteControlConfirmWifiCredentials,
    ConfirmWifiCredentials,
    revision,
    RemoteControlWifiCredentialRevision
);
remote_control_apply_exchange!(
    RemoteControlCancelWifiCredentials,
    CancelWifiCredentials,
    revision,
    RemoteControlWifiCredentialRevision
);

pub struct RemoteControlSetStationUplink;

impl RemoteControlSetStationUplink {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::SetStationUplink.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(
        id: InterfaceId,
        uplink: RemoteControlStationUplink,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::SetStationUplink { id, uplink }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(bytes: &[u8]) -> Result<RemoteControlApplyOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::SetStationUplink(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::SetStationUplink,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlStageWifiCredentials;

impl RemoteControlStageWifiCredentials {
    pub const RESPONSE_CAPACITY: usize =
        RemoteControlRequestKind::StageWifiCredentials.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(
        station: RemoteControlWifiStation,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::StageWifiCredentials { station }
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlWifiStageOutcome, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::StageWifiCredentials(outcome) => Ok(outcome),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::StageWifiCredentials,
                found: response.kind(),
            }),
        }
    }
}

pub struct RemoteControlInspectWifiTransaction;

impl RemoteControlInspectWifiTransaction {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::InspectWifiTransaction;
    pub const RESPONSE_CAPACITY: usize = Self::REQUEST.maximum_response_encoded_len();
    pub const MAXIMUM_RESPONSE_BYTES: ByteLimit =
        ByteLimit::Maximum(Self::RESPONSE_CAPACITY as u64);

    pub fn write_request(out: &mut [u8]) -> Result<usize, RemoteControlError> {
        Self::REQUEST
            .write_into(out)
            .map_err(RemoteControlError::Encode)
    }

    pub fn parse_response(
        bytes: &[u8],
    ) -> Result<RemoteControlWifiTransactionStatus, RemoteControlError> {
        match RemoteControlResponse::parse(bytes).map_err(RemoteControlError::Response)? {
            RemoteControlResponse::InspectWifiTransaction(status) => Ok(status),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
            response => Err(RemoteControlError::UnexpectedResponse {
                expected: RemoteControlResponseKind::InspectWifiTransaction,
                found: response.kind(),
            }),
        }
    }
}

fn require_available(
    available_requests: RemoteControlRequestSet,
    kind: RemoteControlRequestKind,
) -> Result<(), RemoteControlAdmitError> {
    if available_requests.supports(kind) {
        Ok(())
    } else {
        Err(RemoteControlAdmitError::KindNotPermitted)
    }
}

/// Why an inbound Remote Control request produced no reply.
///
/// Every variant maps to [`Decline::Ignore`]. The Controller then times out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlAdmitError {
    UnidentifiedRequester,
    RequestTooLarge,
    NoGrant,
    KindNotPermitted,
    AnnounceUnavailable,
}

impl From<RemoteControlAdmitError> for Decline {
    fn from(_error: RemoteControlAdmitError) -> Self {
        Self::Ignore
    }
}

struct RemoteControlRequestEndpoint;

impl RemoteControlRequestEndpoint {
    fn resolve(
        request: Result<RemoteControlRequest, RemoteControlRequestParseError>,
        available_requests: RemoteControlRequestSet,
        self_announcement: RemoteControlSelfAnnouncement,
    ) -> Result<AdmittedRemoteControlOperation, RemoteControlAdmitError> {
        match request {
            Ok(RemoteControlRequest::Describe) => {
                require_available(available_requests, RemoteControlRequestKind::Describe)?;
                let description = RemoteControlDescription::try_from(available_requests).map_err(
                    |RemoteControlDescriptionError::DescribeUnavailable| {
                        RemoteControlAdmitError::KindNotPermitted
                    },
                )?;
                Ok(AdmittedRemoteControlOperation::Describe(description))
            }
            Ok(RemoteControlRequest::AnnounceSelf) => {
                require_available(available_requests, RemoteControlRequestKind::AnnounceSelf)?;
                let RemoteControlSelfAnnouncement::Destination(destination) = self_announcement
                else {
                    return Err(RemoteControlAdmitError::AnnounceUnavailable);
                };
                Ok(AdmittedRemoteControlOperation::AnnounceSelf { destination })
            }
            Ok(RemoteControlRequest::InventoryInterfaces { page }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InventoryInterfaces,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::InventoryInterfaces { page },
                ))
            }
            Ok(RemoteControlRequest::InventoryPathTable { page }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InventoryPathTable,
                )?;
                Ok(AdmittedRemoteControlOperation::InventoryPathTable { page })
            }
            Ok(RemoteControlRequest::SetInterfacePower { id, power }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetInterfacePower,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetInterfacePower { id, power },
                ))
            }
            Ok(RemoteControlRequest::SetInterfaceMode { id, mode }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetInterfaceMode,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetInterfaceMode { id, mode },
                ))
            }
            Ok(RemoteControlRequest::SetInterfaceGroup { id, group }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetInterfaceGroup,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetInterfaceGroup {
                        id,
                        group: group.into_discovery_group(),
                    },
                ))
            }
            Ok(RemoteControlRequest::InventoryInterfaceDiscoveryGroups { id }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::InventoryInterfaceDiscoveryGroups { id },
                ))
            }
            Ok(RemoteControlRequest::ReplaceInterfaceDiscoveryGroups { id, groups }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::ReplaceInterfaceDiscoveryGroups { id, groups },
                ))
            }
            Ok(RemoteControlRequest::InventoryInterfacePeers { id, page }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InventoryInterfacePeers,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::InventoryInterfacePeers { id, page },
                ))
            }
            Ok(RemoteControlRequest::InventoryInterfaceConfig { id }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InventoryInterfaceConfig,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::InventoryInterfaceConfig { id },
                ))
            }
            Ok(RemoteControlRequest::SetInterfaceLoRaProfile { id, profile }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetInterfaceLoRaProfile,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetInterfaceLoRaProfile { id, profile },
                ))
            }
            #[cfg(feature = "remote-control-wifi-host")]
            Ok(RemoteControlRequest::SetInterfaceWifiStation { id, station }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetInterfaceWifiStation,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetInterfaceWifiStation { id, station },
                ))
            }
            Ok(RemoteControlRequest::InventoryControllers { page }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InventoryControllers,
                )?;
                Ok(AdmittedRemoteControlOperation::InventoryControllers { page })
            }
            Ok(RemoteControlRequest::AuthorizeController {
                controller,
                permitted_requests,
            }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::AuthorizeController,
                )?;
                Ok(AdmittedRemoteControlOperation::AuthorizeController {
                    controller,
                    permitted_requests,
                })
            }
            Ok(RemoteControlRequest::RevokeController { hash }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::RevokeController,
                )?;
                Ok(AdmittedRemoteControlOperation::RevokeController { hash })
            }
            Ok(RemoteControlRequest::DescribeBuild) => {
                require_available(available_requests, RemoteControlRequestKind::DescribeBuild)?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::DescribeBuild,
                ))
            }
            Ok(RemoteControlRequest::DescribePower) => {
                require_available(available_requests, RemoteControlRequestKind::DescribePower)?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::DescribePower,
                ))
            }
            Ok(RemoteControlRequest::SleepRadios) => {
                require_available(available_requests, RemoteControlRequestKind::SleepRadios)?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SleepRadios,
                ))
            }
            Ok(RemoteControlRequest::WakeRadios) => {
                require_available(available_requests, RemoteControlRequestKind::WakeRadios)?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::WakeRadios,
                ))
            }
            Ok(RemoteControlRequest::SetSystemPower { power }) => {
                require_available(available_requests, RemoteControlRequestKind::SetSystemPower)?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetSystemPower { power },
                ))
            }
            Ok(RemoteControlRequest::SetGnssPower { power }) => {
                require_available(available_requests, RemoteControlRequestKind::SetGnssPower)?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetGnssPower { power },
                ))
            }
            Ok(RemoteControlRequest::SetDisplayVisibility { visibility }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetDisplayVisibility,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetDisplayVisibility { visibility },
                ))
            }
            Ok(RemoteControlRequest::SetDisplayAutoOff { auto_off }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetDisplayAutoOff,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetDisplayAutoOff { auto_off },
                ))
            }
            #[cfg(feature = "remote-control-wifi-host")]
            Ok(RemoteControlRequest::SetStationUplink { id, uplink }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetStationUplink,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetStationUplink { id, uplink },
                ))
            }
            Ok(RemoteControlRequest::SetEspRadioMode { mode }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetEspRadioMode,
                )?;
                Ok(AdmittedRemoteControlOperation::Host(
                    RemoteControlHostCommand::SetEspRadioMode { mode },
                ))
            }
            #[cfg(feature = "remote-control-wifi-host")]
            Ok(RemoteControlRequest::StageWifiCredentials { station }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::StageWifiCredentials,
                )?;
                Ok(AdmittedRemoteControlOperation::StageWifiCredentials { station })
            }
            #[cfg(feature = "remote-control-wifi-host")]
            Ok(RemoteControlRequest::ActivateWifiCredentials { revision }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::ActivateWifiCredentials,
                )?;
                Ok(AdmittedRemoteControlOperation::ActivateWifiCredentials { revision })
            }
            #[cfg(feature = "remote-control-wifi-host")]
            Ok(RemoteControlRequest::ConfirmWifiCredentials { revision }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::ConfirmWifiCredentials,
                )?;
                Ok(AdmittedRemoteControlOperation::ConfirmWifiCredentials { revision })
            }
            #[cfg(feature = "remote-control-wifi-host")]
            Ok(RemoteControlRequest::CancelWifiCredentials { revision }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::CancelWifiCredentials,
                )?;
                Ok(AdmittedRemoteControlOperation::CancelWifiCredentials { revision })
            }
            #[cfg(feature = "remote-control-wifi-host")]
            Ok(RemoteControlRequest::InspectWifiTransaction) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InspectWifiTransaction,
                )?;
                Ok(AdmittedRemoteControlOperation::InspectWifiTransaction)
            }
            #[cfg(not(feature = "remote-control-wifi-host"))]
            Ok(
                RemoteControlRequest::SetInterfaceWifiStation { .. }
                | RemoteControlRequest::SetStationUplink { .. }
                | RemoteControlRequest::StageWifiCredentials { .. }
                | RemoteControlRequest::ActivateWifiCredentials { .. }
                | RemoteControlRequest::ConfirmWifiCredentials { .. }
                | RemoteControlRequest::CancelWifiCredentials { .. }
                | RemoteControlRequest::InspectWifiTransaction,
            ) => Err(RemoteControlAdmitError::KindNotPermitted),
            Err(error) => Ok(AdmittedRemoteControlOperation::ProtocolError(
                RemoteControlProtocolError::from(error),
            )),
        }
    }

    async fn handle_admitted<AppState>(
        mut context: RequestContext<'_, AppState>,
        node: &impl PrnsNodeApi,
        operation: AdmittedRemoteControlOperation,
    ) -> Result<(), Decline>
    where
        AppState: RemoteControlHostControls,
    {
        let response = match operation {
            AdmittedRemoteControlOperation::Host(command) => {
                execute_remote_control_host(context.state, command).await
            }
            AdmittedRemoteControlOperation::Describe(description) => {
                RemoteControlResponse::Describe(description)
            }
            AdmittedRemoteControlOperation::AnnounceSelf { destination } => {
                let outcome = match node
                    .announce_now(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    })
                    .await
                {
                    Ok(()) => RemoteControlAnnounceSelfOutcome::Announced,
                    Err(AnnounceNowError::NodeStopped | AnnounceNowError::Busy) => {
                        RemoteControlAnnounceSelfOutcome::Unavailable
                    }
                    Err(AnnounceNowError::Rejected(_)) => {
                        RemoteControlAnnounceSelfOutcome::Rejected
                    }
                    Err(AnnounceNowError::WriteFailed(_)) => {
                        RemoteControlAnnounceSelfOutcome::WriteFailed
                    }
                };
                RemoteControlResponse::AnnounceSelf(outcome)
            }
            AdmittedRemoteControlOperation::InventoryControllers { .. } => {
                RemoteControlResponse::InventoryControllers(
                    RemoteControlControllerInventory::empty(),
                )
            }
            AdmittedRemoteControlOperation::AuthorizeControllerGrant { grant } => {
                let outcome = match node.set_remote_control_controller_grant(grant).await {
                    Ok(
                        SetRemoteControlControllerGrantOutcome::Added
                        | SetRemoteControlControllerGrantOutcome::Unchanged
                        | SetRemoteControlControllerGrantOutcome::Updated { .. },
                    ) => RemoteControlAuthorizeControllerOutcome::Applied,
                    Err(SetRemoteControlControllerGrantControlError::CapacityExhausted) => {
                        RemoteControlAuthorizeControllerOutcome::CapacityExhausted
                    }
                    Err(SetRemoteControlControllerGrantControlError::Busy) => {
                        RemoteControlAuthorizeControllerOutcome::Busy
                    }
                    Err(
                        SetRemoteControlControllerGrantControlError::NodeStopped
                        | SetRemoteControlControllerGrantControlError::Unavailable,
                    ) => RemoteControlAuthorizeControllerOutcome::Failed,
                };
                RemoteControlResponse::AuthorizeController(outcome)
            }
            AdmittedRemoteControlOperation::RevokeControllerGrant { controller } => {
                let outcome = match node.revoke_remote_control_controller(controller).await {
                    Ok(RevokeRemoteControlControllerOutcome::Revoked { .. }) => {
                        RemoteControlRevokeControllerOutcome::Applied
                    }
                    Ok(RevokeRemoteControlControllerOutcome::NotFound) => {
                        RemoteControlRevokeControllerOutcome::NotFound
                    }
                    Err(RevokeRemoteControlControllerControlError::Busy) => {
                        RemoteControlRevokeControllerOutcome::Busy
                    }
                    Err(
                        RevokeRemoteControlControllerControlError::NodeStopped
                        | RevokeRemoteControlControllerControlError::Unavailable,
                    ) => RemoteControlRevokeControllerOutcome::Failed,
                };
                RemoteControlResponse::RevokeController(outcome)
            }
            AdmittedRemoteControlOperation::AuthorizeController { .. } => {
                RemoteControlResponse::AuthorizeController(
                    RemoteControlAuthorizeControllerOutcome::Failed,
                )
            }
            AdmittedRemoteControlOperation::RevokeController { .. } => {
                RemoteControlResponse::RevokeController(
                    RemoteControlRevokeControllerOutcome::Failed,
                )
            }
            AdmittedRemoteControlOperation::InventoryControllersReady(inventory) => {
                RemoteControlResponse::InventoryControllers(inventory)
            }
            AdmittedRemoteControlOperation::AuthorizeControllerReady(outcome) => {
                RemoteControlResponse::AuthorizeController(outcome)
            }
            AdmittedRemoteControlOperation::RevokeControllerReady(outcome) => {
                RemoteControlResponse::RevokeController(outcome)
            }
            AdmittedRemoteControlOperation::ProtocolError(error) => {
                RemoteControlResponse::ProtocolError(error)
            }
            AdmittedRemoteControlOperation::InventoryPathTable { page } => {
                RemoteControlResponse::InventoryPathTable(node.inventory_path_table(page).await)
            }
            #[cfg(feature = "remote-control-wifi-host")]
            _ => return Err(Decline::Ignore),
        };
        let mut out = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
        let encoded_len = response
            .write_into(&mut out)
            .map_err(|_| Decline::ResponseTooLarge)?;
        let encoded = out.get(..encoded_len).ok_or(Decline::ResponseTooLarge)?;
        context.respond(encoded)
    }
}

impl<AppState> RequestEndpoint<AppState> for RemoteControlRequestEndpoint
where
    AppState: RemoteControlHostControls,
{
    const ENDPOINT_ID: &'static str = REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::RequireIdentified;

    async fn handle(
        context: RequestContext<'_, AppState>,
        node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        let operation = Self::resolve(
            RemoteControlRequest::parse(context.data),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            RemoteControlSelfAnnouncement::Unavailable,
        )?;
        Self::handle_admitted(context, node, operation).await
    }
}

enum AdmittedRemoteControlOperation {
    Host(RemoteControlHostCommand),
    Describe(RemoteControlDescription),
    AnnounceSelf {
        destination: DestinationHash,
    },
    InventoryControllers {
        page: RemoteControlControllerPage,
    },
    AuthorizeController {
        controller: RemoteControlControllerIdentity,
        permitted_requests: RemoteControlRequestSet,
    },
    RevokeController {
        hash: IdentityHash,
    },
    AuthorizeControllerGrant {
        grant: RemoteControlControllerGrant,
    },
    RevokeControllerGrant {
        controller: RemoteControlControllerIdentity,
    },
    InventoryControllersReady(RemoteControlControllerInventory),
    AuthorizeControllerReady(RemoteControlAuthorizeControllerOutcome),
    RevokeControllerReady(RemoteControlRevokeControllerOutcome),
    #[cfg(feature = "remote-control-wifi-host")]
    StageWifiCredentials {
        station: RemoteControlWifiStation,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    ActivateWifiCredentials {
        revision: RemoteControlWifiCredentialRevision,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    ConfirmWifiCredentials {
        revision: RemoteControlWifiCredentialRevision,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    CancelWifiCredentials {
        revision: RemoteControlWifiCredentialRevision,
    },
    #[cfg(feature = "remote-control-wifi-host")]
    InspectWifiTransaction,
    ProtocolError(RemoteControlProtocolError),
    InventoryPathTable {
        page: RemoteControlPathPage,
    },
}

impl AdmittedRemoteControlOperation {
    fn prepare_host(self, controller: IdentityHash) -> Self {
        #[cfg(not(feature = "remote-control-wifi-host"))]
        {
            let _ = controller;
            self
        }
        #[cfg(feature = "remote-control-wifi-host")]
        {
            let command = match self {
                Self::StageWifiCredentials { station } => {
                    RemoteControlHostCommand::StageWifiCredentials {
                        controller,
                        station,
                    }
                }
                Self::ActivateWifiCredentials { revision } => {
                    RemoteControlHostCommand::ActivateWifiCredentials {
                        controller,
                        revision,
                    }
                }
                Self::ConfirmWifiCredentials { revision } => {
                    RemoteControlHostCommand::ConfirmWifiCredentials {
                        controller,
                        revision,
                    }
                }
                Self::CancelWifiCredentials { revision } => {
                    RemoteControlHostCommand::CancelWifiCredentials {
                        controller,
                        revision,
                    }
                }
                Self::InspectWifiTransaction => {
                    RemoteControlHostCommand::InspectWifiTransaction { controller }
                }
                operation => return operation,
            };
            Self::Host(command)
        }
    }
}

struct RemoteControlRequestBinding {
    destination: DestinationHash,
    controller: IdentityHash,
    responder: RespondToken,
    requested_at: InstantMillis,
    data_hash: [u8; SHA256_OUTPUT_LEN],
    data_len: usize,
}

impl RemoteControlRequestBinding {
    fn new(request: &InboundRequest<'_>) -> Result<Self, RemoteControlAdmitError> {
        let Some(controller) = request.requester else {
            return Err(RemoteControlAdmitError::UnidentifiedRequester);
        };
        if request.data.len() > RemoteControlRequest::MAX_ENCODED_LEN {
            return Err(RemoteControlAdmitError::RequestTooLarge);
        }
        Ok(Self {
            destination: request.destination,
            controller,
            responder: request.respond_token(),
            requested_at: request.requested_at,
            data_hash: sha256(request.data),
            data_len: request.data.len(),
        })
    }

    fn matches(&self, request: &InboundRequest<'_>) -> bool {
        self.destination == request.destination
            && request.requester == Some(self.controller)
            && self.responder == request.respond_token()
            && self.requested_at == request.requested_at
            && self.data_len == request.data.len()
            && self.data_hash == sha256(request.data)
    }
}

pub struct AdmittedRemoteControlRequest {
    binding: RemoteControlRequestBinding,
    operation: AdmittedRemoteControlOperation,
}

pub struct VerifiedAdmittedRemoteControlRequest {
    operation: AdmittedRemoteControlOperation,
}

impl VerifiedAdmittedRemoteControlRequest {
    pub fn authorize_controller_grant(&self) -> Option<RemoteControlControllerGrant> {
        match self.operation {
            AdmittedRemoteControlOperation::AuthorizeControllerGrant { grant } => Some(grant),
            _ => None,
        }
    }

    pub fn revoke_controller_grant(&self) -> Option<RemoteControlControllerIdentity> {
        match self.operation {
            AdmittedRemoteControlOperation::RevokeControllerGrant { controller } => {
                Some(controller)
            }
            _ => None,
        }
    }
}

pub fn verify_admitted_remote_control_request(
    admission: AdmittedRemoteControlRequest,
    request: &InboundRequest<'_>,
) -> Result<VerifiedAdmittedRemoteControlRequest, Decline> {
    if !admission.binding.matches(request) {
        return Err(Decline::Ignore);
    }
    Ok(VerifiedAdmittedRemoteControlRequest {
        operation: admission.operation,
    })
}

pub fn admit_remote_control_request<ControllerGrants>(
    controller_grants: &ControllerGrants,
    supported_requests: RemoteControlRequestSet,
    self_announcement: RemoteControlSelfAnnouncement,
    request: &InboundRequest<'_>,
) -> Result<AdmittedRemoteControlRequest, RemoteControlAdmitError>
where
    ControllerGrants: RemoteControlControllerGrantTable,
{
    let binding = RemoteControlRequestBinding::new(request)?;
    let operation = resolve_admitted_remote_control_operation(
        controller_grants,
        supported_requests,
        self_announcement,
        binding.controller,
        request.data,
    )?;
    Ok(AdmittedRemoteControlRequest { binding, operation })
}

pub fn admit_verified_remote_control_request<ControllerGrants>(
    controller_grants: &ControllerGrants,
    supported_requests: RemoteControlRequestSet,
    self_announcement: RemoteControlSelfAnnouncement,
    request: &InboundRequest<'_>,
) -> Result<VerifiedAdmittedRemoteControlRequest, RemoteControlAdmitError>
where
    ControllerGrants: RemoteControlControllerGrantTable,
{
    let Some(controller) = request.requester else {
        return Err(RemoteControlAdmitError::UnidentifiedRequester);
    };
    if request.data.len() > RemoteControlRequest::MAX_ENCODED_LEN {
        return Err(RemoteControlAdmitError::RequestTooLarge);
    }
    let operation = resolve_admitted_remote_control_operation(
        controller_grants,
        supported_requests,
        self_announcement,
        controller,
        request.data,
    )?;
    Ok(VerifiedAdmittedRemoteControlRequest { operation })
}

fn resolve_admitted_remote_control_operation<ControllerGrants>(
    controller_grants: &ControllerGrants,
    supported_requests: RemoteControlRequestSet,
    self_announcement: RemoteControlSelfAnnouncement,
    controller: IdentityHash,
    request: &[u8],
) -> Result<AdmittedRemoteControlOperation, RemoteControlAdmitError>
where
    ControllerGrants: RemoteControlControllerGrantTable,
{
    let Some(grant) = controller_grants.grant_for(&controller).copied() else {
        return Err(RemoteControlAdmitError::NoGrant);
    };
    let available_requests = supported_requests.intersection(&grant.effective_requests());
    let operation = RemoteControlRequestEndpoint::resolve(
        RemoteControlRequest::parse(request),
        available_requests,
        self_announcement,
    )?;
    let operation = prepare_controller_grant_operation(controller_grants, controller, operation);
    Ok(operation.prepare_host(controller))
}

fn prepare_controller_grant_operation<ControllerGrants>(
    controller_grants: &ControllerGrants,
    requester: IdentityHash,
    operation: AdmittedRemoteControlOperation,
) -> AdmittedRemoteControlOperation
where
    ControllerGrants: RemoteControlControllerGrantTable,
{
    match operation {
        AdmittedRemoteControlOperation::InventoryControllers { page } => {
            match RemoteControlControllerInventory::from_grants(controller_grants, page) {
                Ok(inventory) => {
                    AdmittedRemoteControlOperation::InventoryControllersReady(inventory)
                }
                Err(_) => AdmittedRemoteControlOperation::ProtocolError(
                    RemoteControlProtocolError::InternalFailure {
                        request: RemoteControlRequestKind::InventoryControllers,
                    },
                ),
            }
        }
        AdmittedRemoteControlOperation::AuthorizeController {
            controller,
            permitted_requests,
        } => {
            let target = controller.identity_hash();
            if target == requester
                || controller_grants.grant_for(&target).is_some_and(|grant| {
                    grant.authority() == RemoteControlControllerAuthority::Administrator
                })
            {
                return AdmittedRemoteControlOperation::AuthorizeControllerReady(
                    RemoteControlAuthorizeControllerOutcome::Forbidden,
                );
            }
            match RemoteControlControllerGrant::new(
                controller,
                RemoteControlControllerAuthority::Operator,
                permitted_requests,
            ) {
                Ok(grant) => AdmittedRemoteControlOperation::AuthorizeControllerGrant { grant },
                Err(_) => AdmittedRemoteControlOperation::AuthorizeControllerReady(
                    RemoteControlAuthorizeControllerOutcome::Failed,
                ),
            }
        }
        AdmittedRemoteControlOperation::RevokeController { hash } => {
            if hash == requester {
                return AdmittedRemoteControlOperation::RevokeControllerReady(
                    RemoteControlRevokeControllerOutcome::Forbidden,
                );
            }
            match controller_grants.grant_for(&hash).copied() {
                None => AdmittedRemoteControlOperation::RevokeControllerReady(
                    RemoteControlRevokeControllerOutcome::NotFound,
                ),
                Some(grant)
                    if grant.authority() == RemoteControlControllerAuthority::Administrator =>
                {
                    AdmittedRemoteControlOperation::RevokeControllerReady(
                        RemoteControlRevokeControllerOutcome::Forbidden,
                    )
                }
                Some(grant) => AdmittedRemoteControlOperation::RevokeControllerGrant {
                    controller: *grant.controller(),
                },
            }
        }
        operation => operation,
    }
}

pub async fn dispatch_admitted_remote_control_request<'a, AppState>(
    state: &'a AppState,
    node: &impl PrnsNodeApi,
    request: InboundRequest<'a>,
    sink: &'a mut dyn ResponseSink,
    admission: AdmittedRemoteControlRequest,
) -> Result<(), Decline>
where
    AppState: RemoteControlHostControls,
{
    let verified = verify_admitted_remote_control_request(admission, &request)?;
    dispatch_verified_admitted_remote_control_request(state, node, request, sink, verified).await
}

pub fn dispatch_verified_admitted_remote_control_request<'a, AppState>(
    state: &'a AppState,
    node: &'a impl PrnsNodeApi,
    request: InboundRequest<'a>,
    sink: &'a mut dyn ResponseSink,
    verified: VerifiedAdmittedRemoteControlRequest,
) -> impl core::future::Future<Output = Result<(), Decline>> + 'a
where
    AppState: RemoteControlHostControls,
{
    RemoteControlRequestEndpoint::handle_admitted(
        RequestContext::from_inbound(state, request, sink),
        node,
        verified.operation,
    )
}

pub async fn dispatch_remote_control_request<'a, AppState, ControllerGrants>(
    state: &'a AppState,
    controller_grants: &mut ControllerGrants,
    supported_requests: RemoteControlRequestSet,
    self_announcement: RemoteControlSelfAnnouncement,
    node: &impl PrnsNodeApi,
    request: InboundRequest<'a>,
    sink: &'a mut dyn ResponseSink,
) -> Result<(), Decline>
where
    AppState: RemoteControlHostControls,
    ControllerGrants: RemoteControlControllerGrantTable,
{
    let admission = admit_remote_control_request(
        controller_grants,
        supported_requests,
        self_announcement,
        &request,
    )?;
    dispatch_admitted_remote_control_request(state, node, request, sink, admission).await
}

#[cfg(test)]
mod tests {
    use super::super::RemoteControlControllerGrantControl;
    use super::*;
    use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
    use crate::identity::{
        IdentityEncryptionPublicKey, IdentityHash, IdentityPublicKeys, IdentitySigningPublicKey,
    };
    use crate::remote_control::{
        FixedRemoteControlControllerGrantTable, RemoteControlControllerGrant,
        RemoteControlControllerIdentity, RemoteControlProtocolVersion, RemoteControlRequestKind,
        RemoteControlRequestSet,
    };
    use crate::routing::links::request::RequestId;
    use crate::routing::links::LinkId;
    use crate::runtime::request_endpoints::InboundRequest;
    use crate::units::{InstantMillis, RttMillis};
    use crate::wire::DestinationHash;
    use std::sync::Mutex;

    struct AnnounceNode {
        result: Result<(), AnnounceNowError>,
        received: Mutex<Option<AnnounceNow>>,
        controller_grant_result: Result<
            SetRemoteControlControllerGrantOutcome,
            SetRemoteControlControllerGrantControlError,
        >,
        revoked_controller_result:
            Result<RevokeRemoteControlControllerOutcome, RevokeRemoteControlControllerControlError>,
        received_controller_grant: Mutex<Option<RemoteControlControllerGrant>>,
        received_revocation: Mutex<Option<RemoteControlControllerIdentity>>,
    }

    impl AnnounceNode {
        fn new(result: Result<(), AnnounceNowError>) -> Self {
            Self {
                result,
                received: Mutex::new(None),
                controller_grant_result: Err(
                    SetRemoteControlControllerGrantControlError::NodeStopped,
                ),
                revoked_controller_result: Err(
                    RevokeRemoteControlControllerControlError::NodeStopped,
                ),
                received_controller_grant: Mutex::new(None),
                received_revocation: Mutex::new(None),
            }
        }

        fn authorization(revoked: RemoteControlControllerGrant) -> Self {
            Self {
                result: Err(AnnounceNowError::NodeStopped),
                received: Mutex::new(None),
                controller_grant_result: Ok(SetRemoteControlControllerGrantOutcome::Updated {
                    previous: revoked,
                }),
                revoked_controller_result: Ok(RevokeRemoteControlControllerOutcome::Revoked {
                    grant: revoked,
                }),
                received_controller_grant: Mutex::new(None),
                received_revocation: Mutex::new(None),
            }
        }
    }

    impl RemoteControlControllerGrantControl for AnnounceNode {
        async fn set_remote_control_controller_grant(
            &self,
            grant: RemoteControlControllerGrant,
        ) -> Result<
            SetRemoteControlControllerGrantOutcome,
            SetRemoteControlControllerGrantControlError,
        > {
            *self.received_controller_grant.lock().unwrap() = Some(grant);
            self.controller_grant_result
        }

        async fn revoke_remote_control_controller(
            &self,
            controller: RemoteControlControllerIdentity,
        ) -> Result<RevokeRemoteControlControllerOutcome, RevokeRemoteControlControllerControlError>
        {
            *self.received_revocation.lock().unwrap() = Some(controller);
            self.revoked_controller_result
        }
    }

    impl PrnsNodeApi for AnnounceNode {
        fn issue(&self, command: crate::engine::PrnsCommand) -> Option<crate::engine::CommandId> {
            <() as PrnsNodeApi>::issue(&(), command)
        }

        async fn announce_now(&self, announce: AnnounceNow) -> Result<(), AnnounceNowError> {
            *self.received.lock().unwrap() = Some(announce);
            self.result
        }

        async fn set_registered_announce_app_data(
            &self,
            set: crate::engine::SetRegisteredAnnounceAppData,
        ) -> Result<(), super::super::SetRegisteredAnnounceAppDataError> {
            <() as PrnsNodeApi>::set_registered_announce_app_data(&(), set).await
        }

        async fn open_remote_control_pairing(
            &self,
            open: crate::engine::OpenRemoteControlPairing,
        ) -> Result<
            crate::engine::RemoteControlPairingOpened,
            super::super::OpenRemoteControlPairingControlError,
        > {
            <() as PrnsNodeApi>::open_remote_control_pairing(&(), open).await
        }

        async fn close_remote_control_pairing(
            &self,
        ) -> Result<
            crate::engine::CloseRemoteControlPairingOutcome,
            super::super::CloseRemoteControlPairingControlError,
        > {
            <() as PrnsNodeApi>::close_remote_control_pairing(&()).await
        }

        async fn send_single_packet(
            &self,
            destination: DestinationHash,
            data: &[u8],
        ) -> Result<
            crate::engine::PacketReceiptDelivered,
            SendError<crate::engine::SendSinglePacketFailure>,
        > {
            <() as PrnsNodeApi>::send_single_packet(&(), destination, data).await
        }

        async fn send_plain_packet(
            &self,
            destination: DestinationHash,
            data: &[u8],
        ) -> Result<(), SendError<crate::engine::SendPlainPacketFailure>> {
            <() as PrnsNodeApi>::send_plain_packet(&(), destination, data).await
        }

        async fn send_group_packet(
            &self,
            destination: DestinationHash,
            data: &[u8],
        ) -> Result<(), SendError<crate::engine::SendGroupFailure>> {
            <() as PrnsNodeApi>::send_group_packet(&(), destination, data).await
        }

        fn respond_packed(
            &self,
            responder: super::super::request_endpoints::RespondToken,
            packed: &[u8],
        ) -> bool {
            <() as PrnsNodeApi>::respond_packed(&(), responder, packed)
        }

        fn close_link(&self, link_id: LinkId) -> bool {
            <() as PrnsNodeApi>::close_link(&(), link_id)
        }
    }

    fn identity(fill: u8) -> RemoteControlControllerIdentity {
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
        })
    }

    fn controller_grants(
        allowed: RemoteControlControllerIdentity,
    ) -> FixedRemoteControlControllerGrantTable<1> {
        controller_grants_permitting(allowed, RemoteControlRequestSet::all_operator())
    }

    fn controller_grants_permitting(
        allowed: RemoteControlControllerIdentity,
        permitted_requests: RemoteControlRequestSet,
    ) -> FixedRemoteControlControllerGrantTable<1> {
        let mut controller_grants = FixedRemoteControlControllerGrantTable::default();
        controller_grants
            .set_controller_grant(
                RemoteControlControllerGrant::new(
                    allowed,
                    RemoteControlControllerAuthority::Operator,
                    permitted_requests,
                )
                .unwrap(),
            )
            .unwrap();
        controller_grants
    }

    async fn dispatch(
        controller_grants: &mut impl RemoteControlControllerGrantTable,
        requester: Option<IdentityHash>,
        data: &[u8],
        sink: &mut dyn super::super::request_endpoints::ResponseSink,
    ) -> Result<(), Decline> {
        dispatch_with_node(controller_grants, &(), requester, data, sink).await
    }

    async fn dispatch_with_node(
        controller_grants: &mut impl RemoteControlControllerGrantTable,
        node: &impl PrnsNodeApi,
        requester: Option<IdentityHash>,
        data: &[u8],
        sink: &mut dyn super::super::request_endpoints::ResponseSink,
    ) -> Result<(), Decline> {
        dispatch_with_configuration(
            controller_grants,
            RemoteControlRequestSet::all(),
            RemoteControlSelfAnnouncement::Destination(DestinationHash::new([0x87; 16])),
            node,
            requester,
            data,
            sink,
        )
        .await
    }

    async fn dispatch_with_configuration(
        controller_grants: &mut impl RemoteControlControllerGrantTable,
        supported_requests: RemoteControlRequestSet,
        self_announcement: RemoteControlSelfAnnouncement,
        node: &impl PrnsNodeApi,
        requester: Option<IdentityHash>,
        data: &[u8],
        sink: &mut dyn super::super::request_endpoints::ResponseSink,
    ) -> Result<(), Decline> {
        let request = InboundRequest::new(
            DestinationHash::new([0x21; 16]),
            LinkId::new([0x43; 16]),
            RequestId([0x65; 16]),
            requester,
            InstantMillis(1_000),
            RttMillis::new(20),
            data,
        );
        dispatch_remote_control_request(
            &NoRemoteControlHostControls,
            controller_grants,
            supported_requests,
            self_announcement,
            node,
            request,
            sink,
        )
        .await
    }

    fn describe_request() -> [u8; RemoteControlRequest::Describe.encoded_len()] {
        let mut request = [0u8; RemoteControlRequest::Describe.encoded_len()];
        RemoteControlRequest::Describe
            .write_into(&mut request)
            .unwrap();
        request
    }

    fn announce_self_request() -> [u8; RemoteControlRequest::AnnounceSelf.encoded_len()] {
        let mut request = [0u8; RemoteControlRequest::AnnounceSelf.encoded_len()];
        RemoteControlRequest::AnnounceSelf
            .write_into(&mut request)
            .unwrap();
        request
    }

    #[cfg(not(feature = "remote-control-wifi-host"))]
    #[test]
    fn a_non_wifi_host_rejects_wifi_operations_even_when_the_capability_set_is_overstated() {
        assert!(matches!(
            RemoteControlRequestEndpoint::resolve(
                Ok(RemoteControlRequest::InspectWifiTransaction),
                RemoteControlRequestSet::all(),
                RemoteControlSelfAnnouncement::Unavailable,
            ),
            Err(RemoteControlAdmitError::KindNotPermitted),
        ));
    }

    #[derive(Clone, Copy)]
    struct InboundRequestFixture<'a> {
        destination: DestinationHash,
        link_id: LinkId,
        request_id: RequestId,
        requester: Option<IdentityHash>,
        requested_at: InstantMillis,
        rtt: RttMillis,
        data: &'a [u8],
    }

    impl<'a> InboundRequestFixture<'a> {
        fn inbound(self) -> InboundRequest<'a> {
            InboundRequest::new(
                self.destination,
                self.link_id,
                self.request_id,
                self.requester,
                self.requested_at,
                self.rtt,
                self.data,
            )
        }
    }

    #[test]
    fn describe_exchange_owns_its_wire_contract() {
        let mut request = [0u8; RemoteControlDescribe::REQUEST.encoded_len()];
        assert_eq!(
            RemoteControlDescribe::write_request(&mut request),
            Ok(request.len()),
        );
        assert_eq!(
            request,
            [
                RemoteControlProtocolVersion::V1.wire_value(),
                RemoteControlDescribe::REQUEST.kind().wire_value(),
            ],
        );
        assert_eq!(
            RemoteControlDescribe::MAXIMUM_RESPONSE_BYTES,
            ByteLimit::Maximum(RemoteControlResponse::MAX_ENCODED_LEN as u64),
        );

        let description =
            RemoteControlDescription::try_from(RemoteControlRequestSet::all()).unwrap();
        let response = RemoteControlResponse::Describe(description);
        let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
        let encoded_len = response.write_into(&mut encoded).unwrap();
        assert_eq!(
            RemoteControlDescribe::parse_response(&encoded[..encoded_len]),
            Ok(description),
        );

        let protocol_error = RemoteControlProtocolError::UnknownRequestKind { found: 0xA5 };
        let response = RemoteControlResponse::ProtocolError(protocol_error);
        let encoded_len = response.write_into(&mut encoded).unwrap();
        assert_eq!(
            RemoteControlDescribe::parse_response(&encoded[..encoded_len]),
            Err(RemoteControlError::Remote(protocol_error)),
        );
    }

    #[test]
    fn announce_self_exchange_owns_its_wire_contract() {
        let mut request = [0u8; RemoteControlAnnounceSelf::REQUEST.encoded_len()];
        assert_eq!(
            RemoteControlAnnounceSelf::write_request(&mut request),
            Ok(request.len()),
        );
        assert_eq!(
            request,
            [
                RemoteControlProtocolVersion::V1.wire_value(),
                RemoteControlAnnounceSelf::REQUEST.kind().wire_value(),
            ],
        );
        assert_eq!(
            RemoteControlAnnounceSelf::MAXIMUM_RESPONSE_BYTES,
            ByteLimit::Maximum(
                RemoteControlAnnounceSelf::REQUEST.maximum_response_encoded_len() as u64,
            ),
        );

        let cases = [
            (RemoteControlAnnounceSelfOutcome::Announced, Ok(())),
            (
                RemoteControlAnnounceSelfOutcome::Unavailable,
                Err(RemoteControlError::AnnounceSelf(
                    RemoteControlAnnounceSelfFailure::Unavailable,
                )),
            ),
            (
                RemoteControlAnnounceSelfOutcome::Rejected,
                Err(RemoteControlError::AnnounceSelf(
                    RemoteControlAnnounceSelfFailure::Rejected,
                )),
            ),
            (
                RemoteControlAnnounceSelfOutcome::WriteFailed,
                Err(RemoteControlError::AnnounceSelf(
                    RemoteControlAnnounceSelfFailure::WriteFailed,
                )),
            ),
        ];
        for (outcome, expected) in cases {
            let response = RemoteControlResponse::AnnounceSelf(outcome);
            let mut encoded = [0u8; RemoteControlAnnounceSelf::RESPONSE_CAPACITY];
            let encoded_len = response.write_into(&mut encoded).unwrap();
            assert_eq!(
                RemoteControlAnnounceSelf::parse_response(&encoded[..encoded_len]),
                expected,
            );
        }
    }

    #[test]
    fn an_admitted_announce_self_waits_for_the_exact_destination_effect() {
        futures_executor::block_on(async {
            let allowed = identity(0x31);
            let mut controller_grants = controller_grants(allowed);
            let node = AnnounceNode::new(Ok(()));
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();

            assert_eq!(
                dispatch_with_node(
                    &mut controller_grants,
                    &node,
                    Some(allowed.identity_hash()),
                    &announce_self_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            assert_eq!(
                node.received.lock().unwrap().take(),
                Some(AnnounceNow {
                    destination: DestinationHash::new([0x87; 16]),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            );
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::AnnounceSelf(
                    RemoteControlAnnounceSelfOutcome::Announced,
                )),
            );
        });
    }

    #[test]
    fn unavailable_self_announcement_is_neither_described_nor_dispatched() {
        futures_executor::block_on(async {
            let allowed = identity(0x35);
            let mut controller_grants = controller_grants(allowed);
            let supported_requests =
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
            let node = AnnounceNode::new(Ok(()));
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();

            assert_eq!(
                dispatch_with_configuration(
                    &mut controller_grants,
                    supported_requests,
                    RemoteControlSelfAnnouncement::Unavailable,
                    &node,
                    Some(allowed.identity_hash()),
                    &describe_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            let description = RemoteControlDescription::try_from(supported_requests).unwrap();
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::Describe(description)),
            );

            response.clear();
            assert_eq!(
                dispatch_with_configuration(
                    &mut controller_grants,
                    supported_requests,
                    RemoteControlSelfAnnouncement::Unavailable,
                    &node,
                    Some(allowed.identity_hash()),
                    &announce_self_request(),
                    &mut response,
                )
                .await,
                Err(Decline::Ignore),
            );
            assert!(response.is_empty());
            assert!(node.received.lock().unwrap().is_none());
        });
    }

    #[test]
    fn a_controller_grant_reaches_only_its_permitted_requests() {
        futures_executor::block_on(async {
            let allowed = identity(0x33);
            let permitted_requests =
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
            let mut controller_grants = controller_grants_permitting(allowed, permitted_requests);
            let node = AnnounceNode::new(Ok(()));
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();

            assert_eq!(
                dispatch_with_node(
                    &mut controller_grants,
                    &node,
                    Some(allowed.identity_hash()),
                    &announce_self_request(),
                    &mut response,
                )
                .await,
                Err(Decline::Ignore),
            );
            assert!(response.is_empty());
            assert!(node.received.lock().unwrap().is_none());

            assert_eq!(
                dispatch_with_node(
                    &mut controller_grants,
                    &node,
                    Some(allowed.identity_hash()),
                    &describe_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            let description = RemoteControlDescription::try_from(permitted_requests).unwrap();
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::Describe(description)),
            );
        });
    }

    #[test]
    fn describe_reports_only_the_board_and_grant_capability_intersection() {
        futures_executor::block_on(async {
            let allowed = identity(0x36);
            let mut supported = RemoteControlRequestSet::empty();
            assert!(supported.insert(RemoteControlRequestKind::Describe));
            assert!(supported.insert(RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups,));
            let mut permitted = RemoteControlRequestSet::empty();
            assert!(permitted.insert(RemoteControlRequestKind::Describe));
            assert!(permitted.insert(RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups,));
            let mut controller_grants = controller_grants_permitting(allowed, permitted);
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();

            assert_eq!(
                dispatch_with_configuration(
                    &mut controller_grants,
                    supported,
                    RemoteControlSelfAnnouncement::Unavailable,
                    &(),
                    Some(allowed.identity_hash()),
                    &describe_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            let expected = RemoteControlDescription::try_from(RemoteControlRequestSet::only(
                RemoteControlRequestKind::Describe,
            ))
            .unwrap();
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::Describe(expected)),
            );
        });
    }

    #[test]
    fn an_admission_binding_requires_every_inbound_request_fact_to_match() {
        let allowed = identity(0x34);
        let admitted_data = [0x73, RemoteControlRequestKind::Describe.wire_value()];
        let same_error_different_data = [0x73, RemoteControlRequestKind::AnnounceSelf.wire_value()];
        let admitted = InboundRequestFixture {
            destination: DestinationHash::new([0x21; 16]),
            link_id: LinkId::new([0x43; 16]),
            request_id: RequestId([0x65; 16]),
            requester: Some(allowed.identity_hash()),
            requested_at: InstantMillis(1_000),
            rtt: RttMillis::new(20),
            data: &admitted_data,
        };
        let binding = RemoteControlRequestBinding::new(&admitted.inbound()).unwrap();
        let changed_requests = [
            InboundRequestFixture {
                destination: DestinationHash::new([0x22; 16]),
                ..admitted
            },
            InboundRequestFixture {
                link_id: LinkId::new([0x44; 16]),
                ..admitted
            },
            InboundRequestFixture {
                request_id: RequestId([0x66; 16]),
                ..admitted
            },
            InboundRequestFixture {
                requester: Some(identity(0x35).identity_hash()),
                ..admitted
            },
            InboundRequestFixture {
                requested_at: InstantMillis(1_001),
                ..admitted
            },
            InboundRequestFixture {
                rtt: RttMillis::new(21),
                ..admitted
            },
            InboundRequestFixture {
                data: &same_error_different_data,
                ..admitted
            },
        ];

        assert!(binding.matches(&admitted.inbound()));
        for changed in changed_requests {
            assert!(!binding.matches(&changed.inbound()));
        }
    }

    #[test]
    fn oversized_requests_cannot_enter_the_admitted_state() {
        let allowed = identity(0x36);
        let data = [0x73; RemoteControlRequest::MAX_ENCODED_LEN + 1];
        let request = InboundRequestFixture {
            destination: DestinationHash::new([0x21; 16]),
            link_id: LinkId::new([0x43; 16]),
            request_id: RequestId([0x65; 16]),
            requester: Some(allowed.identity_hash()),
            requested_at: InstantMillis(1_000),
            rtt: RttMillis::new(20),
            data: &data,
        };

        assert!(matches!(
            RemoteControlRequestBinding::new(&request.inbound()),
            Err(RemoteControlAdmitError::RequestTooLarge),
        ));
    }

    #[test]
    fn announce_self_effect_failures_are_stable_wire_outcomes() {
        futures_executor::block_on(async {
            let allowed = identity(0x32);
            let mut controller_grants = controller_grants(allowed);
            let cases = [
                (
                    AnnounceNowError::NodeStopped,
                    RemoteControlAnnounceSelfOutcome::Unavailable,
                ),
                (
                    AnnounceNowError::Busy,
                    RemoteControlAnnounceSelfOutcome::Unavailable,
                ),
                (
                    AnnounceNowError::Rejected(
                        crate::engine::AnnounceNowRejection::UnknownDestination,
                    ),
                    RemoteControlAnnounceSelfOutcome::Rejected,
                ),
                (
                    AnnounceNowError::WriteFailed(crate::engine::AnnounceWriteFailure::Rejected(
                        crate::engine::AnnounceRejection::NotRegistered,
                    )),
                    RemoteControlAnnounceSelfOutcome::WriteFailed,
                ),
            ];

            for (failure, expected) in cases {
                let node = AnnounceNode::new(Err(failure));
                let mut response =
                    heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();
                assert_eq!(
                    dispatch_with_node(
                        &mut controller_grants,
                        &node,
                        Some(allowed.identity_hash()),
                        &announce_self_request(),
                        &mut response,
                    )
                    .await,
                    Ok(()),
                );
                assert_eq!(
                    RemoteControlResponse::parse(response.as_slice()),
                    Ok(RemoteControlResponse::AnnounceSelf(expected)),
                );
            }
        });
    }

    #[test]
    fn the_endpoint_requires_an_identified_requester_before_access_is_checked() {
        assert_eq!(
            <RemoteControlRequestEndpoint as RequestEndpoint<NoRemoteControlHostControls>>::POLICY,
            RequestEndpointPolicy::RequireIdentified,
        );
    }

    #[test]
    fn an_admitted_identity_receives_only_its_available_requests() {
        futures_executor::block_on(async {
            let allowed = identity(0x21);
            let available_requests =
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
            let mut controller_grants = controller_grants_permitting(allowed, available_requests);
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();

            assert_eq!(
                dispatch(
                    &mut controller_grants,
                    Some(allowed.identity_hash()),
                    &describe_request(),
                    &mut response,
                )
                .await,
                Ok(()),
            );
            let description = RemoteControlDescription::try_from(available_requests).unwrap();
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::Describe(description)),
            );
        });
    }

    #[test]
    fn unidentified_and_unlisted_requesters_cannot_reach_remote_control() {
        futures_executor::block_on(async {
            let mut controller_grants = controller_grants(identity(0x43));
            let node = AnnounceNode::new(Ok(()));

            for requester in [None, Some(identity(0x65).identity_hash())] {
                let mut response =
                    heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();
                assert_eq!(
                    dispatch_with_node(
                        &mut controller_grants,
                        &node,
                        requester,
                        &announce_self_request(),
                        &mut response,
                    )
                    .await,
                    Err(Decline::Ignore),
                );
                assert!(response.is_empty());
                assert!(node.received.lock().unwrap().is_none());
            }
        });
    }

    #[test]
    fn admit_names_the_silent_ignore_reasons() {
        let allowed = identity(0x43);
        let grants = controller_grants_permitting(
            allowed,
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        );
        let announce = announce_self_request();
        let describe = describe_request();
        let inbound = |requester, data| InboundRequestFixture {
            destination: DestinationHash::new([0x21; 16]),
            link_id: LinkId::new([0x43; 16]),
            request_id: RequestId([0x65; 16]),
            requester,
            requested_at: InstantMillis(1_000),
            rtt: RttMillis::new(20),
            data,
        };

        assert_eq!(
            admit_remote_control_request(
                &grants,
                RemoteControlRequestSet::all(),
                RemoteControlSelfAnnouncement::Unavailable,
                &inbound(None, &announce).inbound(),
            )
            .err(),
            Some(RemoteControlAdmitError::UnidentifiedRequester),
        );
        assert_eq!(
            admit_remote_control_request(
                &grants,
                RemoteControlRequestSet::all(),
                RemoteControlSelfAnnouncement::Unavailable,
                &inbound(Some(identity(0x65).identity_hash()), &announce).inbound(),
            )
            .err(),
            Some(RemoteControlAdmitError::NoGrant),
        );
        assert_eq!(
            admit_remote_control_request(
                &grants,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
                RemoteControlSelfAnnouncement::Unavailable,
                &inbound(Some(allowed.identity_hash()), &announce).inbound(),
            )
            .err(),
            Some(RemoteControlAdmitError::KindNotPermitted),
        );
        let all_grants = controller_grants(allowed);
        assert_eq!(
            admit_remote_control_request(
                &all_grants,
                RemoteControlRequestSet::all(),
                RemoteControlSelfAnnouncement::Unavailable,
                &inbound(Some(allowed.identity_hash()), &announce).inbound(),
            )
            .err(),
            Some(RemoteControlAdmitError::AnnounceUnavailable),
        );
        assert!(admit_remote_control_request(
            &grants,
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            RemoteControlSelfAnnouncement::Unavailable,
            &inbound(Some(allowed.identity_hash()), &describe).inbound(),
        )
        .is_ok());
    }

    #[test]
    fn admitted_protocol_failures_receive_typed_errors() {
        futures_executor::block_on(async {
            let allowed = identity(0x87);
            let mut controller_grants = controller_grants(allowed);
            let unsupported_version = 0x73;
            let unknown_request_kind = 0x95;
            let cases = [
                (&[][..], RemoteControlProtocolError::MalformedRequest),
                (
                    &[
                        unsupported_version,
                        RemoteControlRequestKind::Describe.wire_value(),
                    ][..],
                    RemoteControlProtocolError::UnsupportedVersion {
                        found: unsupported_version,
                    },
                ),
                (
                    &[
                        RemoteControlProtocolVersion::V1.wire_value(),
                        unknown_request_kind,
                    ][..],
                    RemoteControlProtocolError::UnknownRequestKind {
                        found: unknown_request_kind,
                    },
                ),
            ];

            for (request, expected) in cases {
                let mut response =
                    heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();
                assert_eq!(
                    dispatch(
                        &mut controller_grants,
                        Some(allowed.identity_hash()),
                        request,
                        &mut response,
                    )
                    .await,
                    Ok(()),
                );
                assert_eq!(
                    RemoteControlResponse::parse(response.as_slice()),
                    Ok(RemoteControlResponse::ProtocolError(expected)),
                );
            }
        });
    }

    #[test]
    fn controller_management_is_prepared_during_admission_and_mutated_only_by_persistent_control() {
        futures_executor::block_on(async {
            let allowed = identity(0x31);
            let extra = identity(0x42);
            let mut grants = FixedRemoteControlControllerGrantTable::<2>::default();
            let administrator = RemoteControlControllerGrant::new(
                allowed,
                RemoteControlControllerAuthority::Administrator,
                RemoteControlRequestSet::all_operator(),
            )
            .unwrap();
            let prior_operator = RemoteControlControllerGrant::new(
                extra,
                RemoteControlControllerAuthority::Operator,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
            )
            .unwrap();
            grants.set_controller_grant(administrator).unwrap();
            grants.set_controller_grant(prior_operator).unwrap();
            let node = AnnounceNode::authorization(prior_operator);

            let mut request = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
            let encoded_len = RemoteControlInventoryControllers::write_request(&mut request)
                .expect("inventory request");
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();
            assert_eq!(
                dispatch_with_node(
                    &mut grants,
                    &node,
                    Some(allowed.identity_hash()),
                    &request[..encoded_len],
                    &mut response,
                )
                .await,
                Ok(()),
            );
            let RemoteControlResponse::InventoryControllers(inventory) =
                RemoteControlResponse::parse(response.as_slice()).expect("inventory response")
            else {
                panic!("expected controller inventory");
            };
            assert_eq!(inventory.hashes().len(), 2);
            assert!(inventory.hashes().contains(&allowed.identity_hash()));
            assert!(inventory.hashes().contains(&extra.identity_hash()));

            let updated_requests =
                RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf);
            let encoded_len = RemoteControlAuthorizeController::write_request(
                extra,
                updated_requests,
                &mut request,
            )
            .expect("authorize request");
            response.clear();
            assert_eq!(
                dispatch_with_node(
                    &mut grants,
                    &node,
                    Some(allowed.identity_hash()),
                    &request[..encoded_len],
                    &mut response,
                )
                .await,
                Ok(()),
            );
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::AuthorizeController(
                    RemoteControlAuthorizeControllerOutcome::Applied,
                )),
            );
            assert_eq!(
                grants.grant_for(&extra.identity_hash()),
                Some(&prior_operator)
            );
            assert_eq!(
                *node.received_controller_grant.lock().unwrap(),
                Some(
                    RemoteControlControllerGrant::new(
                        extra,
                        RemoteControlControllerAuthority::Operator,
                        updated_requests,
                    )
                    .unwrap(),
                ),
            );

            let encoded_len =
                RemoteControlRevokeController::write_request(allowed.identity_hash(), &mut request)
                    .expect("self-revoke request");
            response.clear();
            assert_eq!(
                dispatch_with_node(
                    &mut grants,
                    &node,
                    Some(allowed.identity_hash()),
                    &request[..encoded_len],
                    &mut response,
                )
                .await,
                Ok(()),
            );
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::RevokeController(
                    RemoteControlRevokeControllerOutcome::Forbidden,
                )),
            );
            assert!(grants.contains_controller(&allowed.identity_hash()));

            let encoded_len =
                RemoteControlRevokeController::write_request(extra.identity_hash(), &mut request)
                    .expect("revoke request");
            response.clear();
            assert_eq!(
                dispatch_with_node(
                    &mut grants,
                    &node,
                    Some(allowed.identity_hash()),
                    &request[..encoded_len],
                    &mut response,
                )
                .await,
                Ok(()),
            );
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::RevokeController(
                    RemoteControlRevokeControllerOutcome::Applied,
                )),
            );
            assert!(grants.contains_controller(&extra.identity_hash()));
            assert_eq!(*node.received_revocation.lock().unwrap(), Some(extra));
        });
    }
}
