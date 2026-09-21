use personal_rns::interfaces::lora::{
    CodingRate, LoraBandwidth, Modulation, PreambleSymbols, RadioProfile, SpreadingFactor,
};
use personal_rns::interfaces::subghz::{Frequency, RegulatoryRegion, SubGRegion, TxPower};
use personal_rns::interfaces::{
    DiscoveryGroupId, DiscoveryGroupSet, InterfaceId, InterfaceMode, INTERFACE_ID_LEN,
};
use personal_rns::remote_control as core;

use crate::contract::*;

pub(super) enum PreparedQuery {
    Overview,
    Interfaces(Option<InterfaceId>),
    Interface(InterfaceId),
    Peers(InterfaceId, Option<InterfaceId>),
}

pub(super) fn interface_id(bytes: &[u8]) -> Result<InterfaceId, String> {
    <[u8; INTERFACE_ID_LEN]>::try_from(bytes)
        .map(InterfaceId::new)
        .map_err(|_| "Choose a valid node connection.".to_owned())
}

pub(super) fn prepare_query(query: RemoteNodeQuery) -> Result<PreparedQuery, String> {
    let cursor = |value: Option<Vec<u8>>| value.as_deref().map(interface_id).transpose();
    Ok(match query {
        RemoteNodeQuery::Overview => PreparedQuery::Overview,
        RemoteNodeQuery::Interfaces { after } => PreparedQuery::Interfaces(cursor(after)?),
        RemoteNodeQuery::Interface { interface_id: id } => {
            PreparedQuery::Interface(interface_id(&id)?)
        }
        RemoteNodeQuery::Peers {
            interface_id: id,
            after,
        } => PreparedQuery::Peers(interface_id(&id)?, cursor(after)?),
    })
}

pub(super) enum PreparedChange {
    InterfacePower(InterfaceId, core::RemoteControlInterfacePower),
    InterfaceMode(InterfaceId, InterfaceMode),
    InterfaceGroup(InterfaceId, core::RemoteControlInterfaceGroup),
    InterfaceLoRa(InterfaceId, core::RemoteControlLoRaProfile),
    DiscoveryGroups(InterfaceId, core::RemoteControlDiscoveryGroups),
    GnssPower(core::RemoteControlGnssPower),
    DisplayVisibility(core::RemoteControlDisplayVisibility),
    DisplayAutoOff(core::RemoteControlDisplayAutoOff),
    SystemPower(core::RemoteControlSystemPower),
    StationUplink(InterfaceId, core::RemoteControlStationUplink),
    RadioMode(core::RemoteControlEspRadioMode),
    SleepRadios,
    WakeRadios,
}

impl PreparedChange {
    pub(super) fn kind(&self) -> core::RemoteControlRequestKind {
        use core::RemoteControlRequestKind as Kind;
        match self {
            Self::InterfacePower(..) => Kind::SetInterfacePower,
            Self::InterfaceMode(..) => Kind::SetInterfaceMode,
            Self::InterfaceGroup(..) => Kind::SetInterfaceGroup,
            Self::InterfaceLoRa(..) => Kind::SetInterfaceLoRaProfile,
            Self::DiscoveryGroups(..) => Kind::ReplaceInterfaceDiscoveryGroups,
            Self::GnssPower(..) => Kind::SetGnssPower,
            Self::DisplayVisibility(..) => Kind::SetDisplayVisibility,
            Self::DisplayAutoOff(..) => Kind::SetDisplayAutoOff,
            Self::SystemPower(..) => Kind::SetSystemPower,
            Self::StationUplink(..) => Kind::SetStationUplink,
            Self::RadioMode(..) => Kind::SetEspRadioMode,
            Self::SleepRadios => Kind::SleepRadios,
            Self::WakeRadios => Kind::WakeRadios,
        }
    }
}

pub(super) fn prepare_change(change: &RemoteNodeChange) -> Result<PreparedChange, String> {
    use core::*;
    Ok(match change {
        RemoteNodeChange::InterfacePower {
            interface_id: id,
            enabled,
        } => PreparedChange::InterfacePower(
            interface_id(id)?,
            if *enabled {
                RemoteControlInterfacePower::On
            } else {
                RemoteControlInterfacePower::Off
            },
        ),
        RemoteNodeChange::InterfaceMode {
            interface_id: id,
            mode,
        } => PreparedChange::InterfaceMode(interface_id(id)?, lower_mode(*mode)),
        RemoteNodeChange::InterfaceGroup {
            interface_id: id,
            group,
        } => PreparedChange::InterfaceGroup(
            interface_id(id)?,
            RemoteControlInterfaceGroup::parse(group)
                .ok_or_else(|| "Enter a group name between 1 and 32 UTF-8 bytes.".to_owned())?,
        ),
        RemoteNodeChange::InterfaceLoRa {
            interface_id: id,
            profile,
        } => PreparedChange::InterfaceLoRa(interface_id(id)?, lower_lora(profile)?),
        RemoteNodeChange::DiscoveryGroups {
            interface_id: id,
            groups,
        } => {
            // Check the bound before allocating/parsing attacker-controlled input.
            if groups.is_empty() || groups.len() > personal_rns::interfaces::MAX_DISCOVERY_GROUPS {
                return Err("Choose between one and four discovery groups.".to_owned());
            }
            let parsed = groups
                .iter()
                .map(|group| DiscoveryGroupId::parse(group))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| {
                    "Each group name must contain between 1 and 32 UTF-8 bytes.".to_owned()
                })?;
            let groups = DiscoveryGroupSet::try_from_slice(&parsed)
                .map_err(|_| "Discovery groups must be distinct.".to_owned())?;
            PreparedChange::DiscoveryGroups(
                interface_id(id)?,
                RemoteControlDiscoveryGroups::new(groups),
            )
        }
        RemoteNodeChange::GnssPower { enabled } => PreparedChange::GnssPower(if *enabled {
            RemoteControlGnssPower::On
        } else {
            RemoteControlGnssPower::Off
        }),
        RemoteNodeChange::DisplayVisibility { visible } => {
            PreparedChange::DisplayVisibility(if *visible {
                RemoteControlDisplayVisibility::Visible
            } else {
                RemoteControlDisplayVisibility::Hidden
            })
        }
        RemoteNodeChange::DisplayAutoOff { enabled } => {
            PreparedChange::DisplayAutoOff(if *enabled {
                RemoteControlDisplayAutoOff::Enabled
            } else {
                RemoteControlDisplayAutoOff::Disabled
            })
        }
        RemoteNodeChange::SystemPower { awake } => PreparedChange::SystemPower(if *awake {
            RemoteControlSystemPower::Awake
        } else {
            RemoteControlSystemPower::Asleep
        }),
        RemoteNodeChange::StationUplink {
            interface_id: id,
            enabled,
        } => PreparedChange::StationUplink(
            interface_id(id)?,
            if *enabled {
                RemoteControlStationUplink::Enabled
            } else {
                RemoteControlStationUplink::Disabled
            },
        ),
        RemoteNodeChange::RadioMode { mode } => PreparedChange::RadioMode(match mode {
            RemoteRadioMode::Bluetooth => RemoteControlEspRadioMode::Bluetooth,
            RemoteRadioMode::AccessPoint => RemoteControlEspRadioMode::AccessPoint,
        }),
        RemoteNodeChange::SleepRadios => PreparedChange::SleepRadios,
        RemoteNodeChange::WakeRadios => PreparedChange::WakeRadios,
    })
}

pub(super) fn lower_mode(mode: RemoteInterfaceMode) -> InterfaceMode {
    match mode {
        RemoteInterfaceMode::Full => InterfaceMode::Full,
        RemoteInterfaceMode::PointToPoint => InterfaceMode::PointToPoint,
        RemoteInterfaceMode::AccessPoint => InterfaceMode::AccessPoint,
        RemoteInterfaceMode::Roaming => InterfaceMode::Roaming,
        RemoteInterfaceMode::Boundary => InterfaceMode::Boundary,
        RemoteInterfaceMode::Gateway => InterfaceMode::Gateway,
        RemoteInterfaceMode::Internal => InterfaceMode::Internal,
    }
}

pub(super) fn lower_region(region: RemoteLoRaRegion) -> SubGRegion {
    SubGRegion::Regulated(match region {
        RemoteLoRaRegion::Us915 => RegulatoryRegion::Us915,
        RemoteLoRaRegion::Au915 => RegulatoryRegion::Au915,
        RemoteLoRaRegion::Eu433 => RegulatoryRegion::Eu433,
        RemoteLoRaRegion::Eu865 => RegulatoryRegion::Eu865,
        RemoteLoRaRegion::Eu868 => RegulatoryRegion::Eu868,
        RemoteLoRaRegion::Eu869 => RegulatoryRegion::Eu869,
        RemoteLoRaRegion::As923 => RegulatoryRegion::As923,
        RemoteLoRaRegion::In865 => RegulatoryRegion::In865,
        RemoteLoRaRegion::Cn470 => RegulatoryRegion::Cn470,
        RemoteLoRaRegion::Kr920 => RegulatoryRegion::Kr920,
        RemoteLoRaRegion::Jp920 => RegulatoryRegion::Jp920,
        RemoteLoRaRegion::Custom => return SubGRegion::Custom,
    })
}

pub(super) fn lower_lora(
    profile: &RemoteLoRaProfile,
) -> Result<core::RemoteControlLoRaProfile, String> {
    let spreading_factor = match profile.spreading_factor {
        5 => SpreadingFactor::Sf5,
        6 => SpreadingFactor::Sf6,
        7 => SpreadingFactor::Sf7,
        8 => SpreadingFactor::Sf8,
        9 => SpreadingFactor::Sf9,
        10 => SpreadingFactor::Sf10,
        11 => SpreadingFactor::Sf11,
        12 => SpreadingFactor::Sf12,
        _ => return Err("Choose a spreading factor between 5 and 12.".to_owned()),
    };
    let bandwidth = match profile.bandwidth_hz {
        125_000 => LoraBandwidth::Bw125kHz,
        250_000 => LoraBandwidth::Bw250kHz,
        500_000 => LoraBandwidth::Bw500kHz,
        _ => return Err("Choose a bandwidth of 125, 250 or 500 kHz.".to_owned()),
    };
    let coding_rate = CodingRate::from_denominator(profile.coding_rate)
        .ok_or_else(|| "Choose a coding rate between 4/5 and 4/8.".to_owned())?;
    let validated = RadioProfile::new(
        lower_region(profile.region),
        Frequency::new(profile.frequency_hz),
        Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        },
        TxPower::new(profile.tx_power_dbm),
        PreambleSymbols::new(profile.preamble_symbols),
    )
    .map_err(|_| {
        "The frequency, power or preamble is not valid for the selected region.".to_owned()
    })?;
    core::RemoteControlLoRaProfile::from_profile(validated)
        .ok_or_else(|| "The radio profile cannot be sent to this node.".to_owned())
}
