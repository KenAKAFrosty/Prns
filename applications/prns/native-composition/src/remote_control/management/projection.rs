use super::{invalid_response, unknown_interface, Failure};
use crate::contract::*;
use personal_rns::interfaces::lora::{Modulation, RadioProfile};
use personal_rns::interfaces::subghz::{RegulatoryRegion, SubGRegion};
use personal_rns::interfaces::{
    BluetoothIndication, ConnectionState, InterfaceId, InterfaceMode, LoRaIndication,
    RadioIndication, WifiIndication,
};
use personal_rns::remote_control as core;
use prns_core::capabilities::power::{ChargingState, ExternalPowerState, PowerSnapshot};

pub(super) fn project_power(power: PowerSnapshot) -> RemoteNodePower {
    RemoteNodePower {
        battery_percent: power.battery().map(|value| value.get()),
        external_power: match power.external_power() {
            ExternalPowerState::Unknown => RemoteExternalPower::Unknown,
            ExternalPowerState::Absent => RemoteExternalPower::Absent,
            ExternalPowerState::Present {
                charging: ChargingState::Unknown,
            } => RemoteExternalPower::Present,
            ExternalPowerState::Present {
                charging: ChargingState::Charging,
            } => RemoteExternalPower::Charging,
            ExternalPowerState::Present {
                charging: ChargingState::Idle,
            } => RemoteExternalPower::Idle,
        },
    }
}

pub(super) fn project_interfaces(
    inventory: core::RemoteControlInterfaceInventory,
    after: Option<InterfaceId>,
) -> Result<RemoteInterfacePage, Failure> {
    let next = match inventory.continuation() {
        core::RemoteControlInterfaceContinuation::Complete => None,
        core::RemoteControlInterfaceContinuation::More(cursor) => {
            Some(cursor.id().as_bytes().to_vec())
        }
    };
    // Wire parsing validates within-page ordering and continuation ownership;
    // the application must also reject pages that go backwards from its cursor.
    if after.is_some_and(|after| {
        inventory
            .entries()
            .first()
            .is_some_and(|first| first.id.as_bytes() <= after.as_bytes())
    }) {
        return Err(invalid_response());
    }
    Ok(RemoteInterfacePage {
        entries: inventory
            .entries()
            .iter()
            .map(|entry| RemoteInterfaceEntry {
                interface_id: entry.id.as_bytes().to_vec(),
                kind: entry.kind.name().to_owned(),
                mode: project_mode(entry.mode),
                connection: project_connection(entry.connection),
                enabled: entry.enabled,
                tx_bytes: entry.tx_bytes,
                rx_bytes: entry.rx_bytes,
                links: entry.links,
                rate_bytes_per_sec: entry.rate_bytes_per_sec,
            })
            .collect(),
        next,
    })
}

pub(super) fn project_config(
    outcome: core::RemoteControlInterfaceConfigOutcome,
) -> RemoteInterfaceConfiguration {
    match outcome {
        core::RemoteControlInterfaceConfigOutcome::UnknownInterface => {
            RemoteInterfaceConfiguration::UnknownInterface
        }
        core::RemoteControlInterfaceConfigOutcome::Card(card) => {
            RemoteInterfaceConfiguration::Available {
                card: RemoteInterfaceCard {
                    name: card.name.to_string(),
                    group: card.group.to_string(),
                    configuration: card.config.to_string(),
                    failure: card.failure.to_string(),
                    destinations: card.destinations,
                    transported_links: card.transported_links,
                    lora_profile: RadioProfile::parse_inventory_config(card.config.as_str())
                        .map(project_lora),
                },
            }
        }
    }
}

pub(super) fn project_groups(
    outcome: core::RemoteControlDiscoveryGroupsInventoryOutcome,
) -> RemoteDiscoveryGroups {
    match outcome {
        core::RemoteControlDiscoveryGroupsInventoryOutcome::Groups(groups) => {
            RemoteDiscoveryGroups::Available {
                groups: groups
                    .groups()
                    .iter()
                    .map(|group| group.as_str().to_owned())
                    .collect(),
            }
        }
        core::RemoteControlDiscoveryGroupsInventoryOutcome::UnknownInterface => {
            RemoteDiscoveryGroups::UnknownInterface
        }
        core::RemoteControlDiscoveryGroupsInventoryOutcome::Unsupported => {
            RemoteDiscoveryGroups::Unavailable
        }
    }
}

pub(super) fn project_peers(
    outcome: core::RemoteControlInterfacePeersOutcome,
    expected: InterfaceId,
    after: Option<InterfaceId>,
) -> Result<RemotePeerPage, Failure> {
    let core::RemoteControlInterfacePeersOutcome::Page(page) = outcome else {
        return Err(unknown_interface());
    };
    if page.id != expected
        || after.is_some_and(|after| {
            page.peers
                .first()
                .is_some_and(|first| first.id.as_bytes() <= after.as_bytes())
        })
    {
        return Err(invalid_response());
    }
    let next = match page.continuation() {
        core::RemoteControlPeerContinuation::Complete => None,
        core::RemoteControlPeerContinuation::More(cursor) => Some(cursor.id().as_bytes().to_vec()),
    };
    Ok(RemotePeerPage {
        interface_id: page.id.as_bytes().to_vec(),
        entries: page
            .peers
            .iter()
            .map(|peer| RemotePeerEntry {
                peer_id: peer.id.as_bytes().to_vec(),
                connection: project_connection(peer.connection),
                tx_bytes: peer.tx_bytes,
                rx_bytes: peer.rx_bytes,
                links: peer.links,
                destinations: peer.destinations,
                rate_bytes_per_sec: peer.rate_bytes_per_sec,
                radio: project_radio(peer.radio),
                details: peer.details.to_string(),
            })
            .collect(),
        next,
    })
}

pub(super) fn project_connection(state: ConnectionState) -> RemoteConnectionState {
    match state {
        ConnectionState::Initializing => RemoteConnectionState::Initializing,
        ConnectionState::Connected => RemoteConnectionState::Connected,
        ConnectionState::Degraded => RemoteConnectionState::Degraded,
        ConnectionState::Reconnecting => RemoteConnectionState::Reconnecting,
        ConnectionState::Failed => RemoteConnectionState::Failed,
        ConnectionState::Disconnected => RemoteConnectionState::Disconnected,
        ConnectionState::Disabled => RemoteConnectionState::Disabled,
        ConnectionState::Unknown => RemoteConnectionState::Unknown,
    }
}

fn project_mode(mode: InterfaceMode) -> RemoteInterfaceMode {
    match mode {
        InterfaceMode::Full => RemoteInterfaceMode::Full,
        InterfaceMode::PointToPoint => RemoteInterfaceMode::PointToPoint,
        InterfaceMode::AccessPoint => RemoteInterfaceMode::AccessPoint,
        InterfaceMode::Roaming => RemoteInterfaceMode::Roaming,
        InterfaceMode::Boundary => RemoteInterfaceMode::Boundary,
        InterfaceMode::Gateway => RemoteInterfaceMode::Gateway,
        InterfaceMode::Internal => RemoteInterfaceMode::Internal,
    }
}

fn project_radio(radio: RadioIndication) -> RemotePeerRadio {
    match radio {
        RadioIndication::NotRadio => RemotePeerRadio::NotRadio,
        RadioIndication::Bluetooth(BluetoothIndication::Pending) => RemotePeerRadio::Pending {
            family: RemoteRadioFamily::Bluetooth,
        },
        RadioIndication::Bluetooth(BluetoothIndication::Rssi(rssi)) => RemotePeerRadio::Measured {
            family: RemoteRadioFamily::Bluetooth,
            rssi_dbm: rssi.get(),
            snr_quarter_db: None,
            quality_tenths_percent: None,
        },
        RadioIndication::Wifi(WifiIndication::Pending) => RemotePeerRadio::Pending {
            family: RemoteRadioFamily::Wifi,
        },
        RadioIndication::Wifi(WifiIndication::Unavailable) => RemotePeerRadio::Unavailable {
            family: RemoteRadioFamily::Wifi,
        },
        RadioIndication::Wifi(WifiIndication::Rssi(rssi)) => RemotePeerRadio::Measured {
            family: RemoteRadioFamily::Wifi,
            rssi_dbm: rssi.get(),
            snr_quarter_db: None,
            quality_tenths_percent: None,
        },
        RadioIndication::LoRa(LoRaIndication::Pending) => RemotePeerRadio::Pending {
            family: RemoteRadioFamily::LoRa,
        },
        RadioIndication::LoRa(LoRaIndication::Sample { rssi, snr, quality }) => {
            RemotePeerRadio::Measured {
                family: RemoteRadioFamily::LoRa,
                rssi_dbm: rssi.get(),
                snr_quarter_db: snr.map(|value| value.quarters()),
                quality_tenths_percent: quality.map(|value| value.tenths_percent()),
            }
        }
    }
}

pub(super) fn project_lora(profile: RadioProfile) -> RemoteLoRaProfile {
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation();
    let region = match profile.region() {
        SubGRegion::Custom => RemoteLoRaRegion::Custom,
        SubGRegion::Regulated(region) => match region {
            RegulatoryRegion::Us915 => RemoteLoRaRegion::Us915,
            RegulatoryRegion::Au915 => RemoteLoRaRegion::Au915,
            RegulatoryRegion::Eu433 => RemoteLoRaRegion::Eu433,
            RegulatoryRegion::Eu865 => RemoteLoRaRegion::Eu865,
            RegulatoryRegion::Eu868 => RemoteLoRaRegion::Eu868,
            RegulatoryRegion::Eu869 => RemoteLoRaRegion::Eu869,
            RegulatoryRegion::As923 => RemoteLoRaRegion::As923,
            RegulatoryRegion::In865 => RemoteLoRaRegion::In865,
            RegulatoryRegion::Cn470 => RemoteLoRaRegion::Cn470,
            RegulatoryRegion::Kr920 => RemoteLoRaRegion::Kr920,
            RegulatoryRegion::Jp920 => RemoteLoRaRegion::Jp920,
        },
    };
    RemoteLoRaProfile {
        region,
        frequency_hz: profile.frequency().hz(),
        spreading_factor: spreading_factor as u8,
        bandwidth_hz: bandwidth.hz(),
        coding_rate: coding_rate.denominator(),
        tx_power_dbm: profile.tx_power().dbm(),
        preamble_symbols: profile.preamble().count(),
    }
}
