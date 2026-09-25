use crate::capabilities::power::PowerSnapshot;
use crate::identity::{IdentityHash, IDENTITY_PUBLIC_KEY_LEN};
use crate::interfaces::{InterfaceId, InterfaceMode, INTERFACE_ID_LEN};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

use super::inventory::{
    parse_controller_public_keys, RemoteControlAuthorizeControllerOutcome,
    RemoteControlBuildVersion, RemoteControlControllerInventory, RemoteControlDiscoveryGroups,
    RemoteControlDiscoveryGroupsInventoryOutcome, RemoteControlDiscoveryGroupsReplaceOutcome,
    RemoteControlGroupOutcome, RemoteControlInterfaceConfigOutcome, RemoteControlInterfaceGroup,
    RemoteControlInterfaceInventory, RemoteControlInterfacePeersOutcome,
    RemoteControlInterfacePower, RemoteControlLoRaOutcome, RemoteControlLoRaProfile,
    RemoteControlModeOutcome, RemoteControlPowerOutcome, RemoteControlRevokeControllerOutcome,
    RemoteControlSleepOutcome, RemoteControlWifiStation, RemoteControlWifiStationOutcome,
    REMOTE_CONTROL_BUILD_VERSION_CAP, REMOTE_CONTROL_INTERFACE_CONFIG_CAP,
    REMOTE_CONTROL_INTERFACE_ENTRY_ENCODED_LEN, REMOTE_CONTROL_INTERFACE_GROUP_CAP,
    REMOTE_CONTROL_INTERFACE_INVENTORY_CAP,
    REMOTE_CONTROL_INTERFACE_INVENTORY_CONTINUATION_MAX_ENCODED_LEN,
    REMOTE_CONTROL_WIFI_PASSWORD_CAP, REMOTE_CONTROL_WIFI_SSID_CAP,
};
use super::{
    RemoteControlApplyOutcome, RemoteControlControllerIdentity, RemoteControlControllerPage,
    RemoteControlDisplayAutoOff, RemoteControlDisplayVisibility, RemoteControlEspRadioMode,
    RemoteControlGnssPower, RemoteControlInterfacePage, RemoteControlPeerPage,
    RemoteControlStationUplink, RemoteControlSystemPower, RemoteControlWifiCredentialRevision,
    RemoteControlWifiStageOutcome, RemoteControlWifiTransactionStatus,
};

const MESSAGE_HEADER_ENCODED_LEN: usize = 2;
const DESCRIPTION_COUNT_ENCODED_LEN: usize = 1;
const PROTOCOL_ERROR_KIND_ENCODED_LEN: usize = 1;
const PROTOCOL_ERROR_DETAIL_ENCODED_LEN: usize = 1;
// V1 request kinds occupy the contiguous wire range 0x01..=0x1f. Unknown values are rejected
// before a request can enter this typed set, so four bytes represent the complete domain.
const REQUEST_KIND_BITMAP_LEN: usize = 4;

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlProtocolVersion {
        V1 = 1,
    }
}

impl RemoteControlProtocolVersion {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    fn from_wire(value: u8) -> Option<Self> {
        enum_from_wire(value, Self::ALL, Self::wire_value)
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlRequestKind {
        Describe = 0x01,
        AnnounceSelf = 0x02,
        InventoryInterfaces = 0x03,
        SetInterfacePower = 0x04,
        SleepRadios = 0x05,
        WakeRadios = 0x06,
        SetInterfaceMode = 0x07,
        SetInterfaceGroup = 0x08,
        InventoryInterfacePeers = 0x09,
        InventoryInterfaceConfig = 0x0A,
        SetInterfaceLoRaProfile = 0x0B,
        DescribeBuild = 0x0C,
        SetInterfaceWifiStation = 0x0D,
        InventoryControllers = 0x0E,
        AuthorizeController = 0x0F,
        RevokeController = 0x10,
        DescribePower = 0x11,
        SetSystemPower = 0x12,
        SetGnssPower = 0x13,
        SetDisplayVisibility = 0x14,
        SetDisplayAutoOff = 0x15,
        SetStationUplink = 0x16,
        SetEspRadioMode = 0x17,
        StageWifiCredentials = 0x18,
        ActivateWifiCredentials = 0x19,
        ConfirmWifiCredentials = 0x1A,
        CancelWifiCredentials = 0x1B,
        InspectWifiTransaction = 0x1C,
        InventoryInterfaceDiscoveryGroups = 0x1D,
        ReplaceInterfaceDiscoveryGroups = 0x1E,
        /// Permission to stream an image to the install destination. It is not a
        /// remote-control request the node answers. Managing grants receive it through
        /// `effective_requests` when it was added after the grant was stored.
        FirmwareUpdate = 0x1F,
    }
}

impl RemoteControlRequestKind {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        enum_from_wire(value, Self::ALL, Self::wire_value)
    }

    #[must_use]
    pub const fn requires_administrator(self) -> bool {
        matches!(
            self,
            Self::InventoryControllers | Self::AuthorizeController | Self::RevokeController
        )
    }

    #[must_use]
    pub const fn maximum_response_encoded_len(self) -> usize {
        match self {
            Self::Describe => RemoteControlResponse::MAX_ENCODED_LEN,
            Self::AnnounceSelf => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlAnnounceSelfOutcome::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::InventoryInterfaces => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                1usize
                    .saturating_add(
                        REMOTE_CONTROL_INTERFACE_INVENTORY_CAP
                            .saturating_mul(REMOTE_CONTROL_INTERFACE_ENTRY_ENCODED_LEN),
                    )
                    .saturating_add(
                        REMOTE_CONTROL_INTERFACE_INVENTORY_CONTINUATION_MAX_ENCODED_LEN,
                    ),
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::SetInterfacePower => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlPowerOutcome::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::SetInterfaceMode => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlModeOutcome::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::SetInterfaceGroup => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlGroupOutcome::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::InventoryInterfacePeers => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlInterfacePeersOutcome::MAX_ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::InventoryInterfaceConfig => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlInterfaceConfigOutcome::MAX_ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::SetInterfaceLoRaProfile => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlLoRaOutcome::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::SetInterfaceWifiStation => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlWifiStationOutcome::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::InventoryControllers => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlControllerInventory::MAX_ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::AuthorizeController => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlAuthorizeControllerOutcome::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::RevokeController => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlRevokeControllerOutcome::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::DescribeBuild => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlBuildVersion::MAX_ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::DescribePower => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                PowerSnapshot::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::SleepRadios | Self::WakeRadios => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                    RemoteControlSleepOutcome::ENCODED_LEN,
                    RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
                ))
            }
            Self::SetSystemPower
            | Self::SetGnssPower
            | Self::SetDisplayVisibility
            | Self::SetDisplayAutoOff
            | Self::SetStationUplink
            | Self::SetEspRadioMode
            | Self::ActivateWifiCredentials
            | Self::ConfirmWifiCredentials
            | Self::CancelWifiCredentials => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlApplyOutcome::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::StageWifiCredentials => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlWifiStageOutcome::MAX_ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::InspectWifiTransaction => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlWifiTransactionStatus::MAX_ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
            Self::InventoryInterfaceDiscoveryGroups => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                    RemoteControlDiscoveryGroupsInventoryOutcome::MAX_ENCODED_LEN,
                    RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
                ))
            }
            Self::ReplaceInterfaceDiscoveryGroups => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                    RemoteControlDiscoveryGroupsReplaceOutcome::ENCODED_LEN,
                    RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
                ))
            }
            Self::FirmwareUpdate => MESSAGE_HEADER_ENCODED_LEN
                .saturating_add(RemoteControlProtocolError::MAX_ENCODED_BODY_LEN),
        }
    }
}

#[allow(clippy::indexing_slicing)]
const _: () = {
    let mut index = 0;
    while index < RemoteControlRequestKind::ALL.len() {
        assert!(
            (RemoteControlRequestKind::ALL[index] as usize >> 3) < REQUEST_KIND_BITMAP_LEN,
            "every typed request kind must fit the request-set bitmap"
        );
        index += 1;
    }
};

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlResponseKind {
        Describe = 0x01,
        AnnounceSelf = 0x02,
        InventoryInterfaces = 0x03,
        SetInterfacePower = 0x04,
        SleepRadios = 0x05,
        WakeRadios = 0x06,
        SetInterfaceMode = 0x07,
        SetInterfaceGroup = 0x08,
        InventoryInterfacePeers = 0x09,
        InventoryInterfaceConfig = 0x0A,
        SetInterfaceLoRaProfile = 0x0B,
        DescribeBuild = 0x0C,
        SetInterfaceWifiStation = 0x0D,
        InventoryControllers = 0x0E,
        AuthorizeController = 0x0F,
        RevokeController = 0x10,
        DescribePower = 0x11,
        SetSystemPower = 0x12,
        SetGnssPower = 0x13,
        SetDisplayVisibility = 0x14,
        SetDisplayAutoOff = 0x15,
        SetStationUplink = 0x16,
        SetEspRadioMode = 0x17,
        StageWifiCredentials = 0x18,
        ActivateWifiCredentials = 0x19,
        ConfirmWifiCredentials = 0x1A,
        CancelWifiCredentials = 0x1B,
        InspectWifiTransaction = 0x1C,
        InventoryInterfaceDiscoveryGroups = 0x1D,
        ReplaceInterfaceDiscoveryGroups = 0x1E,
        ProtocolError = 0xFF,
    }
}

impl RemoteControlResponseKind {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    fn from_wire(value: u8) -> Option<Self> {
        enum_from_wire(value, Self::ALL, Self::wire_value)
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlProtocolErrorKind {
        MalformedRequest = 0x01,
        UnsupportedVersion = 0x02,
        UnknownRequestKind = 0x03,
        UnsupportedRequest = 0x04,
        Busy = 0x05,
        ApplyFailed = 0x06,
        PersistenceFailed = 0x07,
        RollbackFailed = 0x08,
        InternalFailure = 0x09,
    }
}

impl RemoteControlProtocolErrorKind {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    fn from_wire(value: u8) -> Option<Self> {
        enum_from_wire(value, Self::ALL, Self::wire_value)
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlAnnounceSelfOutcome {
        Announced = 0x01,
        Unavailable = 0x02,
        Rejected = 0x03,
        WriteFailed = 0x04,
    }
}

impl RemoteControlAnnounceSelfOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    fn from_wire(value: u8) -> Option<Self> {
        enum_from_wire(value, Self::ALL, Self::wire_value)
    }

    #[must_use]
    pub const fn encoded_len(self) -> usize {
        Self::ENCODED_LEN
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlRequest {
    Describe,
    AnnounceSelf,
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
        group: RemoteControlInterfaceGroup,
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
    SetInterfaceWifiStation {
        id: InterfaceId,
        station: RemoteControlWifiStation,
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
    SetStationUplink {
        id: InterfaceId,
        uplink: RemoteControlStationUplink,
    },
    SetEspRadioMode {
        mode: RemoteControlEspRadioMode,
    },
    StageWifiCredentials {
        station: RemoteControlWifiStation,
    },
    ActivateWifiCredentials {
        revision: RemoteControlWifiCredentialRevision,
    },
    ConfirmWifiCredentials {
        revision: RemoteControlWifiCredentialRevision,
    },
    CancelWifiCredentials {
        revision: RemoteControlWifiCredentialRevision,
    },
    InspectWifiTransaction,
}

impl RemoteControlRequest {
    pub const MAX_ENCODED_LEN: usize = MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
        INTERFACE_ID_LEN
            .saturating_add(1)
            .saturating_add(REMOTE_CONTROL_WIFI_SSID_CAP)
            .saturating_add(1)
            .saturating_add(REMOTE_CONTROL_WIFI_PASSWORD_CAP),
        INTERFACE_ID_LEN.saturating_add(RemoteControlDiscoveryGroups::MAX_ENCODED_BODY_LEN),
    ));

    #[must_use]
    pub const fn kind(&self) -> RemoteControlRequestKind {
        match self {
            Self::Describe => RemoteControlRequestKind::Describe,
            Self::AnnounceSelf => RemoteControlRequestKind::AnnounceSelf,
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
            Self::SetInterfaceWifiStation { .. } => {
                RemoteControlRequestKind::SetInterfaceWifiStation
            }
            Self::InventoryControllers { .. } => RemoteControlRequestKind::InventoryControllers,
            Self::AuthorizeController { .. } => RemoteControlRequestKind::AuthorizeController,
            Self::RevokeController { .. } => RemoteControlRequestKind::RevokeController,
            Self::DescribeBuild => RemoteControlRequestKind::DescribeBuild,
            Self::DescribePower => RemoteControlRequestKind::DescribePower,
            Self::SleepRadios => RemoteControlRequestKind::SleepRadios,
            Self::WakeRadios => RemoteControlRequestKind::WakeRadios,
            Self::SetSystemPower { .. } => RemoteControlRequestKind::SetSystemPower,
            Self::SetGnssPower { .. } => RemoteControlRequestKind::SetGnssPower,
            Self::SetDisplayVisibility { .. } => RemoteControlRequestKind::SetDisplayVisibility,
            Self::SetDisplayAutoOff { .. } => RemoteControlRequestKind::SetDisplayAutoOff,
            Self::SetStationUplink { .. } => RemoteControlRequestKind::SetStationUplink,
            Self::SetEspRadioMode { .. } => RemoteControlRequestKind::SetEspRadioMode,
            Self::StageWifiCredentials { .. } => RemoteControlRequestKind::StageWifiCredentials,
            Self::ActivateWifiCredentials { .. } => {
                RemoteControlRequestKind::ActivateWifiCredentials
            }
            Self::ConfirmWifiCredentials { .. } => RemoteControlRequestKind::ConfirmWifiCredentials,
            Self::CancelWifiCredentials { .. } => RemoteControlRequestKind::CancelWifiCredentials,
            Self::InspectWifiTransaction => RemoteControlRequestKind::InspectWifiTransaction,
        }
    }

    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        match self {
            Self::Describe
            | Self::AnnounceSelf
            | Self::DescribeBuild
            | Self::DescribePower
            | Self::SleepRadios
            | Self::WakeRadios
            | Self::InspectWifiTransaction => MESSAGE_HEADER_ENCODED_LEN,
            Self::SetSystemPower { .. }
            | Self::SetGnssPower { .. }
            | Self::SetDisplayVisibility { .. }
            | Self::SetDisplayAutoOff { .. }
            | Self::SetEspRadioMode { .. } => MESSAGE_HEADER_ENCODED_LEN.saturating_add(1),
            Self::SetStationUplink { .. } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(INTERFACE_ID_LEN.saturating_add(1))
            }
            Self::StageWifiCredentials { station } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(station.encoded_body_len())
            }
            Self::ActivateWifiCredentials { .. }
            | Self::ConfirmWifiCredentials { .. }
            | Self::CancelWifiCredentials { .. } => MESSAGE_HEADER_ENCODED_LEN.saturating_add(4),
            Self::InventoryInterfaces { page } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(page.encoded_len())
            }
            Self::InventoryControllers { page } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(page.encoded_len())
            }
            Self::SetInterfacePower { .. } | Self::SetInterfaceMode { .. } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(INTERFACE_ID_LEN.saturating_add(1))
            }
            Self::InventoryInterfacePeers { page, .. } => MESSAGE_HEADER_ENCODED_LEN
                .saturating_add(INTERFACE_ID_LEN)
                .saturating_add(page.encoded_len()),
            Self::InventoryInterfaceConfig { .. } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(INTERFACE_ID_LEN)
            }
            Self::InventoryInterfaceDiscoveryGroups { .. } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(INTERFACE_ID_LEN)
            }
            Self::ReplaceInterfaceDiscoveryGroups { groups, .. } => MESSAGE_HEADER_ENCODED_LEN
                .saturating_add(INTERFACE_ID_LEN.saturating_add(groups.encoded_body_len())),
            Self::SetInterfaceGroup { group, .. } => MESSAGE_HEADER_ENCODED_LEN
                .saturating_add(INTERFACE_ID_LEN.saturating_add(group.encoded_body_len())),
            Self::SetInterfaceLoRaProfile { profile, .. } => MESSAGE_HEADER_ENCODED_LEN
                .saturating_add(INTERFACE_ID_LEN.saturating_add(profile.encoded_body_len())),
            Self::SetInterfaceWifiStation { station, .. } => MESSAGE_HEADER_ENCODED_LEN
                .saturating_add(INTERFACE_ID_LEN.saturating_add(station.encoded_body_len())),
            Self::AuthorizeController {
                permitted_requests, ..
            } => MESSAGE_HEADER_ENCODED_LEN
                .saturating_add(IDENTITY_PUBLIC_KEY_LEN)
                .saturating_add(1)
                .saturating_add(permitted_requests.len()),
            Self::RevokeController { .. } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(TRUNCATED_HASH_BYTE_LEN)
            }
        }
    }

    #[must_use]
    pub const fn maximum_response_encoded_len(&self) -> usize {
        self.kind().maximum_response_encoded_len()
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, RemoteControlRequestParseError> {
        let Some((version, rest)) = bytes.split_first() else {
            return Err(RemoteControlRequestParseError::Truncated);
        };
        let Some((kind, body)) = rest.split_first() else {
            return Err(RemoteControlRequestParseError::Truncated);
        };
        if RemoteControlProtocolVersion::from_wire(*version).is_none() {
            return Err(RemoteControlRequestParseError::UnsupportedVersion { found: *version });
        }
        let Some(kind) = RemoteControlRequestKind::from_wire(*kind) else {
            return Err(RemoteControlRequestParseError::UnknownRequestKind { found: *kind });
        };
        match kind {
            RemoteControlRequestKind::Describe if body.is_empty() => Ok(Self::Describe),
            RemoteControlRequestKind::AnnounceSelf if body.is_empty() => Ok(Self::AnnounceSelf),
            RemoteControlRequestKind::InventoryInterfaces => {
                RemoteControlInterfacePage::parse(body)
                    .map(|page| Self::InventoryInterfaces { page })
            }
            RemoteControlRequestKind::InventoryControllers => {
                RemoteControlControllerPage::parse(body)
                    .map(|page| Self::InventoryControllers { page })
            }
            RemoteControlRequestKind::AuthorizeController => parse_authorize_controller(body),
            RemoteControlRequestKind::RevokeController => parse_revoke_controller(body),
            RemoteControlRequestKind::DescribeBuild if body.is_empty() => Ok(Self::DescribeBuild),
            RemoteControlRequestKind::DescribePower if body.is_empty() => Ok(Self::DescribePower),
            RemoteControlRequestKind::SleepRadios if body.is_empty() => Ok(Self::SleepRadios),
            RemoteControlRequestKind::WakeRadios if body.is_empty() => Ok(Self::WakeRadios),
            RemoteControlRequestKind::SetSystemPower => parse_set_system_power(body),
            RemoteControlRequestKind::SetGnssPower => parse_set_gnss_power(body),
            RemoteControlRequestKind::SetDisplayVisibility => parse_set_display_visibility(body),
            RemoteControlRequestKind::SetDisplayAutoOff => parse_set_display_auto_off(body),
            RemoteControlRequestKind::SetStationUplink => parse_set_station_uplink(body),
            RemoteControlRequestKind::SetEspRadioMode => parse_set_esp_radio_mode(body),
            RemoteControlRequestKind::StageWifiCredentials => parse_stage_wifi_credentials(body),
            RemoteControlRequestKind::ActivateWifiCredentials => {
                parse_wifi_revision(body).map(|revision| Self::ActivateWifiCredentials { revision })
            }
            RemoteControlRequestKind::ConfirmWifiCredentials => {
                parse_wifi_revision(body).map(|revision| Self::ConfirmWifiCredentials { revision })
            }
            RemoteControlRequestKind::CancelWifiCredentials => {
                parse_wifi_revision(body).map(|revision| Self::CancelWifiCredentials { revision })
            }
            RemoteControlRequestKind::InspectWifiTransaction if body.is_empty() => {
                Ok(Self::InspectWifiTransaction)
            }
            RemoteControlRequestKind::FirmwareUpdate => {
                Err(RemoteControlRequestParseError::Malformed)
            }
            RemoteControlRequestKind::SetInterfacePower => parse_set_interface_power(body),
            RemoteControlRequestKind::SetInterfaceMode => parse_set_interface_mode(body),
            RemoteControlRequestKind::SetInterfaceGroup => parse_set_interface_group(body),
            RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups => {
                parse_inventory_interface_discovery_groups(body)
            }
            RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups => {
                parse_replace_interface_discovery_groups(body)
            }
            RemoteControlRequestKind::InventoryInterfacePeers => {
                parse_inventory_interface_peers(body)
            }
            RemoteControlRequestKind::InventoryInterfaceConfig => {
                parse_inventory_interface_config(body)
            }
            RemoteControlRequestKind::SetInterfaceLoRaProfile => {
                parse_set_interface_lora_profile(body)
            }
            RemoteControlRequestKind::SetInterfaceWifiStation => {
                parse_set_interface_wifi_station(body)
            }
            RemoteControlRequestKind::Describe
            | RemoteControlRequestKind::AnnounceSelf
            | RemoteControlRequestKind::DescribeBuild
            | RemoteControlRequestKind::DescribePower
            | RemoteControlRequestKind::SleepRadios
            | RemoteControlRequestKind::WakeRadios
            | RemoteControlRequestKind::InspectWifiTransaction => {
                Err(RemoteControlRequestParseError::Malformed)
            }
        }
    }

    pub fn write_into(&self, out: &mut [u8]) -> Result<usize, RemoteControlMessageWriteError> {
        let encoded_len = self.encoded_len();
        let Some(target) = out.get_mut(..encoded_len) else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        let Some((version, rest)) = target.split_first_mut() else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        let Some((kind, body)) = rest.split_first_mut() else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        *version = RemoteControlProtocolVersion::V1.wire_value();
        *kind = self.kind().wire_value();
        match self {
            Self::Describe
            | Self::AnnounceSelf
            | Self::DescribeBuild
            | Self::DescribePower
            | Self::SleepRadios
            | Self::WakeRadios => {}
            Self::InspectWifiTransaction => {}
            Self::InventoryInterfaces { page } => page.write_into(body)?,
            Self::InventoryControllers { page } => page.write_into(body)?,
            Self::SetInterfacePower { id, power } => {
                write_interface_id_and_byte(body, *id, power.wire_value())?;
            }
            Self::SetInterfaceMode { id, mode } => {
                write_interface_id_and_byte(body, *id, mode.wire_value())?;
            }
            Self::SetInterfaceGroup { id, group } => {
                write_interface_id_and_group(body, *id, *group)?;
            }
            Self::InventoryInterfaceDiscoveryGroups { id } => {
                write_interface_id(body, *id)?;
            }
            Self::ReplaceInterfaceDiscoveryGroups { id, groups } => {
                let Some((id_out, groups_out)) = body.split_at_mut_checked(INTERFACE_ID_LEN) else {
                    return Err(RemoteControlMessageWriteError::BufferTooShort);
                };
                id_out.copy_from_slice(id.as_bytes());
                groups.write_body(groups_out)?;
            }
            Self::InventoryInterfacePeers { id, page } => {
                let Some((id_out, page_out)) = body.split_at_mut_checked(INTERFACE_ID_LEN) else {
                    return Err(RemoteControlMessageWriteError::BufferTooShort);
                };
                id_out.copy_from_slice(id.as_bytes());
                page.write_into(page_out)?;
            }
            Self::InventoryInterfaceConfig { id } => {
                write_interface_id(body, *id)?;
            }
            Self::SetInterfaceLoRaProfile { id, profile } => {
                write_interface_id_and_lora_profile(body, *id, *profile)?;
            }
            Self::SetInterfaceWifiStation { id, station } => {
                write_interface_id_and_wifi_station(body, *id, station)?;
            }
            Self::AuthorizeController {
                controller,
                permitted_requests,
            } => {
                write_controller_authorization(body, *controller, *permitted_requests)?;
            }
            Self::RevokeController { hash } => {
                write_controller_hash(body, *hash)?;
            }
            Self::SetSystemPower { power } => write_single_byte(body, power.wire_value())?,
            Self::SetGnssPower { power } => write_single_byte(body, power.wire_value())?,
            Self::SetDisplayVisibility { visibility } => {
                write_single_byte(body, visibility.wire_value())?;
            }
            Self::SetDisplayAutoOff { auto_off } => {
                write_single_byte(body, auto_off.wire_value())?;
            }
            Self::SetStationUplink { id, uplink } => {
                write_interface_id_and_byte(body, *id, uplink.wire_value())?;
            }
            Self::SetEspRadioMode { mode } => write_single_byte(body, mode.wire_value())?,
            Self::StageWifiCredentials { station } => write_wifi_station(body, station)?,
            Self::ActivateWifiCredentials { revision }
            | Self::ConfirmWifiCredentials { revision }
            | Self::CancelWifiCredentials { revision } => {
                write_wifi_revision(body, *revision)?;
            }
        }
        Ok(encoded_len)
    }
}

fn parse_single_byte(body: &[u8]) -> Result<u8, RemoteControlRequestParseError> {
    let [value] = body else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    Ok(*value)
}

fn parse_set_system_power(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let value = parse_single_byte(body)?;
    let power = RemoteControlSystemPower::from_wire(value)
        .ok_or(RemoteControlRequestParseError::Malformed)?;
    Ok(RemoteControlRequest::SetSystemPower { power })
}

fn parse_set_gnss_power(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let value = parse_single_byte(body)?;
    let power = RemoteControlGnssPower::from_wire(value)
        .ok_or(RemoteControlRequestParseError::Malformed)?;
    Ok(RemoteControlRequest::SetGnssPower { power })
}

fn parse_set_display_visibility(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let value = parse_single_byte(body)?;
    let visibility = RemoteControlDisplayVisibility::from_wire(value)
        .ok_or(RemoteControlRequestParseError::Malformed)?;
    Ok(RemoteControlRequest::SetDisplayVisibility { visibility })
}

fn parse_set_display_auto_off(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let value = parse_single_byte(body)?;
    let auto_off = RemoteControlDisplayAutoOff::from_wire(value)
        .ok_or(RemoteControlRequestParseError::Malformed)?;
    Ok(RemoteControlRequest::SetDisplayAutoOff { auto_off })
}

fn parse_set_station_uplink(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let expected = INTERFACE_ID_LEN.saturating_add(1);
    if body.len() != expected {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    }
    let Some((id_bytes, uplink_bytes)) = body.split_at_checked(INTERFACE_ID_LEN) else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let uplink = RemoteControlStationUplink::from_wire(parse_single_byte(uplink_bytes)?)
        .ok_or(RemoteControlRequestParseError::Malformed)?;
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    Ok(RemoteControlRequest::SetStationUplink {
        id: InterfaceId::new(id),
        uplink,
    })
}

fn parse_set_esp_radio_mode(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let value = parse_single_byte(body)?;
    let mode = RemoteControlEspRadioMode::from_wire(value)
        .ok_or(RemoteControlRequestParseError::Malformed)?;
    Ok(RemoteControlRequest::SetEspRadioMode { mode })
}

fn parse_stage_wifi_credentials(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    parse_wifi_station(body).map(|station| RemoteControlRequest::StageWifiCredentials { station })
}

fn parse_wifi_revision(
    body: &[u8],
) -> Result<RemoteControlWifiCredentialRevision, RemoteControlRequestParseError> {
    let bytes: [u8; 4] = body.try_into().map_err(|_| {
        if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        }
    })?;
    RemoteControlWifiCredentialRevision::from_wire(bytes)
        .ok_or(RemoteControlRequestParseError::Malformed)
}

fn parse_set_interface_power(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let expected = INTERFACE_ID_LEN.saturating_add(1);
    let Some(bytes) = body.get(..expected) else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    if body.len() != expected {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let Some((id_bytes, power_byte)) = bytes.split_at_checked(INTERFACE_ID_LEN) else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let Some(power_wire) = power_byte.first().copied() else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let Some(power) = RemoteControlInterfacePower::from_wire(power_wire) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    Ok(RemoteControlRequest::SetInterfacePower {
        id: InterfaceId::new(id),
        power,
    })
}

fn parse_set_interface_mode(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let expected = INTERFACE_ID_LEN.saturating_add(1);
    let Some(bytes) = body.get(..expected) else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    if body.len() != expected {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let Some((id_bytes, mode_byte)) = bytes.split_at_checked(INTERFACE_ID_LEN) else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let Some(mode_wire) = mode_byte.first().copied() else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let Some(mode) = InterfaceMode::from_wire(mode_wire) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    Ok(RemoteControlRequest::SetInterfaceMode {
        id: InterfaceId::new(id),
        mode,
    })
}

fn parse_set_interface_group(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let Some((id_bytes, rest)) = body.split_at_checked(INTERFACE_ID_LEN) else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    let Some((len, group_bytes)) = rest.split_first() else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let group_len = usize::from(*len);
    if group_len == 0 || group_len > REMOTE_CONTROL_INTERFACE_GROUP_CAP {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let Some(group_bytes) = group_bytes.get(..group_len) else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    if group_bytes.len() != rest.len().saturating_sub(1) {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let Ok(text) = core::str::from_utf8(group_bytes) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    let Some(group) = RemoteControlInterfaceGroup::parse(text) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    Ok(RemoteControlRequest::SetInterfaceGroup {
        id: InterfaceId::new(id),
        group,
    })
}

fn parse_inventory_interface_discovery_groups(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    if body.len() != INTERFACE_ID_LEN {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    }
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(body);
    Ok(RemoteControlRequest::InventoryInterfaceDiscoveryGroups {
        id: InterfaceId::new(id),
    })
}

fn parse_replace_interface_discovery_groups(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let Some((id_bytes, groups_bytes)) = body.split_at_checked(INTERFACE_ID_LEN) else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    let groups = RemoteControlDiscoveryGroups::parse_body(groups_bytes)?;
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    Ok(RemoteControlRequest::ReplaceInterfaceDiscoveryGroups {
        id: InterfaceId::new(id),
        groups,
    })
}

fn parse_set_interface_lora_profile(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let Some((id_bytes, rest)) = body.split_at_checked(INTERFACE_ID_LEN) else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    let Some((len, profile_bytes)) = rest.split_first() else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let profile_len = usize::from(*len);
    if profile_len == 0 || profile_len > REMOTE_CONTROL_INTERFACE_CONFIG_CAP {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let Some(profile_bytes) = profile_bytes.get(..profile_len) else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    if profile_bytes.len() != rest.len().saturating_sub(1) {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let Ok(text) = core::str::from_utf8(profile_bytes) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    let Some(profile) = RemoteControlLoRaProfile::parse(text) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    Ok(RemoteControlRequest::SetInterfaceLoRaProfile {
        id: InterfaceId::new(id),
        profile,
    })
}

fn parse_set_interface_wifi_station(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let Some((id_bytes, rest)) = body.split_at_checked(INTERFACE_ID_LEN) else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    let station = parse_wifi_station(rest)?;
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    Ok(RemoteControlRequest::SetInterfaceWifiStation {
        id: InterfaceId::new(id),
        station,
    })
}

fn parse_wifi_station(
    body: &[u8],
) -> Result<RemoteControlWifiStation, RemoteControlRequestParseError> {
    let Some((ssid_len, rest)) = body.split_first() else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let ssid_len = usize::from(*ssid_len);
    if ssid_len == 0 || ssid_len > REMOTE_CONTROL_WIFI_SSID_CAP {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let Some((ssid_bytes, rest)) = rest.split_at_checked(ssid_len) else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let Some((password_len, password_bytes)) = rest.split_first() else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let password_len = usize::from(*password_len);
    if password_len > REMOTE_CONTROL_WIFI_PASSWORD_CAP {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let Some(password_bytes) = password_bytes.get(..password_len) else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    if password_bytes.len() != rest.len().saturating_sub(1) {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let Ok(ssid) = core::str::from_utf8(ssid_bytes) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    let Ok(password) = core::str::from_utf8(password_bytes) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    let Ok(station) = RemoteControlWifiStation::parse(ssid, password) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    Ok(station)
}

fn parse_authorize_controller(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let Some((controller_bytes, permissions)) = body.split_at_checked(IDENTITY_PUBLIC_KEY_LEN)
    else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    let Some(controller) = parse_controller_public_keys(controller_bytes) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    let permitted_requests = parse_operator_request_set(permissions)?;
    Ok(RemoteControlRequest::AuthorizeController {
        controller,
        permitted_requests,
    })
}

fn parse_operator_request_set(
    body: &[u8],
) -> Result<RemoteControlRequestSet, RemoteControlRequestParseError> {
    let Some((count, requests)) = body.split_first() else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    if requests.len() != usize::from(*count) || requests.is_empty() {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let mut permitted_requests = RemoteControlRequestSet::empty();
    let mut previous = None;
    for wire_value in requests {
        let Some(request) = RemoteControlRequestKind::from_wire(*wire_value) else {
            return Err(RemoteControlRequestParseError::UnknownRequestKind { found: *wire_value });
        };
        if request.requires_administrator()
            || previous.is_some_and(|previous| previous >= *wire_value)
            || !permitted_requests.insert(request)
        {
            return Err(RemoteControlRequestParseError::Malformed);
        }
        previous = Some(*wire_value);
    }
    Ok(permitted_requests)
}

fn parse_revoke_controller(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let Some(bytes) = body.get(..TRUNCATED_HASH_BYTE_LEN) else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    if body.len() != TRUNCATED_HASH_BYTE_LEN {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    let mut hash = [0u8; TRUNCATED_HASH_BYTE_LEN];
    hash.copy_from_slice(bytes);
    Ok(RemoteControlRequest::RevokeController {
        hash: IdentityHash::new(hash),
    })
}

fn parse_inventory_interface_peers(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let Some((id_bytes, page_bytes)) = body.split_at_checked(INTERFACE_ID_LEN) else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    let page = RemoteControlPeerPage::parse(page_bytes)?;
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    Ok(RemoteControlRequest::InventoryInterfacePeers {
        id: InterfaceId::new(id),
        page,
    })
}

fn parse_inventory_interface_config(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    if body.len() != INTERFACE_ID_LEN {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    }
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(body);
    Ok(RemoteControlRequest::InventoryInterfaceConfig {
        id: InterfaceId::new(id),
    })
}

fn write_interface_id_and_group(
    body: &mut [u8],
    id: InterfaceId,
    group: RemoteControlInterfaceGroup,
) -> Result<(), RemoteControlMessageWriteError> {
    let Some((id_out, rest)) = body.split_at_mut_checked(INTERFACE_ID_LEN) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    id_out.copy_from_slice(id.as_bytes());
    let Some((len_out, rest)) = rest.split_first_mut() else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    *len_out = group.as_bytes().len() as u8;
    let Some(group_out) = rest.get_mut(..group.as_bytes().len()) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    group_out.copy_from_slice(group.as_bytes());
    Ok(())
}

fn write_interface_id_and_lora_profile(
    body: &mut [u8],
    id: InterfaceId,
    profile: RemoteControlLoRaProfile,
) -> Result<(), RemoteControlMessageWriteError> {
    let Some((id_out, rest)) = body.split_at_mut_checked(INTERFACE_ID_LEN) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    id_out.copy_from_slice(id.as_bytes());
    let Some((len_out, rest)) = rest.split_first_mut() else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    *len_out = profile.as_bytes().len() as u8;
    let Some(profile_out) = rest.get_mut(..profile.as_bytes().len()) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    profile_out.copy_from_slice(profile.as_bytes());
    Ok(())
}

fn write_interface_id_and_wifi_station(
    body: &mut [u8],
    id: InterfaceId,
    station: &RemoteControlWifiStation,
) -> Result<(), RemoteControlMessageWriteError> {
    let Some((id_out, rest)) = body.split_at_mut_checked(INTERFACE_ID_LEN) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    id_out.copy_from_slice(id.as_bytes());
    write_wifi_station(rest, station)
}

fn write_wifi_station(
    body: &mut [u8],
    station: &RemoteControlWifiStation,
) -> Result<(), RemoteControlMessageWriteError> {
    let Some((ssid_len_out, rest)) = body.split_first_mut() else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    *ssid_len_out = station.ssid_bytes().len() as u8;
    let Some((ssid_out, rest)) = rest.split_at_mut_checked(station.ssid_bytes().len()) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    ssid_out.copy_from_slice(station.ssid_bytes());
    let Some((password_len_out, rest)) = rest.split_first_mut() else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    *password_len_out = station.password_bytes().len() as u8;
    let Some(password_out) = rest.get_mut(..station.password_bytes().len()) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    password_out.copy_from_slice(station.password_bytes());
    Ok(())
}

fn write_single_byte(body: &mut [u8], value: u8) -> Result<(), RemoteControlMessageWriteError> {
    let [out] = body else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    *out = value;
    Ok(())
}

fn write_wifi_revision(
    body: &mut [u8],
    revision: RemoteControlWifiCredentialRevision,
) -> Result<(), RemoteControlMessageWriteError> {
    if body.len() != 4 {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    }
    let Some(out) = body.get_mut(..4) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    out.copy_from_slice(&revision.wire_bytes());
    Ok(())
}

fn write_controller_public_keys(
    body: &mut [u8],
    controller: RemoteControlControllerIdentity,
) -> Result<(), RemoteControlMessageWriteError> {
    let keys = controller.public_keys().public_key_bytes();
    let Some(out) = body.get_mut(..keys.len()) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    out.copy_from_slice(&keys);
    Ok(())
}

fn write_controller_hash(
    body: &mut [u8],
    hash: IdentityHash,
) -> Result<(), RemoteControlMessageWriteError> {
    let Some(out) = body.get_mut(..TRUNCATED_HASH_BYTE_LEN) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    out.copy_from_slice(hash.as_bytes());
    Ok(())
}

fn write_interface_id(
    body: &mut [u8],
    id: InterfaceId,
) -> Result<(), RemoteControlMessageWriteError> {
    let Some(id_out) = body.get_mut(..INTERFACE_ID_LEN) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    id_out.copy_from_slice(id.as_bytes());
    Ok(())
}

fn write_interface_id_and_byte(
    body: &mut [u8],
    id: InterfaceId,
    value: u8,
) -> Result<(), RemoteControlMessageWriteError> {
    let Some((id_out, rest)) = body.split_at_mut_checked(INTERFACE_ID_LEN) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    id_out.copy_from_slice(id.as_bytes());
    let Some(slot) = rest.first_mut() else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    *slot = value;
    Ok(())
}

fn write_controller_authorization(
    body: &mut [u8],
    controller: RemoteControlControllerIdentity,
    permitted_requests: RemoteControlRequestSet,
) -> Result<(), RemoteControlMessageWriteError> {
    if permitted_requests.is_empty()
        || permitted_requests
            .iter()
            .any(RemoteControlRequestKind::requires_administrator)
    {
        return Err(RemoteControlMessageWriteError::InvalidRequestSet);
    }
    let Some((public_keys, permissions)) = body.split_at_mut_checked(IDENTITY_PUBLIC_KEY_LEN)
    else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    write_controller_public_keys(public_keys, controller)?;
    let Some((count, requests)) = permissions.split_first_mut() else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    *count = permitted_requests.wire_count();
    for (out, request) in requests.iter_mut().zip(permitted_requests.iter()) {
        *out = request.wire_value();
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlRequestSet {
    bits: [u8; REQUEST_KIND_BITMAP_LEN],
    len: u8,
}

impl RemoteControlRequestSet {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            bits: [0; REQUEST_KIND_BITMAP_LEN],
            len: 0,
        }
    }

    #[must_use]
    pub fn only(kind: RemoteControlRequestKind) -> Self {
        let mut requests = Self::empty();
        let _inserted = requests.insert(kind);
        requests
    }

    #[must_use]
    pub fn all() -> Self {
        let mut supported = Self::empty();
        for kind in RemoteControlRequestKind::ALL {
            let _inserted = supported.insert(kind);
        }
        supported
    }

    #[must_use]
    pub fn all_operator() -> Self {
        let mut supported = Self::empty();
        for kind in RemoteControlRequestKind::ALL {
            if !kind.requires_administrator() {
                let _inserted = supported.insert(kind);
            }
        }
        supported
    }

    #[must_use]
    pub fn supports(&self, kind: RemoteControlRequestKind) -> bool {
        let (index, mask) = request_kind_position(kind);
        self.bits.get(index).is_some_and(|byte| *byte & mask != 0)
    }

    pub fn insert(&mut self, kind: RemoteControlRequestKind) -> bool {
        let (index, mask) = request_kind_position(kind);
        let Some(byte) = self.bits.get_mut(index) else {
            return false;
        };
        if *byte & mask != 0 {
            return false;
        }
        let Some(len) = self.len.checked_add(1) else {
            return false;
        };
        *byte |= mask;
        self.len = len;
        true
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) const fn wire_count(&self) -> u8 {
        self.len
    }

    pub fn iter(&self) -> impl Iterator<Item = RemoteControlRequestKind> + '_ {
        RemoteControlRequestKind::ALL
            .into_iter()
            .filter(|kind| self.supports(*kind))
    }

    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        let mut bits = [0; REQUEST_KIND_BITMAP_LEN];
        let mut len = 0u8;
        for ((byte, left), right) in bits.iter_mut().zip(self.bits).zip(other.bits) {
            *byte = left & right;
            len = len.saturating_add((*byte).count_ones() as u8);
        }
        Self { bits, len }
    }
}

impl core::fmt::Debug for RemoteControlRequestSet {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_set().entries(self.iter()).finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlDescription {
    available_requests: RemoteControlRequestSet,
}

impl RemoteControlDescription {
    #[must_use]
    pub const fn available_requests(&self) -> &RemoteControlRequestSet {
        &self.available_requests
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlDescriptionError {
    DescribeUnavailable,
}

impl TryFrom<RemoteControlRequestSet> for RemoteControlDescription {
    type Error = RemoteControlDescriptionError;

    fn try_from(available_requests: RemoteControlRequestSet) -> Result<Self, Self::Error> {
        if !available_requests.supports(RemoteControlRequestKind::Describe) {
            return Err(RemoteControlDescriptionError::DescribeUnavailable);
        }
        Ok(Self { available_requests })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlProtocolError {
    MalformedRequest,
    UnsupportedVersion { found: u8 },
    UnknownRequestKind { found: u8 },
    UnsupportedRequest { request: RemoteControlRequestKind },
    Busy { request: RemoteControlRequestKind },
    ApplyFailed { request: RemoteControlRequestKind },
    PersistenceFailed { request: RemoteControlRequestKind },
    RollbackFailed { request: RemoteControlRequestKind },
    InternalFailure { request: RemoteControlRequestKind },
}

impl RemoteControlProtocolError {
    const MAX_ENCODED_BODY_LEN: usize =
        PROTOCOL_ERROR_KIND_ENCODED_LEN.saturating_add(PROTOCOL_ERROR_DETAIL_ENCODED_LEN);

    #[must_use]
    pub const fn kind(self) -> RemoteControlProtocolErrorKind {
        match self {
            Self::MalformedRequest => RemoteControlProtocolErrorKind::MalformedRequest,
            Self::UnsupportedVersion { .. } => RemoteControlProtocolErrorKind::UnsupportedVersion,
            Self::UnknownRequestKind { .. } => RemoteControlProtocolErrorKind::UnknownRequestKind,
            Self::UnsupportedRequest { .. } => RemoteControlProtocolErrorKind::UnsupportedRequest,
            Self::Busy { .. } => RemoteControlProtocolErrorKind::Busy,
            Self::ApplyFailed { .. } => RemoteControlProtocolErrorKind::ApplyFailed,
            Self::PersistenceFailed { .. } => RemoteControlProtocolErrorKind::PersistenceFailed,
            Self::RollbackFailed { .. } => RemoteControlProtocolErrorKind::RollbackFailed,
            Self::InternalFailure { .. } => RemoteControlProtocolErrorKind::InternalFailure,
        }
    }

    const fn encoded_body_len(self) -> usize {
        match self {
            Self::MalformedRequest => PROTOCOL_ERROR_KIND_ENCODED_LEN,
            Self::UnsupportedVersion { .. }
            | Self::UnknownRequestKind { .. }
            | Self::UnsupportedRequest { .. }
            | Self::Busy { .. }
            | Self::ApplyFailed { .. }
            | Self::PersistenceFailed { .. }
            | Self::RollbackFailed { .. }
            | Self::InternalFailure { .. } => Self::MAX_ENCODED_BODY_LEN,
        }
    }

    const fn found(self) -> Option<u8> {
        match self {
            Self::MalformedRequest => None,
            Self::UnsupportedVersion { found } | Self::UnknownRequestKind { found } => Some(found),
            Self::UnsupportedRequest { request }
            | Self::Busy { request }
            | Self::ApplyFailed { request }
            | Self::PersistenceFailed { request }
            | Self::RollbackFailed { request }
            | Self::InternalFailure { request } => Some(request.wire_value()),
        }
    }
}

impl From<RemoteControlRequestParseError> for RemoteControlProtocolError {
    fn from(error: RemoteControlRequestParseError) -> Self {
        match error {
            RemoteControlRequestParseError::Truncated
            | RemoteControlRequestParseError::Malformed => Self::MalformedRequest,
            RemoteControlRequestParseError::UnsupportedVersion { found } => {
                Self::UnsupportedVersion { found }
            }
            RemoteControlRequestParseError::UnknownRequestKind { found } => {
                Self::UnknownRequestKind { found }
            }
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteControlResponse {
    Describe(RemoteControlDescription),
    AnnounceSelf(RemoteControlAnnounceSelfOutcome),
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
    InventoryControllers(RemoteControlControllerInventory),
    AuthorizeController(RemoteControlAuthorizeControllerOutcome),
    RevokeController(RemoteControlRevokeControllerOutcome),
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
    ProtocolError(RemoteControlProtocolError),
}

impl RemoteControlResponse {
    pub const MAX_ENCODED_LEN: usize = MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
        RemoteControlDiscoveryGroupsInventoryOutcome::MAX_ENCODED_LEN,
        maximum(
            DESCRIPTION_COUNT_ENCODED_LEN.saturating_add(RemoteControlRequestKind::ALL.len()),
            maximum(
                1usize
                    .saturating_add(
                        REMOTE_CONTROL_INTERFACE_INVENTORY_CAP
                            .saturating_mul(REMOTE_CONTROL_INTERFACE_ENTRY_ENCODED_LEN),
                    )
                    .saturating_add(REMOTE_CONTROL_INTERFACE_INVENTORY_CONTINUATION_MAX_ENCODED_LEN),
                maximum(
                    RemoteControlInterfacePeersOutcome::MAX_ENCODED_LEN,
                    maximum(
                        RemoteControlInterfaceConfigOutcome::MAX_ENCODED_LEN,
                        maximum(
                            RemoteControlControllerInventory::MAX_ENCODED_LEN,
                            maximum(
                                RemoteControlBuildVersion::MAX_ENCODED_LEN,
                                maximum(
                                    PowerSnapshot::ENCODED_LEN,
                                    maximum(
                                        RemoteControlAnnounceSelfOutcome::ENCODED_LEN,
                                        maximum(
                                            RemoteControlPowerOutcome::ENCODED_LEN,
                                            maximum(
                                                RemoteControlModeOutcome::ENCODED_LEN,
                                                maximum(
                                                    RemoteControlSleepOutcome::ENCODED_LEN,
                                                    RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
                                                ),
                                            ),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
        ),
    ));

    #[must_use]
    pub const fn kind(&self) -> RemoteControlResponseKind {
        match self {
            Self::Describe(_) => RemoteControlResponseKind::Describe,
            Self::AnnounceSelf(_) => RemoteControlResponseKind::AnnounceSelf,
            Self::InventoryInterfaces(_) => RemoteControlResponseKind::InventoryInterfaces,
            Self::SetInterfacePower(_) => RemoteControlResponseKind::SetInterfacePower,
            Self::SetInterfaceMode(_) => RemoteControlResponseKind::SetInterfaceMode,
            Self::SetInterfaceGroup(_) => RemoteControlResponseKind::SetInterfaceGroup,
            Self::InventoryInterfaceDiscoveryGroups(_) => {
                RemoteControlResponseKind::InventoryInterfaceDiscoveryGroups
            }
            Self::ReplaceInterfaceDiscoveryGroups(_) => {
                RemoteControlResponseKind::ReplaceInterfaceDiscoveryGroups
            }
            Self::InventoryInterfacePeers(_) => RemoteControlResponseKind::InventoryInterfacePeers,
            Self::InventoryInterfaceConfig(_) => {
                RemoteControlResponseKind::InventoryInterfaceConfig
            }
            Self::SetInterfaceLoRaProfile(_) => RemoteControlResponseKind::SetInterfaceLoRaProfile,
            Self::SetInterfaceWifiStation(_) => RemoteControlResponseKind::SetInterfaceWifiStation,
            Self::InventoryControllers(_) => RemoteControlResponseKind::InventoryControllers,
            Self::AuthorizeController(_) => RemoteControlResponseKind::AuthorizeController,
            Self::RevokeController(_) => RemoteControlResponseKind::RevokeController,
            Self::DescribeBuild(_) => RemoteControlResponseKind::DescribeBuild,
            Self::DescribePower(_) => RemoteControlResponseKind::DescribePower,
            Self::SleepRadios(_) => RemoteControlResponseKind::SleepRadios,
            Self::WakeRadios(_) => RemoteControlResponseKind::WakeRadios,
            Self::SetSystemPower(_) => RemoteControlResponseKind::SetSystemPower,
            Self::SetGnssPower(_) => RemoteControlResponseKind::SetGnssPower,
            Self::SetDisplayVisibility(_) => RemoteControlResponseKind::SetDisplayVisibility,
            Self::SetDisplayAutoOff(_) => RemoteControlResponseKind::SetDisplayAutoOff,
            Self::SetStationUplink(_) => RemoteControlResponseKind::SetStationUplink,
            Self::SetEspRadioMode(_) => RemoteControlResponseKind::SetEspRadioMode,
            Self::StageWifiCredentials(_) => RemoteControlResponseKind::StageWifiCredentials,
            Self::ActivateWifiCredentials(_) => RemoteControlResponseKind::ActivateWifiCredentials,
            Self::ConfirmWifiCredentials(_) => RemoteControlResponseKind::ConfirmWifiCredentials,
            Self::CancelWifiCredentials(_) => RemoteControlResponseKind::CancelWifiCredentials,
            Self::InspectWifiTransaction(_) => RemoteControlResponseKind::InspectWifiTransaction,
            Self::ProtocolError(_) => RemoteControlResponseKind::ProtocolError,
        }
    }

    #[must_use]
    pub fn encoded_len(&self) -> usize {
        let body_len = match self {
            Self::Describe(description) => {
                DESCRIPTION_COUNT_ENCODED_LEN.saturating_add(description.available_requests.len())
            }
            Self::AnnounceSelf(outcome) => outcome.encoded_len(),
            Self::InventoryInterfaces(inventory) => inventory.encoded_body_len(),
            Self::SetInterfacePower(_) => RemoteControlPowerOutcome::ENCODED_LEN,
            Self::SetInterfaceMode(_) => RemoteControlModeOutcome::ENCODED_LEN,
            Self::SetInterfaceGroup(_) => RemoteControlGroupOutcome::ENCODED_LEN,
            Self::InventoryInterfaceDiscoveryGroups(outcome) => outcome.encoded_len(),
            Self::ReplaceInterfaceDiscoveryGroups(_) => {
                RemoteControlDiscoveryGroupsReplaceOutcome::ENCODED_LEN
            }
            Self::InventoryInterfacePeers(outcome) => outcome.encoded_body_len(),
            Self::InventoryInterfaceConfig(outcome) => outcome.encoded_body_len(),
            Self::SetInterfaceLoRaProfile(_) => RemoteControlLoRaOutcome::ENCODED_LEN,
            Self::SetInterfaceWifiStation(_) => RemoteControlWifiStationOutcome::ENCODED_LEN,
            Self::InventoryControllers(inventory) => inventory.encoded_body_len(),
            Self::AuthorizeController(_) => RemoteControlAuthorizeControllerOutcome::ENCODED_LEN,
            Self::RevokeController(_) => RemoteControlRevokeControllerOutcome::ENCODED_LEN,
            Self::DescribeBuild(version) => version.encoded_body_len(),
            Self::DescribePower(snapshot) => snapshot.encoded_body_len(),
            Self::SleepRadios(_) | Self::WakeRadios(_) => RemoteControlSleepOutcome::ENCODED_LEN,
            Self::SetSystemPower(_)
            | Self::SetGnssPower(_)
            | Self::SetDisplayVisibility(_)
            | Self::SetDisplayAutoOff(_)
            | Self::SetStationUplink(_)
            | Self::SetEspRadioMode(_)
            | Self::ActivateWifiCredentials(_)
            | Self::ConfirmWifiCredentials(_)
            | Self::CancelWifiCredentials(_) => RemoteControlApplyOutcome::ENCODED_LEN,
            Self::StageWifiCredentials(outcome) => outcome.encoded_len(),
            Self::InspectWifiTransaction(status) => status.encoded_len(),
            Self::ProtocolError(error) => error.encoded_body_len(),
        };
        MESSAGE_HEADER_ENCODED_LEN.saturating_add(body_len)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, RemoteControlResponseParseError> {
        let Some((version, rest)) = bytes.split_first() else {
            return Err(RemoteControlResponseParseError::Truncated);
        };
        let Some((kind, body)) = rest.split_first() else {
            return Err(RemoteControlResponseParseError::Truncated);
        };
        if RemoteControlProtocolVersion::from_wire(*version).is_none() {
            return Err(RemoteControlResponseParseError::UnsupportedVersion { found: *version });
        }
        let Some(kind) = RemoteControlResponseKind::from_wire(*kind) else {
            return Err(RemoteControlResponseParseError::UnknownResponseKind { found: *kind });
        };
        match kind {
            RemoteControlResponseKind::Describe => parse_description(body).map(Self::Describe),
            RemoteControlResponseKind::AnnounceSelf => {
                parse_announce_self_outcome(body).map(Self::AnnounceSelf)
            }
            RemoteControlResponseKind::InventoryInterfaces => {
                RemoteControlInterfaceInventory::parse_body(body).map(Self::InventoryInterfaces)
            }
            RemoteControlResponseKind::SetInterfacePower => {
                parse_power_outcome(body).map(Self::SetInterfacePower)
            }
            RemoteControlResponseKind::SetInterfaceMode => {
                parse_mode_outcome(body).map(Self::SetInterfaceMode)
            }
            RemoteControlResponseKind::SetInterfaceGroup => {
                parse_group_outcome(body).map(Self::SetInterfaceGroup)
            }
            RemoteControlResponseKind::InventoryInterfaceDiscoveryGroups => {
                RemoteControlDiscoveryGroupsInventoryOutcome::parse_body(body)
                    .map(Self::InventoryInterfaceDiscoveryGroups)
            }
            RemoteControlResponseKind::ReplaceInterfaceDiscoveryGroups => {
                parse_discovery_groups_replace_outcome(body)
                    .map(Self::ReplaceInterfaceDiscoveryGroups)
            }
            RemoteControlResponseKind::InventoryInterfacePeers => {
                RemoteControlInterfacePeersOutcome::parse_body(body)
                    .map(Self::InventoryInterfacePeers)
            }
            RemoteControlResponseKind::InventoryInterfaceConfig => {
                RemoteControlInterfaceConfigOutcome::parse_body(body)
                    .map(Self::InventoryInterfaceConfig)
            }
            RemoteControlResponseKind::SetInterfaceLoRaProfile => {
                parse_lora_outcome(body).map(Self::SetInterfaceLoRaProfile)
            }
            RemoteControlResponseKind::SetInterfaceWifiStation => {
                parse_wifi_station_outcome(body).map(Self::SetInterfaceWifiStation)
            }
            RemoteControlResponseKind::InventoryControllers => {
                RemoteControlControllerInventory::parse_body(body)
                    .map(Self::InventoryControllers)
                    .ok_or(if body.is_empty() {
                        RemoteControlResponseParseError::Truncated
                    } else {
                        RemoteControlResponseParseError::Malformed
                    })
            }
            RemoteControlResponseKind::AuthorizeController => {
                parse_authorize_controller_outcome(body).map(Self::AuthorizeController)
            }
            RemoteControlResponseKind::RevokeController => {
                parse_revoke_controller_outcome(body).map(Self::RevokeController)
            }
            RemoteControlResponseKind::DescribeBuild => {
                parse_build_version(body).map(Self::DescribeBuild)
            }
            RemoteControlResponseKind::DescribePower => {
                parse_power_snapshot(body).map(Self::DescribePower)
            }
            RemoteControlResponseKind::SleepRadios => {
                parse_sleep_outcome(body).map(Self::SleepRadios)
            }
            RemoteControlResponseKind::WakeRadios => {
                parse_sleep_outcome(body).map(Self::WakeRadios)
            }
            RemoteControlResponseKind::SetSystemPower => {
                parse_apply_outcome(body).map(Self::SetSystemPower)
            }
            RemoteControlResponseKind::SetGnssPower => {
                parse_apply_outcome(body).map(Self::SetGnssPower)
            }
            RemoteControlResponseKind::SetDisplayVisibility => {
                parse_apply_outcome(body).map(Self::SetDisplayVisibility)
            }
            RemoteControlResponseKind::SetDisplayAutoOff => {
                parse_apply_outcome(body).map(Self::SetDisplayAutoOff)
            }
            RemoteControlResponseKind::SetStationUplink => {
                parse_apply_outcome(body).map(Self::SetStationUplink)
            }
            RemoteControlResponseKind::SetEspRadioMode => {
                parse_apply_outcome(body).map(Self::SetEspRadioMode)
            }
            RemoteControlResponseKind::StageWifiCredentials => {
                parse_wifi_stage_outcome(body).map(Self::StageWifiCredentials)
            }
            RemoteControlResponseKind::ActivateWifiCredentials => {
                parse_apply_outcome(body).map(Self::ActivateWifiCredentials)
            }
            RemoteControlResponseKind::ConfirmWifiCredentials => {
                parse_apply_outcome(body).map(Self::ConfirmWifiCredentials)
            }
            RemoteControlResponseKind::CancelWifiCredentials => {
                parse_apply_outcome(body).map(Self::CancelWifiCredentials)
            }
            RemoteControlResponseKind::InspectWifiTransaction => {
                parse_wifi_transaction_status(body).map(Self::InspectWifiTransaction)
            }
            RemoteControlResponseKind::ProtocolError => {
                parse_protocol_error(body).map(Self::ProtocolError)
            }
        }
    }

    pub fn write_into(&self, out: &mut [u8]) -> Result<usize, RemoteControlMessageWriteError> {
        let encoded_len = self.encoded_len();
        let Some(target) = out.get_mut(..encoded_len) else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        let Some((version, rest)) = target.split_first_mut() else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        let Some((kind, body)) = rest.split_first_mut() else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        *version = RemoteControlProtocolVersion::V1.wire_value();
        *kind = self.kind().wire_value();
        match self {
            Self::Describe(description) => write_description(description, body),
            Self::AnnounceSelf(outcome) => write_announce_self_outcome(*outcome, body),
            Self::InventoryInterfaces(inventory) => inventory.write_body(body)?,
            Self::SetInterfacePower(outcome) => write_power_outcome(*outcome, body),
            Self::SetInterfaceMode(outcome) => write_mode_outcome(*outcome, body),
            Self::SetInterfaceGroup(outcome) => write_group_outcome(*outcome, body),
            Self::InventoryInterfaceDiscoveryGroups(outcome) => outcome.write_body(body)?,
            Self::ReplaceInterfaceDiscoveryGroups(outcome) => {
                write_discovery_groups_replace_outcome(*outcome, body)
            }
            Self::InventoryInterfacePeers(outcome) => outcome.write_body(body)?,
            Self::InventoryInterfaceConfig(outcome) => outcome.write_body(body)?,
            Self::SetInterfaceLoRaProfile(outcome) => write_lora_outcome(*outcome, body),
            Self::SetInterfaceWifiStation(outcome) => write_wifi_station_outcome(*outcome, body),
            Self::InventoryControllers(inventory) => inventory
                .write_body(body)
                .ok_or(RemoteControlMessageWriteError::BufferTooShort)?,
            Self::AuthorizeController(outcome) => {
                write_authorize_controller_outcome(*outcome, body)
            }
            Self::RevokeController(outcome) => write_revoke_controller_outcome(*outcome, body),
            Self::DescribeBuild(version) => write_build_version(*version, body),
            Self::DescribePower(snapshot) => write_power_snapshot(*snapshot, body),
            Self::SleepRadios(outcome) | Self::WakeRadios(outcome) => {
                write_sleep_outcome(*outcome, body)
            }
            Self::SetSystemPower(outcome)
            | Self::SetGnssPower(outcome)
            | Self::SetDisplayVisibility(outcome)
            | Self::SetDisplayAutoOff(outcome)
            | Self::SetStationUplink(outcome)
            | Self::SetEspRadioMode(outcome)
            | Self::ActivateWifiCredentials(outcome)
            | Self::ConfirmWifiCredentials(outcome)
            | Self::CancelWifiCredentials(outcome) => write_apply_outcome(*outcome, body),
            Self::StageWifiCredentials(outcome) => write_wifi_stage_outcome(*outcome, body),
            Self::InspectWifiTransaction(status) => write_wifi_transaction_status(*status, body),
            Self::ProtocolError(error) => write_protocol_error(error, body),
        }
        Ok(encoded_len)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlRequestParseError {
    Truncated,
    UnsupportedVersion { found: u8 },
    UnknownRequestKind { found: u8 },
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlResponseParseError {
    Truncated,
    UnsupportedVersion { found: u8 },
    UnknownResponseKind { found: u8 },
    UnknownAnnounceSelfOutcome { found: u8 },
    UnknownPowerOutcome { found: u8 },
    UnknownModeOutcome { found: u8 },
    UnknownGroupOutcome { found: u8 },
    UnknownLoRaOutcome { found: u8 },
    UnknownWifiStationOutcome { found: u8 },
    UnknownAuthorizeControllerOutcome { found: u8 },
    UnknownRevokeControllerOutcome { found: u8 },
    UnknownSleepOutcome { found: u8 },
    UnknownApplyOutcome { found: u8 },
    UnknownWifiStageOutcome { found: u8 },
    UnknownWifiTransactionStatus { found: u8 },
    UnknownProtocolErrorKind { found: u8 },
    UnknownRequestKind { found: u8 },
    UnknownInterfaceKind { found: u8 },
    UnknownInterfaceMode { found: u8 },
    UnknownConnectionState { found: u8 },
    NonCanonicalRequestSet,
    NonCanonicalCursor,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlMessageWriteError {
    BufferTooShort,
    InvalidRequestSet,
}

fn request_kind_position(kind: RemoteControlRequestKind) -> (usize, u8) {
    let wire_value = kind.wire_value();
    let index = usize::from(wire_value >> 3);
    let mask = 1u8.wrapping_shl(u32::from(wire_value & 0x07));
    (index, mask)
}

fn enum_from_wire<T: Copy, const N: usize>(
    value: u8,
    variants: [T; N],
    wire_value: impl Fn(T) -> u8,
) -> Option<T> {
    variants
        .into_iter()
        .find(|variant| wire_value(*variant) == value)
}

const fn maximum(left: usize, right: usize) -> usize {
    if left > right {
        left
    } else {
        right
    }
}

fn parse_description(
    body: &[u8],
) -> Result<RemoteControlDescription, RemoteControlResponseParseError> {
    let Some((count, kinds)) = body.split_first() else {
        return Err(RemoteControlResponseParseError::Truncated);
    };
    if kinds.len() != usize::from(*count) {
        return Err(RemoteControlResponseParseError::Malformed);
    }
    let mut available_requests = RemoteControlRequestSet::empty();
    let mut previous = None;
    for wire_value in kinds {
        let Some(kind) = RemoteControlRequestKind::from_wire(*wire_value) else {
            return Err(RemoteControlResponseParseError::UnknownRequestKind { found: *wire_value });
        };
        if previous.is_some_and(|value| value >= *wire_value) || !available_requests.insert(kind) {
            return Err(RemoteControlResponseParseError::NonCanonicalRequestSet);
        }
        previous = Some(*wire_value);
    }
    RemoteControlDescription::try_from(available_requests).map_err(
        |RemoteControlDescriptionError::DescribeUnavailable| {
            RemoteControlResponseParseError::Malformed
        },
    )
}

fn parse_announce_self_outcome(
    body: &[u8],
) -> Result<RemoteControlAnnounceSelfOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlAnnounceSelfOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::UnknownAnnounceSelfOutcome { found: *outcome })
}

fn parse_power_outcome(
    body: &[u8],
) -> Result<RemoteControlPowerOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlPowerOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::UnknownPowerOutcome { found: *outcome })
}

fn parse_mode_outcome(
    body: &[u8],
) -> Result<RemoteControlModeOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlModeOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::UnknownModeOutcome { found: *outcome })
}

fn parse_build_version(
    body: &[u8],
) -> Result<RemoteControlBuildVersion, RemoteControlResponseParseError> {
    let Some((len, rest)) = body.split_first() else {
        return Err(RemoteControlResponseParseError::Truncated);
    };
    let version_len = usize::from(*len);
    if version_len > REMOTE_CONTROL_BUILD_VERSION_CAP {
        return Err(RemoteControlResponseParseError::Malformed);
    }
    let Some(bytes) = rest.get(..version_len) else {
        return Err(RemoteControlResponseParseError::Truncated);
    };
    if bytes.len() != body.len().saturating_sub(1) {
        return Err(RemoteControlResponseParseError::Malformed);
    }
    let Ok(text) = core::str::from_utf8(bytes) else {
        return Err(RemoteControlResponseParseError::Malformed);
    };
    RemoteControlBuildVersion::from_text(text).ok_or(RemoteControlResponseParseError::Malformed)
}

fn parse_power_snapshot(body: &[u8]) -> Result<PowerSnapshot, RemoteControlResponseParseError> {
    let Some((snapshot, rest)) = PowerSnapshot::parse(body) else {
        return Err(if body.len() < PowerSnapshot::ENCODED_LEN {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    if !rest.is_empty() {
        return Err(RemoteControlResponseParseError::Malformed);
    }
    Ok(snapshot)
}

fn parse_lora_outcome(
    body: &[u8],
) -> Result<RemoteControlLoRaOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlLoRaOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::UnknownLoRaOutcome { found: *outcome })
}

fn parse_wifi_station_outcome(
    body: &[u8],
) -> Result<RemoteControlWifiStationOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlWifiStationOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::UnknownWifiStationOutcome { found: *outcome })
}

fn parse_authorize_controller_outcome(
    body: &[u8],
) -> Result<RemoteControlAuthorizeControllerOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlAuthorizeControllerOutcome::from_wire(*outcome).ok_or(
        RemoteControlResponseParseError::UnknownAuthorizeControllerOutcome { found: *outcome },
    )
}

fn parse_revoke_controller_outcome(
    body: &[u8],
) -> Result<RemoteControlRevokeControllerOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlRevokeControllerOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::UnknownRevokeControllerOutcome { found: *outcome })
}

fn parse_group_outcome(
    body: &[u8],
) -> Result<RemoteControlGroupOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlGroupOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::UnknownGroupOutcome { found: *outcome })
}

fn parse_discovery_groups_replace_outcome(
    body: &[u8],
) -> Result<RemoteControlDiscoveryGroupsReplaceOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlDiscoveryGroupsReplaceOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::Malformed)
}

fn parse_sleep_outcome(
    body: &[u8],
) -> Result<RemoteControlSleepOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlSleepOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::UnknownSleepOutcome { found: *outcome })
}

fn parse_apply_outcome(
    body: &[u8],
) -> Result<RemoteControlApplyOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlApplyOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::UnknownApplyOutcome { found: *outcome })
}

fn parse_response_wifi_revision(
    body: &[u8],
) -> Result<RemoteControlWifiCredentialRevision, RemoteControlResponseParseError> {
    let bytes: [u8; 4] = body.try_into().map_err(|_| {
        if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        }
    })?;
    RemoteControlWifiCredentialRevision::from_wire(bytes)
        .ok_or(RemoteControlResponseParseError::Malformed)
}

fn parse_wifi_stage_outcome(
    body: &[u8],
) -> Result<RemoteControlWifiStageOutcome, RemoteControlResponseParseError> {
    let Some((tag, rest)) = body.split_first() else {
        return Err(RemoteControlResponseParseError::Truncated);
    };
    match *tag {
        0x01 => parse_response_wifi_revision(rest).map(RemoteControlWifiStageOutcome::Staged),
        0x02 if rest.is_empty() => Ok(RemoteControlWifiStageOutcome::InvalidCredentials),
        0x02 => Err(RemoteControlResponseParseError::Malformed),
        found => Err(RemoteControlResponseParseError::UnknownWifiStageOutcome { found }),
    }
}

fn parse_wifi_transaction_status(
    body: &[u8],
) -> Result<RemoteControlWifiTransactionStatus, RemoteControlResponseParseError> {
    let Some((tag, rest)) = body.split_first() else {
        return Err(RemoteControlResponseParseError::Truncated);
    };
    match *tag {
        0x00 if rest.is_empty() => Ok(RemoteControlWifiTransactionStatus::FactoryProvisioning),
        0x01 => parse_response_wifi_revision(rest)
            .map(|revision| RemoteControlWifiTransactionStatus::Confirmed { revision }),
        0x02 => parse_response_wifi_revision(rest)
            .map(|revision| RemoteControlWifiTransactionStatus::Staged { revision }),
        0x03 => {
            let Some((revision_bytes, remaining)) = rest.split_at_checked(4) else {
                return Err(RemoteControlResponseParseError::Truncated);
            };
            let revision = parse_response_wifi_revision(revision_bytes)?;
            let [remaining] = remaining else {
                return Err(RemoteControlResponseParseError::Malformed);
            };
            let remaining = super::RemoteControlWifiConfirmationRemaining::new(*remaining)
                .ok_or(RemoteControlResponseParseError::Malformed)?;
            Ok(RemoteControlWifiTransactionStatus::AwaitingConfirmation {
                revision,
                remaining,
            })
        }
        0x04 => parse_response_wifi_revision(rest).map(|rejected_revision| {
            RemoteControlWifiTransactionStatus::RollingBack { rejected_revision }
        }),
        0x00 => Err(RemoteControlResponseParseError::Malformed),
        found => Err(RemoteControlResponseParseError::UnknownWifiTransactionStatus { found }),
    }
}

fn parse_protocol_error(
    body: &[u8],
) -> Result<RemoteControlProtocolError, RemoteControlResponseParseError> {
    let Some((kind, detail)) = body.split_first() else {
        return Err(RemoteControlResponseParseError::Truncated);
    };
    let Some(kind) = RemoteControlProtocolErrorKind::from_wire(*kind) else {
        return Err(RemoteControlResponseParseError::UnknownProtocolErrorKind { found: *kind });
    };
    match kind {
        RemoteControlProtocolErrorKind::MalformedRequest if detail.is_empty() => {
            Ok(RemoteControlProtocolError::MalformedRequest)
        }
        RemoteControlProtocolErrorKind::UnsupportedVersion => parse_error_detail(detail)
            .map(|found| RemoteControlProtocolError::UnsupportedVersion { found }),
        RemoteControlProtocolErrorKind::UnknownRequestKind => parse_error_detail(detail)
            .map(|found| RemoteControlProtocolError::UnknownRequestKind { found }),
        RemoteControlProtocolErrorKind::UnsupportedRequest
        | RemoteControlProtocolErrorKind::Busy
        | RemoteControlProtocolErrorKind::ApplyFailed
        | RemoteControlProtocolErrorKind::PersistenceFailed
        | RemoteControlProtocolErrorKind::RollbackFailed
        | RemoteControlProtocolErrorKind::InternalFailure => {
            let found = parse_error_detail(detail)?;
            let request = RemoteControlRequestKind::from_wire(found)
                .ok_or(RemoteControlResponseParseError::UnknownRequestKind { found })?;
            Ok(match kind {
                RemoteControlProtocolErrorKind::UnsupportedRequest => {
                    RemoteControlProtocolError::UnsupportedRequest { request }
                }
                RemoteControlProtocolErrorKind::Busy => {
                    RemoteControlProtocolError::Busy { request }
                }
                RemoteControlProtocolErrorKind::ApplyFailed => {
                    RemoteControlProtocolError::ApplyFailed { request }
                }
                RemoteControlProtocolErrorKind::PersistenceFailed => {
                    RemoteControlProtocolError::PersistenceFailed { request }
                }
                RemoteControlProtocolErrorKind::RollbackFailed => {
                    RemoteControlProtocolError::RollbackFailed { request }
                }
                RemoteControlProtocolErrorKind::InternalFailure => {
                    RemoteControlProtocolError::InternalFailure { request }
                }
                RemoteControlProtocolErrorKind::MalformedRequest
                | RemoteControlProtocolErrorKind::UnsupportedVersion
                | RemoteControlProtocolErrorKind::UnknownRequestKind => {
                    return Err(RemoteControlResponseParseError::Malformed);
                }
            })
        }
        RemoteControlProtocolErrorKind::MalformedRequest => {
            Err(RemoteControlResponseParseError::Malformed)
        }
    }
}

fn parse_error_detail(detail: &[u8]) -> Result<u8, RemoteControlResponseParseError> {
    let [found] = detail else {
        return Err(if detail.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    Ok(*found)
}

fn write_description(description: &RemoteControlDescription, body: &mut [u8]) {
    let Some((count, kinds)) = body.split_first_mut() else {
        return;
    };
    *count = description.available_requests.len;
    for (out, kind) in kinds.iter_mut().zip(description.available_requests.iter()) {
        *out = kind.wire_value();
    }
}

fn write_announce_self_outcome(outcome: RemoteControlAnnounceSelfOutcome, body: &mut [u8]) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_power_outcome(outcome: RemoteControlPowerOutcome, body: &mut [u8]) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_mode_outcome(outcome: RemoteControlModeOutcome, body: &mut [u8]) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_group_outcome(outcome: RemoteControlGroupOutcome, body: &mut [u8]) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_discovery_groups_replace_outcome(
    outcome: RemoteControlDiscoveryGroupsReplaceOutcome,
    body: &mut [u8],
) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_lora_outcome(outcome: RemoteControlLoRaOutcome, body: &mut [u8]) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_wifi_station_outcome(outcome: RemoteControlWifiStationOutcome, body: &mut [u8]) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_authorize_controller_outcome(
    outcome: RemoteControlAuthorizeControllerOutcome,
    body: &mut [u8],
) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_revoke_controller_outcome(outcome: RemoteControlRevokeControllerOutcome, body: &mut [u8]) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_build_version(version: RemoteControlBuildVersion, body: &mut [u8]) {
    version.write_body(body);
}

fn write_power_snapshot(snapshot: PowerSnapshot, body: &mut [u8]) {
    let _ = snapshot.write_into(body);
}

fn write_sleep_outcome(outcome: RemoteControlSleepOutcome, body: &mut [u8]) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_apply_outcome(outcome: RemoteControlApplyOutcome, body: &mut [u8]) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_wifi_stage_outcome(outcome: RemoteControlWifiStageOutcome, body: &mut [u8]) {
    let Some((tag, rest)) = body.split_first_mut() else {
        return;
    };
    match outcome {
        RemoteControlWifiStageOutcome::Staged(revision) => {
            *tag = 0x01;
            if let Some(out) = rest.get_mut(..4) {
                out.copy_from_slice(&revision.wire_bytes());
            }
        }
        RemoteControlWifiStageOutcome::InvalidCredentials => *tag = 0x02,
    }
}

fn write_wifi_transaction_status(status: RemoteControlWifiTransactionStatus, body: &mut [u8]) {
    let Some((tag, rest)) = body.split_first_mut() else {
        return;
    };
    let (tag_value, revision, remaining) = match status {
        RemoteControlWifiTransactionStatus::FactoryProvisioning => (0x00, None, None),
        RemoteControlWifiTransactionStatus::Confirmed { revision } => (0x01, Some(revision), None),
        RemoteControlWifiTransactionStatus::Staged { revision } => (0x02, Some(revision), None),
        RemoteControlWifiTransactionStatus::AwaitingConfirmation {
            revision,
            remaining,
        } => (0x03, Some(revision), Some(remaining)),
        RemoteControlWifiTransactionStatus::RollingBack { rejected_revision } => {
            (0x04, Some(rejected_revision), None)
        }
    };
    *tag = tag_value;
    if let Some(revision) = revision {
        if let Some(out) = rest.get_mut(..4) {
            out.copy_from_slice(&revision.wire_bytes());
        }
    }
    if let Some(remaining) = remaining {
        if let Some(out) = rest.get_mut(4) {
            *out = remaining.seconds();
        }
    }
}

fn write_protocol_error(error: &RemoteControlProtocolError, body: &mut [u8]) {
    let Some((kind, detail)) = body.split_first_mut() else {
        return;
    };
    *kind = error.kind().wire_value();
    if let (Some(found), Some(out)) = (error.found(), detail.first_mut()) {
        *out = found;
    }
}

#[cfg_attr(mutants, mutants::skip)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    fn parse_request_family<const N: usize>(kinds: &[RemoteControlRequestKind]) {
        let mut bytes: [u8; N] = kani::any();
        let len: usize = kani::any();
        kani::assume(len >= MESSAGE_HEADER_ENCODED_LEN && len <= bytes.len());
        bytes[0] = RemoteControlProtocolVersion::V1.wire_value();
        kani::assume(kinds.iter().any(|kind| bytes[1] == kind.wire_value()));
        let _result = RemoteControlRequest::parse(&bytes[..len]);
    }

    fn parse_response_family<const N: usize>(kinds: &[RemoteControlResponseKind]) {
        let mut bytes: [u8; N] = kani::any();
        let len: usize = kani::any();
        kani::assume(len >= MESSAGE_HEADER_ENCODED_LEN && len <= bytes.len());
        bytes[0] = RemoteControlProtocolVersion::V1.wire_value();
        kani::assume(kinds.iter().any(|kind| bytes[1] == kind.wire_value()));
        let _result = RemoteControlResponse::parse(&bytes[..len]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn request_parser_handles_discovery_and_power_family() {
        parse_request_family::<{ MESSAGE_HEADER_ENCODED_LEN + 1 }>(&[
            RemoteControlRequestKind::Describe,
            RemoteControlRequestKind::AnnounceSelf,
            RemoteControlRequestKind::DescribeBuild,
            RemoteControlRequestKind::DescribePower,
            RemoteControlRequestKind::SleepRadios,
            RemoteControlRequestKind::WakeRadios,
            RemoteControlRequestKind::InspectWifiTransaction,
        ]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn request_parser_handles_inventory_and_authorization_family() {
        parse_request_family::<{ RemoteControlRequest::MAX_ENCODED_LEN + 1 }>(&[
            RemoteControlRequestKind::InventoryInterfaces,
            RemoteControlRequestKind::InventoryInterfacePeers,
            RemoteControlRequestKind::InventoryInterfaceConfig,
            RemoteControlRequestKind::InventoryControllers,
            RemoteControlRequestKind::AuthorizeController,
            RemoteControlRequestKind::RevokeController,
        ]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn request_parser_handles_interface_configuration_family() {
        parse_request_family::<
            {
                MESSAGE_HEADER_ENCODED_LEN
                    + INTERFACE_ID_LEN
                    + 1
                    + REMOTE_CONTROL_WIFI_SSID_CAP
                    + 1
                    + REMOTE_CONTROL_WIFI_PASSWORD_CAP
                    + 1
            },
        >(&[
            RemoteControlRequestKind::SetInterfacePower,
            RemoteControlRequestKind::SetInterfaceMode,
            RemoteControlRequestKind::SetInterfaceGroup,
            RemoteControlRequestKind::SetInterfaceLoRaProfile,
            RemoteControlRequestKind::SetInterfaceWifiStation,
        ]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn request_parser_handles_discovery_group_inventory() {
        let bytes: [u8; INTERFACE_ID_LEN + 1] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        let _result = parse_inventory_interface_discovery_groups(&bytes[..len]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn request_parser_accepts_maximum_discovery_group_count() {
        const ENCODED_LEN: usize =
            INTERFACE_ID_LEN + 1 + crate::interfaces::MAX_DISCOVERY_GROUPS * 2;
        let mut bytes: [u8; ENCODED_LEN + 1] = kani::any();
        bytes[INTERFACE_ID_LEN] = crate::interfaces::MAX_DISCOVERY_GROUPS as u8;
        let mut index = INTERFACE_ID_LEN + 1;
        let mut previous = 0u8;
        let mut group = 0;
        while group < crate::interfaces::MAX_DISCOVERY_GROUPS {
            bytes[index] = 1;
            let value = bytes[index + 1];
            kani::assume(value.is_ascii());
            if group > 0 {
                kani::assume(previous < value);
            }
            previous = value;
            index += 2;
            group += 1;
        }
        assert!(parse_replace_interface_discovery_groups(&bytes[..ENCODED_LEN]).is_ok());
        assert!(parse_replace_interface_discovery_groups(&bytes).is_err());
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn request_parser_handles_desired_state_and_wifi_transaction_family() {
        parse_request_family::<{ RemoteControlRequest::MAX_ENCODED_LEN + 1 }>(&[
            RemoteControlRequestKind::SetSystemPower,
            RemoteControlRequestKind::SetGnssPower,
            RemoteControlRequestKind::SetDisplayVisibility,
            RemoteControlRequestKind::SetDisplayAutoOff,
            RemoteControlRequestKind::SetStationUplink,
            RemoteControlRequestKind::SetEspRadioMode,
            RemoteControlRequestKind::StageWifiCredentials,
            RemoteControlRequestKind::ActivateWifiCredentials,
            RemoteControlRequestKind::ConfirmWifiCredentials,
            RemoteControlRequestKind::CancelWifiCredentials,
        ]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn response_parser_handles_inventory_family() {
        parse_response_family::<{ RemoteControlResponse::MAX_ENCODED_LEN + 1 }>(&[
            RemoteControlResponseKind::Describe,
            RemoteControlResponseKind::InventoryInterfaces,
            RemoteControlResponseKind::InventoryInterfacePeers,
            RemoteControlResponseKind::InventoryInterfaceConfig,
            RemoteControlResponseKind::InventoryControllers,
        ]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn response_parser_handles_interface_configuration_family() {
        parse_response_family::<
            { MESSAGE_HEADER_ENCODED_LEN + RemoteControlProtocolError::MAX_ENCODED_BODY_LEN + 1 },
        >(&[
            RemoteControlResponseKind::SetInterfacePower,
            RemoteControlResponseKind::SetInterfaceMode,
            RemoteControlResponseKind::SetInterfaceGroup,
            RemoteControlResponseKind::SetInterfaceLoRaProfile,
            RemoteControlResponseKind::SetInterfaceWifiStation,
        ]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn response_parser_handles_discovery_group_replacement() {
        let bytes: [u8; RemoteControlDiscoveryGroupsReplaceOutcome::ENCODED_LEN + 1] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        let _result = parse_discovery_groups_replace_outcome(&bytes[..len]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn response_parser_handles_desired_state_and_wifi_transaction_family() {
        parse_response_family::<{ RemoteControlResponse::MAX_ENCODED_LEN + 1 }>(&[
            RemoteControlResponseKind::SetSystemPower,
            RemoteControlResponseKind::SetGnssPower,
            RemoteControlResponseKind::SetDisplayVisibility,
            RemoteControlResponseKind::SetDisplayAutoOff,
            RemoteControlResponseKind::SetStationUplink,
            RemoteControlResponseKind::SetEspRadioMode,
            RemoteControlResponseKind::StageWifiCredentials,
            RemoteControlResponseKind::ActivateWifiCredentials,
            RemoteControlResponseKind::ConfirmWifiCredentials,
            RemoteControlResponseKind::CancelWifiCredentials,
            RemoteControlResponseKind::InspectWifiTransaction,
        ]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn response_parser_handles_identity_power_and_error_family() {
        parse_response_family::<{ RemoteControlResponse::MAX_ENCODED_LEN + 1 }>(&[
            RemoteControlResponseKind::AnnounceSelf,
            RemoteControlResponseKind::AuthorizeController,
            RemoteControlResponseKind::RevokeController,
            RemoteControlResponseKind::DescribeBuild,
            RemoteControlResponseKind::DescribePower,
            RemoteControlResponseKind::SleepRadios,
            RemoteControlResponseKind::WakeRadios,
            RemoteControlResponseKind::ProtocolError,
        ]);
    }

    #[kani::proof]
    #[kani::unwind(40)]
    fn request_set_intersection_preserves_exact_membership() {
        const CANONICAL_REQUEST_BITS: u32 = {
            let mut bits = 0u32;
            let mut index = 0;
            while index < RemoteControlRequestKind::ALL.len() {
                bits |= 1u32 << RemoteControlRequestKind::ALL[index].wire_value();
                index += 1;
            }
            bits
        };

        let left_membership: u32 = kani::any::<u32>() & CANONICAL_REQUEST_BITS;
        let right_membership: u32 = kani::any::<u32>() & CANONICAL_REQUEST_BITS;
        let mut left = RemoteControlRequestSet::empty();
        let mut right = RemoteControlRequestSet::empty();
        left.bits[..4].copy_from_slice(&left_membership.to_le_bytes());
        left.len = left_membership.count_ones() as u8;
        right.bits[..4].copy_from_slice(&right_membership.to_le_bytes());
        right.len = right_membership.count_ones() as u8;

        let intersection = left.intersection(&right);
        let selected_index: usize = kani::any();
        kani::assume(selected_index < RemoteControlRequestKind::ALL.len());
        let selected = RemoteControlRequestKind::ALL[selected_index];
        assert_eq!(
            intersection.supports(selected),
            left.supports(selected) && right.supports(selected)
        );
        assert!(intersection.len() <= left.len());
        assert!(intersection.len() <= right.len());
    }
}
