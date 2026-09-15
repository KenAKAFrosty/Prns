use crate::capabilities::power::PowerSnapshot;
use crate::identity::{IdentityHash, IDENTITY_PUBLIC_KEY_LEN};
use crate::interfaces::{InterfaceId, InterfaceMode, INTERFACE_ID_LEN};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

use super::inventory::{
    parse_controller_public_keys, RemoteControlAuthorizeControllerOutcome,
    RemoteControlBuildVersion, RemoteControlControllerInventory, RemoteControlGroupOutcome,
    RemoteControlInterfaceConfigOutcome, RemoteControlInterfaceGroup,
    RemoteControlInterfaceInventory, RemoteControlInterfacePeersOutcome,
    RemoteControlInterfacePower, RemoteControlLoRaOutcome, RemoteControlLoRaProfile,
    RemoteControlModeOutcome, RemoteControlPowerOutcome, RemoteControlRevokeControllerOutcome,
    RemoteControlSleepOutcome, RemoteControlWifiStation, RemoteControlWifiStationOutcome,
    REMOTE_CONTROL_BUILD_VERSION_CAP, REMOTE_CONTROL_INTERFACE_CONFIG_CAP,
    REMOTE_CONTROL_INTERFACE_ENTRY_ENCODED_LEN, REMOTE_CONTROL_INTERFACE_GROUP_CAP,
    REMOTE_CONTROL_INTERFACE_INVENTORY_CAP,
    REMOTE_CONTROL_INTERFACE_INVENTORY_TRAILER_MAX_ENCODED_LEN, REMOTE_CONTROL_WIFI_PASSWORD_CAP,
    REMOTE_CONTROL_WIFI_SSID_CAP,
};
use super::RemoteControlControllerIdentity;

const MESSAGE_HEADER_ENCODED_LEN: usize = 2;
const DESCRIPTION_COUNT_ENCODED_LEN: usize = 1;
const PROTOCOL_ERROR_KIND_ENCODED_LEN: usize = 1;
const PROTOCOL_ERROR_DETAIL_ENCODED_LEN: usize = 1;
const REQUEST_KIND_BITMAP_LEN: usize = 32;

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
                    .saturating_add(REMOTE_CONTROL_INTERFACE_INVENTORY_TRAILER_MAX_ENCODED_LEN),
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
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlRequest {
    Describe,
    AnnounceSelf,
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
    DescribeBuild,
    DescribePower,
    SleepRadios,
    WakeRadios,
}

impl RemoteControlRequest {
    pub const MAX_ENCODED_LEN: usize = MESSAGE_HEADER_ENCODED_LEN.saturating_add(
        INTERFACE_ID_LEN
            .saturating_add(1)
            .saturating_add(REMOTE_CONTROL_WIFI_SSID_CAP)
            .saturating_add(1)
            .saturating_add(REMOTE_CONTROL_WIFI_PASSWORD_CAP),
    );

    #[must_use]
    pub const fn kind(self) -> RemoteControlRequestKind {
        match self {
            Self::Describe => RemoteControlRequestKind::Describe,
            Self::AnnounceSelf => RemoteControlRequestKind::AnnounceSelf,
            Self::InventoryInterfaces => RemoteControlRequestKind::InventoryInterfaces,
            Self::SetInterfacePower { .. } => RemoteControlRequestKind::SetInterfacePower,
            Self::SetInterfaceMode { .. } => RemoteControlRequestKind::SetInterfaceMode,
            Self::SetInterfaceGroup { .. } => RemoteControlRequestKind::SetInterfaceGroup,
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
            Self::InventoryControllers => RemoteControlRequestKind::InventoryControllers,
            Self::AuthorizeController { .. } => RemoteControlRequestKind::AuthorizeController,
            Self::RevokeController { .. } => RemoteControlRequestKind::RevokeController,
            Self::DescribeBuild => RemoteControlRequestKind::DescribeBuild,
            Self::DescribePower => RemoteControlRequestKind::DescribePower,
            Self::SleepRadios => RemoteControlRequestKind::SleepRadios,
            Self::WakeRadios => RemoteControlRequestKind::WakeRadios,
        }
    }

    #[must_use]
    pub const fn encoded_len(self) -> usize {
        match self {
            Self::Describe
            | Self::AnnounceSelf
            | Self::InventoryInterfaces
            | Self::InventoryControllers
            | Self::DescribeBuild
            | Self::DescribePower
            | Self::SleepRadios
            | Self::WakeRadios => MESSAGE_HEADER_ENCODED_LEN,
            Self::SetInterfacePower { .. }
            | Self::SetInterfaceMode { .. }
            | Self::InventoryInterfacePeers { .. } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(INTERFACE_ID_LEN.saturating_add(1))
            }
            Self::InventoryInterfaceConfig { .. } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(INTERFACE_ID_LEN)
            }
            Self::SetInterfaceGroup { group, .. } => MESSAGE_HEADER_ENCODED_LEN
                .saturating_add(INTERFACE_ID_LEN.saturating_add(group.encoded_body_len())),
            Self::SetInterfaceLoRaProfile { profile, .. } => MESSAGE_HEADER_ENCODED_LEN
                .saturating_add(INTERFACE_ID_LEN.saturating_add(profile.encoded_body_len())),
            Self::SetInterfaceWifiStation { station, .. } => MESSAGE_HEADER_ENCODED_LEN
                .saturating_add(INTERFACE_ID_LEN.saturating_add(station.encoded_body_len())),
            Self::AuthorizeController { .. } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(IDENTITY_PUBLIC_KEY_LEN)
            }
            Self::RevokeController { .. } => {
                MESSAGE_HEADER_ENCODED_LEN.saturating_add(TRUNCATED_HASH_BYTE_LEN)
            }
        }
    }

    #[must_use]
    pub const fn maximum_response_encoded_len(self) -> usize {
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
            RemoteControlRequestKind::InventoryInterfaces if body.is_empty() => {
                Ok(Self::InventoryInterfaces)
            }
            RemoteControlRequestKind::InventoryControllers if body.is_empty() => {
                Ok(Self::InventoryControllers)
            }
            RemoteControlRequestKind::AuthorizeController => parse_authorize_controller(body),
            RemoteControlRequestKind::RevokeController => parse_revoke_controller(body),
            RemoteControlRequestKind::DescribeBuild if body.is_empty() => Ok(Self::DescribeBuild),
            RemoteControlRequestKind::DescribePower if body.is_empty() => Ok(Self::DescribePower),
            RemoteControlRequestKind::SleepRadios if body.is_empty() => Ok(Self::SleepRadios),
            RemoteControlRequestKind::WakeRadios if body.is_empty() => Ok(Self::WakeRadios),
            RemoteControlRequestKind::SetInterfacePower => parse_set_interface_power(body),
            RemoteControlRequestKind::SetInterfaceMode => parse_set_interface_mode(body),
            RemoteControlRequestKind::SetInterfaceGroup => parse_set_interface_group(body),
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
            | RemoteControlRequestKind::InventoryInterfaces
            | RemoteControlRequestKind::InventoryControllers
            | RemoteControlRequestKind::DescribeBuild
            | RemoteControlRequestKind::DescribePower
            | RemoteControlRequestKind::SleepRadios
            | RemoteControlRequestKind::WakeRadios => {
                Err(RemoteControlRequestParseError::Malformed)
            }
        }
    }

    pub fn write_into(self, out: &mut [u8]) -> Result<usize, RemoteControlMessageWriteError> {
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
            | Self::InventoryInterfaces
            | Self::InventoryControllers
            | Self::DescribeBuild
            | Self::DescribePower
            | Self::SleepRadios
            | Self::WakeRadios => {}
            Self::SetInterfacePower { id, power } => {
                write_interface_id_and_byte(body, id, power.wire_value())?;
            }
            Self::SetInterfaceMode { id, mode } => {
                write_interface_id_and_byte(body, id, mode.wire_value())?;
            }
            Self::SetInterfaceGroup { id, group } => {
                write_interface_id_and_group(body, id, group)?;
            }
            Self::InventoryInterfacePeers { id, offset } => {
                write_interface_id_and_byte(body, id, offset)?;
            }
            Self::InventoryInterfaceConfig { id } => {
                write_interface_id(body, id)?;
            }
            Self::SetInterfaceLoRaProfile { id, profile } => {
                write_interface_id_and_lora_profile(body, id, profile)?;
            }
            Self::SetInterfaceWifiStation { id, station } => {
                write_interface_id_and_wifi_station(body, id, station)?;
            }
            Self::AuthorizeController { controller } => {
                write_controller_public_keys(body, controller)?;
            }
            Self::RevokeController { hash } => {
                write_controller_hash(body, hash)?;
            }
        }
        Ok(encoded_len)
    }
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
    let Some((ssid_len, rest)) = rest.split_first() else {
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
    let Some(station) = RemoteControlWifiStation::parse(ssid, password) else {
        return Err(RemoteControlRequestParseError::Malformed);
    };
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    Ok(RemoteControlRequest::SetInterfaceWifiStation {
        id: InterfaceId::new(id),
        station,
    })
}

fn parse_authorize_controller(
    body: &[u8],
) -> Result<RemoteControlRequest, RemoteControlRequestParseError> {
    let Some(controller) = parse_controller_public_keys(body) else {
        return Err(if body.is_empty() {
            RemoteControlRequestParseError::Truncated
        } else {
            RemoteControlRequestParseError::Malformed
        });
    };
    if body.len() != IDENTITY_PUBLIC_KEY_LEN {
        return Err(RemoteControlRequestParseError::Malformed);
    }
    Ok(RemoteControlRequest::AuthorizeController { controller })
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
    let Some((id_bytes, offset_byte)) = bytes.split_at_checked(INTERFACE_ID_LEN) else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let Some(offset) = offset_byte.first().copied() else {
        return Err(RemoteControlRequestParseError::Truncated);
    };
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    Ok(RemoteControlRequest::InventoryInterfacePeers {
        id: InterfaceId::new(id),
        offset,
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
    station: RemoteControlWifiStation,
) -> Result<(), RemoteControlMessageWriteError> {
    let Some((id_out, rest)) = body.split_at_mut_checked(INTERFACE_ID_LEN) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    id_out.copy_from_slice(id.as_bytes());
    let Some((ssid_len_out, rest)) = rest.split_first_mut() else {
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
    pub fn len(&self) -> usize {
        usize::from(self.len)
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
        let mut intersection = Self::empty();
        for kind in self.iter() {
            if other.supports(kind) {
                let _inserted = intersection.insert(kind);
            }
        }
        intersection
    }

    /// Pairing freezes the request set the node offered that day. A grant that
    /// already allowed live interface edits also receives later additive edit
    /// kinds so an existing pair still reaches them.
    #[must_use]
    pub fn with_current_operator_edits(self) -> Self {
        let mut requests = self;
        if requests.supports(RemoteControlRequestKind::InventoryInterfaces) {
            let _peers = requests.insert(RemoteControlRequestKind::InventoryInterfacePeers);
            let _config = requests.insert(RemoteControlRequestKind::InventoryInterfaceConfig);
        }
        if requests.supports(RemoteControlRequestKind::InventoryInterfaces)
            && requests.supports(RemoteControlRequestKind::SetInterfacePower)
            && requests.supports(RemoteControlRequestKind::SetInterfaceMode)
        {
            let _inserted = requests.insert(RemoteControlRequestKind::SetInterfaceGroup);
            let _lora = requests.insert(RemoteControlRequestKind::SetInterfaceLoRaProfile);
            let _wifi = requests.insert(RemoteControlRequestKind::SetInterfaceWifiStation);
            let _inventory = requests.insert(RemoteControlRequestKind::InventoryControllers);
            let _authorize = requests.insert(RemoteControlRequestKind::AuthorizeController);
            let _revoke = requests.insert(RemoteControlRequestKind::RevokeController);
        }
        if requests.supports(RemoteControlRequestKind::Describe)
            || requests.supports(RemoteControlRequestKind::DescribeBuild)
        {
            let _build = requests.insert(RemoteControlRequestKind::DescribeBuild);
            let _power = requests.insert(RemoteControlRequestKind::DescribePower);
        }
        requests
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
        }
    }

    const fn encoded_body_len(self) -> usize {
        match self {
            Self::MalformedRequest => PROTOCOL_ERROR_KIND_ENCODED_LEN,
            Self::UnsupportedVersion { .. } | Self::UnknownRequestKind { .. } => {
                Self::MAX_ENCODED_BODY_LEN
            }
        }
    }

    const fn found(self) -> Option<u8> {
        match self {
            Self::MalformedRequest => None,
            Self::UnsupportedVersion { found } | Self::UnknownRequestKind { found } => Some(found),
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
    ProtocolError(RemoteControlProtocolError),
}

impl RemoteControlResponse {
    pub const MAX_ENCODED_LEN: usize = MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
        DESCRIPTION_COUNT_ENCODED_LEN.saturating_add(RemoteControlRequestKind::ALL.len()),
        maximum(
            1usize
                .saturating_add(
                    REMOTE_CONTROL_INTERFACE_INVENTORY_CAP
                        .saturating_mul(REMOTE_CONTROL_INTERFACE_ENTRY_ENCODED_LEN),
                )
                .saturating_add(REMOTE_CONTROL_INTERFACE_INVENTORY_TRAILER_MAX_ENCODED_LEN),
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
    UnknownProtocolErrorKind { found: u8 },
    UnknownRequestKind { found: u8 },
    UnknownInterfaceKind { found: u8 },
    UnknownInterfaceMode { found: u8 },
    NonCanonicalRequestSet,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlMessageWriteError {
    BufferTooShort,
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

    #[kani::proof]
    #[kani::unwind(4)]
    fn remote_control_request_parse_terminates_for_every_distinct_wire_shape() {
        let bytes: [u8; RemoteControlRequest::MAX_ENCODED_LEN + 1] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        let _result = RemoteControlRequest::parse(&bytes[..len]);
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn remote_control_response_parse_terminates_for_every_distinct_wire_shape() {
        let bytes: [u8; RemoteControlResponse::MAX_ENCODED_LEN + 1] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        let _result = RemoteControlResponse::parse(&bytes[..len]);
    }

    #[kani::proof]
    #[kani::unwind(4)]
    fn request_set_intersection_preserves_exact_membership() {
        let mut left = RemoteControlRequestSet::empty();
        let mut right = RemoteControlRequestSet::empty();
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::Describe);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::AnnounceSelf);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::InventoryInterfaces);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::SetInterfacePower);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::SleepRadios);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::WakeRadios);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::SetInterfaceMode);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::SetInterfaceGroup);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::InventoryInterfacePeers);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::InventoryInterfaceConfig);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::SetInterfaceLoRaProfile);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::DescribeBuild);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::SetInterfaceWifiStation);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::InventoryControllers);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::AuthorizeController);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::RevokeController);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::Describe);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::AnnounceSelf);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::InventoryInterfaces);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::SetInterfacePower);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::SleepRadios);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::WakeRadios);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::SetInterfaceMode);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::SetInterfaceGroup);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::InventoryInterfacePeers);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::InventoryInterfaceConfig);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::SetInterfaceLoRaProfile);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::DescribeBuild);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::SetInterfaceWifiStation);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::InventoryControllers);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::AuthorizeController);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::RevokeController);
        }
        let intersection = left.intersection(&right);
        for kind in RemoteControlRequestKind::ALL {
            assert_eq!(
                intersection.supports(kind),
                left.supports(kind) && right.supports(kind)
            );
        }
        assert!(intersection.len() <= left.len());
        assert!(intersection.len() <= right.len());
    }
}
