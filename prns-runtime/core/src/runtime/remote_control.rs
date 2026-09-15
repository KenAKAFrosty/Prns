use crate::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, InstantMillis, SendRequestFailure,
};
use crate::identity::IdentityHash;
use crate::interfaces::{InterfaceId, InterfaceMode};
use crate::remote_control::{
    authorize_remote_control_controller, revoke_remote_control_controller_hash,
    RemoteControlAnnounceSelfOutcome, RemoteControlAuthorizeControllerOutcome,
    RemoteControlBuildVersion, RemoteControlControllerGrantTable, RemoteControlControllerIdentity,
    RemoteControlControllerInventory, RemoteControlDescription, RemoteControlDescriptionError,
    RemoteControlGroupOutcome, RemoteControlInterfaceConfigOutcome, RemoteControlInterfaceGroup,
    RemoteControlInterfaceInventory, RemoteControlInterfacePeersOutcome,
    RemoteControlInterfacePower, RemoteControlLoRaOutcome, RemoteControlLoRaProfile,
    RemoteControlMessageWriteError, RemoteControlModeOutcome, RemoteControlPowerOutcome,
    RemoteControlProtocolError, RemoteControlRequest, RemoteControlRequestKind,
    RemoteControlRequestParseError, RemoteControlRequestSet, RemoteControlResponse,
    RemoteControlResponseKind, RemoteControlResponseParseError,
    RemoteControlRevokeControllerOutcome, RemoteControlSelfAnnouncement, RemoteControlSleepOutcome,
    RemoteControlWifiStation, RemoteControlWifiStationOutcome, REMOTE_CONTROL_REQUEST_ENDPOINT_ID,
};
use crate::routing::links::request::REQUEST_WIRE_OVERHEAD;
use crate::units::ByteLimit;
use crate::wire::DestinationHash;
use prns_core::capabilities::power::PowerSnapshot;

use super::request_endpoints::{
    Decline, InboundRequest, RequestContext, RequestEndpoint, RequestEndpointPolicy, RespondToken,
    ResponseSink,
};
use super::{AnnounceNowError, PrnsNodeApi, SendError};

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

/// Host-side interface apply hooks used by Remote Control Inventory/Power/Mode/Sleep kinds.
///
/// Types that do not expose interface controls can use an empty `impl` and keep the
/// default unavailable/empty outcomes.
pub trait RemoteControlHostControls {
    fn inventory_interfaces(&self) -> RemoteControlInterfaceInventory {
        RemoteControlInterfaceInventory::empty()
    }

    fn set_interface_power(
        &self,
        _id: InterfaceId,
        _power: RemoteControlInterfacePower,
    ) -> RemoteControlPowerOutcome {
        RemoteControlPowerOutcome::Failed
    }

    fn set_interface_mode(
        &self,
        _id: InterfaceId,
        _mode: InterfaceMode,
    ) -> RemoteControlModeOutcome {
        RemoteControlModeOutcome::Failed
    }

    fn set_interface_group(
        &self,
        _id: InterfaceId,
        _group: RemoteControlInterfaceGroup,
    ) -> RemoteControlGroupOutcome {
        RemoteControlGroupOutcome::Failed
    }

    fn inventory_interface_peers(
        &self,
        _id: InterfaceId,
        _offset: u8,
    ) -> RemoteControlInterfacePeersOutcome {
        RemoteControlInterfacePeersOutcome::UnknownInterface
    }

    fn inventory_interface_config(&self, _id: InterfaceId) -> RemoteControlInterfaceConfigOutcome {
        RemoteControlInterfaceConfigOutcome::UnknownInterface
    }

    fn set_interface_lora_profile(
        &self,
        _id: InterfaceId,
        _profile: RemoteControlLoRaProfile,
    ) -> RemoteControlLoRaOutcome {
        RemoteControlLoRaOutcome::Failed
    }

    fn set_interface_wifi_station(
        &self,
        _id: InterfaceId,
        _station: RemoteControlWifiStation,
    ) -> RemoteControlWifiStationOutcome {
        RemoteControlWifiStationOutcome::Failed
    }

    fn build_version(&self) -> RemoteControlBuildVersion {
        RemoteControlBuildVersion::empty()
    }

    fn power_snapshot(&self) -> PowerSnapshot {
        PowerSnapshot::UNKNOWN
    }

    fn sleep_radios(&self) -> RemoteControlSleepOutcome {
        RemoteControlSleepOutcome::Unavailable
    }

    fn wake_radios(&self) -> RemoteControlSleepOutcome {
        RemoteControlSleepOutcome::Unavailable
    }
}

impl RemoteControlHostControls for () {}

pub struct RemoteControlInventoryInterfaces;

impl RemoteControlInventoryInterfaces {
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::InventoryInterfaces;
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
    pub const REQUEST: RemoteControlRequest = RemoteControlRequest::InventoryControllers;
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
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::AuthorizeController { controller }
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
        offset: u8,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlError> {
        RemoteControlRequest::InventoryInterfacePeers { id, offset }
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
            Ok(RemoteControlRequest::InventoryInterfaces) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InventoryInterfaces,
                )?;
                Ok(AdmittedRemoteControlOperation::InventoryInterfaces)
            }
            Ok(RemoteControlRequest::SetInterfacePower { id, power }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetInterfacePower,
                )?;
                Ok(AdmittedRemoteControlOperation::SetInterfacePower { id, power })
            }
            Ok(RemoteControlRequest::SetInterfaceMode { id, mode }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetInterfaceMode,
                )?;
                Ok(AdmittedRemoteControlOperation::SetInterfaceMode { id, mode })
            }
            Ok(RemoteControlRequest::SetInterfaceGroup { id, group }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetInterfaceGroup,
                )?;
                Ok(AdmittedRemoteControlOperation::SetInterfaceGroup { id, group })
            }
            Ok(RemoteControlRequest::InventoryInterfacePeers { id, offset }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InventoryInterfacePeers,
                )?;
                Ok(AdmittedRemoteControlOperation::InventoryInterfacePeers { id, offset })
            }
            Ok(RemoteControlRequest::InventoryInterfaceConfig { id }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InventoryInterfaceConfig,
                )?;
                Ok(AdmittedRemoteControlOperation::InventoryInterfaceConfig { id })
            }
            Ok(RemoteControlRequest::SetInterfaceLoRaProfile { id, profile }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetInterfaceLoRaProfile,
                )?;
                Ok(AdmittedRemoteControlOperation::SetInterfaceLoRaProfile { id, profile })
            }
            Ok(RemoteControlRequest::SetInterfaceWifiStation { id, station }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::SetInterfaceWifiStation,
                )?;
                Ok(AdmittedRemoteControlOperation::SetInterfaceWifiStation { id, station })
            }
            Ok(RemoteControlRequest::InventoryControllers) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::InventoryControllers,
                )?;
                Ok(AdmittedRemoteControlOperation::InventoryControllers)
            }
            Ok(RemoteControlRequest::AuthorizeController { controller }) => {
                require_available(
                    available_requests,
                    RemoteControlRequestKind::AuthorizeController,
                )?;
                Ok(AdmittedRemoteControlOperation::AuthorizeController { controller })
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
                Ok(AdmittedRemoteControlOperation::DescribeBuild)
            }
            Ok(RemoteControlRequest::DescribePower) => {
                require_available(available_requests, RemoteControlRequestKind::DescribePower)?;
                Ok(AdmittedRemoteControlOperation::DescribePower)
            }
            Ok(RemoteControlRequest::SleepRadios) => {
                require_available(available_requests, RemoteControlRequestKind::SleepRadios)?;
                Ok(AdmittedRemoteControlOperation::SleepRadios)
            }
            Ok(RemoteControlRequest::WakeRadios) => {
                require_available(available_requests, RemoteControlRequestKind::WakeRadios)?;
                Ok(AdmittedRemoteControlOperation::WakeRadios)
            }
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
            AdmittedRemoteControlOperation::InventoryInterfaces => {
                RemoteControlResponse::InventoryInterfaces(context.state.inventory_interfaces())
            }
            AdmittedRemoteControlOperation::SetInterfacePower { id, power } => {
                RemoteControlResponse::SetInterfacePower(
                    context.state.set_interface_power(id, power),
                )
            }
            AdmittedRemoteControlOperation::SetInterfaceMode { id, mode } => {
                RemoteControlResponse::SetInterfaceMode(context.state.set_interface_mode(id, mode))
            }
            AdmittedRemoteControlOperation::SetInterfaceGroup { id, group } => {
                RemoteControlResponse::SetInterfaceGroup(
                    context.state.set_interface_group(id, group),
                )
            }
            AdmittedRemoteControlOperation::InventoryInterfacePeers { id, offset } => {
                RemoteControlResponse::InventoryInterfacePeers(
                    context.state.inventory_interface_peers(id, offset),
                )
            }
            AdmittedRemoteControlOperation::InventoryInterfaceConfig { id } => {
                RemoteControlResponse::InventoryInterfaceConfig(
                    context.state.inventory_interface_config(id),
                )
            }
            AdmittedRemoteControlOperation::SetInterfaceLoRaProfile { id, profile } => {
                RemoteControlResponse::SetInterfaceLoRaProfile(
                    context.state.set_interface_lora_profile(id, profile),
                )
            }
            AdmittedRemoteControlOperation::SetInterfaceWifiStation { id, station } => {
                RemoteControlResponse::SetInterfaceWifiStation(
                    context.state.set_interface_wifi_station(id, station),
                )
            }
            AdmittedRemoteControlOperation::InventoryControllers => {
                RemoteControlResponse::InventoryControllers(
                    RemoteControlControllerInventory::empty(),
                )
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
            AdmittedRemoteControlOperation::DescribeBuild => {
                RemoteControlResponse::DescribeBuild(context.state.build_version())
            }
            AdmittedRemoteControlOperation::DescribePower => {
                RemoteControlResponse::DescribePower(context.state.power_snapshot())
            }
            AdmittedRemoteControlOperation::SleepRadios => {
                RemoteControlResponse::SleepRadios(context.state.sleep_radios())
            }
            AdmittedRemoteControlOperation::WakeRadios => {
                RemoteControlResponse::WakeRadios(context.state.wake_radios())
            }
            AdmittedRemoteControlOperation::ProtocolError(error) => {
                RemoteControlResponse::ProtocolError(error)
            }
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
    Describe(RemoteControlDescription),
    AnnounceSelf {
        destination: DestinationHash,
    },
    InventoryInterfaces,
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
        group: RemoteControlInterfaceGroup,
    },
    InventoryInterfacePeers {
        id: InterfaceId,
        offset: u8,
    },
    InventoryInterfaceConfig {
        id: InterfaceId,
    },
    SetInterfaceLoRaProfile {
        id: InterfaceId,
        profile: RemoteControlLoRaProfile,
    },
    SetInterfaceWifiStation {
        id: InterfaceId,
        station: RemoteControlWifiStation,
    },
    InventoryControllers,
    AuthorizeController {
        controller: RemoteControlControllerIdentity,
    },
    RevokeController {
        hash: IdentityHash,
    },
    InventoryControllersReady(RemoteControlControllerInventory),
    AuthorizeControllerReady(RemoteControlAuthorizeControllerOutcome),
    RevokeControllerReady(RemoteControlRevokeControllerOutcome),
    DescribeBuild,
    DescribePower,
    SleepRadios,
    WakeRadios,
    ProtocolError(RemoteControlProtocolError),
}

struct RemoteControlRequestBinding {
    destination: DestinationHash,
    controller: IdentityHash,
    responder: RespondToken,
    requested_at: InstantMillis,
    data: [u8; RemoteControlRequest::MAX_ENCODED_LEN],
    data_len: usize,
}

impl RemoteControlRequestBinding {
    fn new(request: &InboundRequest<'_>) -> Result<Self, RemoteControlAdmitError> {
        let Some(controller) = request.requester else {
            return Err(RemoteControlAdmitError::UnidentifiedRequester);
        };
        let mut data = [0; RemoteControlRequest::MAX_ENCODED_LEN];
        let Some(bound) = data.get_mut(..request.data.len()) else {
            return Err(RemoteControlAdmitError::RequestTooLarge);
        };
        bound.copy_from_slice(request.data);
        Ok(Self {
            destination: request.destination,
            controller,
            responder: request.respond_token(),
            requested_at: request.requested_at,
            data,
            data_len: request.data.len(),
        })
    }

    fn matches(&self, request: &InboundRequest<'_>) -> bool {
        self.destination == request.destination
            && request.requester == Some(self.controller)
            && self.responder == request.respond_token()
            && self.requested_at == request.requested_at
            && self
                .data
                .get(..self.data_len)
                .is_some_and(|data| data == request.data)
    }
}

pub struct AdmittedRemoteControlRequest {
    binding: RemoteControlRequestBinding,
    operation: AdmittedRemoteControlOperation,
}

pub fn admit_remote_control_request<ControllerGrants>(
    controller_grants: &mut ControllerGrants,
    supported_requests: RemoteControlRequestSet,
    self_announcement: RemoteControlSelfAnnouncement,
    request: &InboundRequest<'_>,
) -> Result<AdmittedRemoteControlRequest, RemoteControlAdmitError>
where
    ControllerGrants: RemoteControlControllerGrantTable,
{
    let binding = RemoteControlRequestBinding::new(request)?;
    let Some(grant) = controller_grants.grant_for(&binding.controller).copied() else {
        return Err(RemoteControlAdmitError::NoGrant);
    };
    let available_requests =
        supported_requests.intersection(&grant.permitted_requests().with_current_operator_edits());
    let operation = RemoteControlRequestEndpoint::resolve(
        RemoteControlRequest::parse(request.data),
        available_requests,
        self_announcement,
    )?;
    let operation =
        apply_controller_grant_edits(controller_grants, binding.controller, grant, operation);
    Ok(AdmittedRemoteControlRequest { binding, operation })
}

fn apply_controller_grant_edits<ControllerGrants>(
    controller_grants: &mut ControllerGrants,
    requester: IdentityHash,
    requester_grant: crate::remote_control::RemoteControlControllerGrant,
    operation: AdmittedRemoteControlOperation,
) -> AdmittedRemoteControlOperation
where
    ControllerGrants: RemoteControlControllerGrantTable,
{
    match operation {
        AdmittedRemoteControlOperation::InventoryControllers => {
            AdmittedRemoteControlOperation::InventoryControllersReady(
                RemoteControlControllerInventory::from_grants(controller_grants),
            )
        }
        AdmittedRemoteControlOperation::AuthorizeController { controller } => {
            AdmittedRemoteControlOperation::AuthorizeControllerReady(
                authorize_remote_control_controller(
                    controller_grants,
                    controller,
                    *requester_grant.permitted_requests(),
                ),
            )
        }
        AdmittedRemoteControlOperation::RevokeController { hash } => {
            AdmittedRemoteControlOperation::RevokeControllerReady(
                revoke_remote_control_controller_hash(controller_grants, hash, requester),
            )
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
    if !admission.binding.matches(&request) {
        return Err(Decline::Ignore);
    }
    RemoteControlRequestEndpoint::handle_admitted(
        RequestContext::from_inbound(state, request, sink),
        node,
        admission.operation,
    )
    .await
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
    use core::cell::RefCell;

    struct AnnounceNode {
        result: Result<(), AnnounceNowError>,
        received: RefCell<Option<AnnounceNow>>,
    }

    impl AnnounceNode {
        fn new(result: Result<(), AnnounceNowError>) -> Self {
            Self {
                result,
                received: RefCell::new(None),
            }
        }
    }

    impl PrnsNodeApi for AnnounceNode {
        fn issue(&self, command: crate::engine::PrnsCommand) -> Option<crate::engine::CommandId> {
            <() as PrnsNodeApi>::issue(&(), command)
        }

        async fn announce_now(&self, announce: AnnounceNow) -> Result<(), AnnounceNowError> {
            self.received.replace(Some(announce));
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
        controller_grants_permitting(allowed, RemoteControlRequestSet::all())
    }

    fn controller_grants_permitting(
        allowed: RemoteControlControllerIdentity,
        permitted_requests: RemoteControlRequestSet,
    ) -> FixedRemoteControlControllerGrantTable<1> {
        let mut controller_grants = FixedRemoteControlControllerGrantTable::default();
        controller_grants
            .set_controller_grant(
                RemoteControlControllerGrant::new(allowed, permitted_requests).unwrap(),
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
            &(),
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
                node.received.take(),
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
            assert!(node.received.borrow().is_none());
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
            assert!(node.received.borrow().is_none());

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
            let description = RemoteControlDescription::try_from(
                permitted_requests.with_current_operator_edits(),
            )
            .unwrap();
            assert_eq!(
                RemoteControlResponse::parse(response.as_slice()),
                Ok(RemoteControlResponse::Describe(description)),
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
            <RemoteControlRequestEndpoint as RequestEndpoint<()>>::POLICY,
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
            let description = RemoteControlDescription::try_from(
                available_requests.with_current_operator_edits(),
            )
            .unwrap();
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
                assert!(node.received.borrow().is_none());
            }
        });
    }

    #[test]
    fn admit_names_the_silent_ignore_reasons() {
        let allowed = identity(0x43);
        let mut grants = controller_grants_permitting(
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
                &mut grants,
                RemoteControlRequestSet::all(),
                RemoteControlSelfAnnouncement::Unavailable,
                &inbound(None, &announce).inbound(),
            )
            .err(),
            Some(RemoteControlAdmitError::UnidentifiedRequester),
        );
        assert_eq!(
            admit_remote_control_request(
                &mut grants,
                RemoteControlRequestSet::all(),
                RemoteControlSelfAnnouncement::Unavailable,
                &inbound(Some(identity(0x65).identity_hash()), &announce).inbound(),
            )
            .err(),
            Some(RemoteControlAdmitError::NoGrant),
        );
        assert_eq!(
            admit_remote_control_request(
                &mut grants,
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
                RemoteControlSelfAnnouncement::Unavailable,
                &inbound(Some(allowed.identity_hash()), &announce).inbound(),
            )
            .err(),
            Some(RemoteControlAdmitError::KindNotPermitted),
        );
        let mut all_grants = controller_grants(allowed);
        assert_eq!(
            admit_remote_control_request(
                &mut all_grants,
                RemoteControlRequestSet::all(),
                RemoteControlSelfAnnouncement::Unavailable,
                &inbound(Some(allowed.identity_hash()), &announce).inbound(),
            )
            .err(),
            Some(RemoteControlAdmitError::AnnounceUnavailable),
        );
        assert!(admit_remote_control_request(
            &mut grants,
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
    fn controller_whitelist_inventory_authorize_and_revoke_mutate_the_grant_table() {
        futures_executor::block_on(async {
            let allowed = identity(0x31);
            let extra = identity(0x42);
            let mut grants = FixedRemoteControlControllerGrantTable::<2>::default();
            grants
                .set_controller_grant(
                    RemoteControlControllerGrant::new(allowed, RemoteControlRequestSet::all())
                        .unwrap(),
                )
                .unwrap();

            let mut request = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
            let encoded_len = RemoteControlInventoryControllers::write_request(&mut request)
                .expect("inventory request");
            let mut response =
                heapless::Vec::<u8, { RemoteControlResponse::MAX_ENCODED_LEN }>::new();
            assert_eq!(
                dispatch(
                    &mut grants,
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
            assert_eq!(inventory.hashes(), &[allowed.identity_hash()]);

            let encoded_len = RemoteControlAuthorizeController::write_request(extra, &mut request)
                .expect("authorize request");
            response.clear();
            assert_eq!(
                dispatch(
                    &mut grants,
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
            assert!(grants.contains_controller(&extra.identity_hash()));

            let encoded_len =
                RemoteControlRevokeController::write_request(allowed.identity_hash(), &mut request)
                    .expect("self-revoke request");
            response.clear();
            assert_eq!(
                dispatch(
                    &mut grants,
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
                dispatch(
                    &mut grants,
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
            assert!(!grants.contains_controller(&extra.identity_hash()));
        });
    }
}
