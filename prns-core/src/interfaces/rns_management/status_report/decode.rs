use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use rmp::Marker;

use crate::identity::IdentityHash;
use crate::interfaces::rns_management::message_pack::{MessagePackInteger, MessagePackReader};
use crate::interfaces::rns_management::wire_names::{interface, transport};
use crate::wire::DestinationHash;

use super::{
    RnsFleetPeerReport, RnsInterfaceMode, RnsInterfaceStatsReport, RnsInterfaceStatusReport,
    RnsOptionalField, RnsRemoteInterfaceStatsReport,
};

const MAXIMUM_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsStatsFieldScope {
    Report,
    Interface(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsStatsFieldPath {
    pub scope: RnsStatsFieldScope,
    pub field: String,
}

impl RnsStatsFieldPath {
    pub fn report(field: impl Into<String>) -> Self {
        Self {
            scope: RnsStatsFieldScope::Report,
            field: field.into(),
        }
    }

    pub fn interface(index: usize, field: impl Into<String>) -> Self {
        Self {
            scope: RnsStatsFieldScope::Interface(index),
            field: field.into(),
        }
    }
}

impl fmt::Display for RnsStatsFieldPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.scope {
            RnsStatsFieldScope::Report => formatter.write_str(&self.field),
            RnsStatsFieldScope::Interface(index) => {
                write!(formatter, "interfaces[{index}].{}", self.field)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RnsInterfaceStatsDecodeError {
    InvalidMessagePack,
    ExpectedReportMap,
    ExpectedRemoteResponseArray,
    EmptyRemoteResponse,
    ExpectedInterfacesArray,
    ExpectedInterfaceMap {
        index: usize,
    },
    InvalidMapKey {
        scope: RnsStatsFieldScope,
    },
    MissingField(RnsStatsFieldPath),
    DuplicateField(RnsStatsFieldPath),
    InvalidFieldType(RnsStatsFieldPath),
    InvalidHashLength {
        path: RnsStatsFieldPath,
        expected: usize,
        actual: usize,
    },
    AllocationFailed {
        entries: usize,
    },
    TrailingData,
}

impl fmt::Display for RnsInterfaceStatsDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMessagePack => formatter.write_str("invalid MessagePack status reply"),
            Self::ExpectedReportMap => {
                formatter.write_str("status reply must be a MessagePack map")
            }
            Self::ExpectedRemoteResponseArray => {
                formatter.write_str("remote status reply must be a MessagePack array")
            }
            Self::EmptyRemoteResponse => {
                formatter.write_str("remote status reply must contain interface stats")
            }
            Self::ExpectedInterfacesArray => {
                formatter.write_str("status field interfaces must be an array")
            }
            Self::ExpectedInterfaceMap { index } => {
                write!(formatter, "status field interfaces[{index}] must be a map")
            }
            Self::InvalidMapKey { scope } => match scope {
                RnsStatsFieldScope::Report => {
                    formatter.write_str("status reply contains a non-string field name")
                }
                RnsStatsFieldScope::Interface(index) => write!(
                    formatter,
                    "status field interfaces[{index}] contains a non-string field name"
                ),
            },
            Self::MissingField(path) => write!(formatter, "status reply is missing {path}"),
            Self::DuplicateField(path) => write!(formatter, "status reply repeats {path}"),
            Self::InvalidFieldType(path) => {
                write!(formatter, "status reply has the wrong value type at {path}")
            }
            Self::InvalidHashLength {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "status reply has {actual} bytes at {path}, expected {expected}"
            ),
            Self::AllocationFailed { entries } => write!(
                formatter,
                "status reply declares {entries} interfaces, but storage could not be allocated"
            ),
            Self::TrailingData => formatter.write_str("status reply has trailing MessagePack data"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RnsInterfaceStatsDecodeError {}

#[derive(Default)]
struct ReportBuilder {
    interfaces: Option<Vec<RnsInterfaceStatusReport>>,
    receive_bytes: Option<u64>,
    transmit_bytes: Option<u64>,
    receive_speed_bps: Option<f64>,
    transmit_speed_bps: Option<f64>,
    resident_set_size_bytes: RnsOptionalField<u64>,
    transport_identity: RnsOptionalField<IdentityHash>,
    network_identity: RnsOptionalField<IdentityHash>,
    transport_uptime_seconds: RnsOptionalField<f64>,
    probe_responder: RnsOptionalField<DestinationHash>,
}

impl ReportBuilder {
    fn finish(self) -> Result<RnsInterfaceStatsReport, RnsInterfaceStatsDecodeError> {
        Ok(RnsInterfaceStatsReport {
            interfaces: required(
                self.interfaces,
                RnsStatsFieldPath::report(interface::INTERFACES),
            )?,
            receive_bytes: required(
                self.receive_bytes,
                RnsStatsFieldPath::report(interface::RECEIVE_BYTES),
            )?,
            transmit_bytes: required(
                self.transmit_bytes,
                RnsStatsFieldPath::report(interface::TRANSMIT_BYTES),
            )?,
            receive_speed_bps: required(
                self.receive_speed_bps,
                RnsStatsFieldPath::report(interface::RECEIVE_SPEED),
            )?,
            transmit_speed_bps: required(
                self.transmit_speed_bps,
                RnsStatsFieldPath::report(interface::TRANSMIT_SPEED),
            )?,
            resident_set_size_bytes: self.resident_set_size_bytes,
            transport_identity: self.transport_identity,
            network_identity: self.network_identity,
            transport_uptime_seconds: self.transport_uptime_seconds,
            probe_responder: self.probe_responder,
        })
    }
}

#[derive(Default)]
struct InterfaceBuilder {
    name: Option<String>,
    short_name: RnsOptionalField<String>,
    interface_type: RnsOptionalField<String>,
    interface_hash: RnsOptionalField<Vec<u8>>,
    parent_name: RnsOptionalField<String>,
    parent_hash: RnsOptionalField<Vec<u8>>,
    online: Option<bool>,
    mode: Option<RnsInterfaceMode>,
    gravity: RnsOptionalField<i64>,
    clients: RnsOptionalField<u64>,
    receive_bytes: Option<u64>,
    transmit_bytes: Option<u64>,
    receive_speed_bps: Option<f64>,
    transmit_speed_bps: Option<f64>,
    bitrate_bps: RnsOptionalField<f64>,
    peers: RnsOptionalField<u64>,
    ifac_signature: RnsOptionalField<Vec<u8>>,
    ifac_size_bytes: RnsOptionalField<u64>,
    ifac_network_name: RnsOptionalField<String>,
    autoconnect_source: RnsOptionalField<String>,
    announce_queue: RnsOptionalField<u64>,
    held_announces: RnsOptionalField<u64>,
    incoming_announce_frequency: RnsOptionalField<f64>,
    outgoing_announce_frequency: RnsOptionalField<f64>,
    incoming_path_request_frequency: RnsOptionalField<f64>,
    outgoing_path_request_frequency: RnsOptionalField<f64>,
    announce_rate_target_seconds: RnsOptionalField<f64>,
    announce_rate_penalty_seconds: RnsOptionalField<f64>,
    announce_rate_grace: RnsOptionalField<f64>,
    burst_active: RnsOptionalField<bool>,
    burst_activated_at: RnsOptionalField<f64>,
    path_request_burst_active: RnsOptionalField<bool>,
    path_request_burst_activated_at: RnsOptionalField<f64>,
    i2p_connectable: RnsOptionalField<bool>,
    i2p_b32: RnsOptionalField<String>,
    i2p_tunnel_state: RnsOptionalField<String>,
    airtime_short_percent: RnsOptionalField<f64>,
    airtime_long_percent: RnsOptionalField<f64>,
    channel_load_short_percent: RnsOptionalField<f64>,
    channel_load_long_percent: RnsOptionalField<f64>,
    noise_floor_dbm: RnsOptionalField<f64>,
    interference_dbm: RnsOptionalField<f64>,
    interference_last_at: RnsOptionalField<f64>,
    interference_last_dbm: RnsOptionalField<f64>,
    cpu_load_percent: RnsOptionalField<f64>,
    cpu_temperature_celsius: RnsOptionalField<f64>,
    memory_load_percent: RnsOptionalField<f64>,
    battery_percent: RnsOptionalField<f64>,
    battery_state: RnsOptionalField<String>,
    switch_id: RnsOptionalField<String>,
    endpoint_id: RnsOptionalField<String>,
    via_switch_id: RnsOptionalField<String>,
    blocked_ip_list: RnsOptionalField<Vec<String>>,
    rssi: RnsOptionalField<i64>,
    fleet_peers: Vec<RnsFleetPeerReport>,
}

impl InterfaceBuilder {
    fn finish(
        self,
        index: usize,
    ) -> Result<RnsInterfaceStatusReport, RnsInterfaceStatsDecodeError> {
        let path = |field| RnsStatsFieldPath::interface(index, field);
        Ok(RnsInterfaceStatusReport {
            name: required(self.name, path(interface::NAME))?,
            short_name: self.short_name,
            interface_type: self.interface_type,
            interface_hash: self.interface_hash,
            parent_name: self.parent_name,
            parent_hash: self.parent_hash,
            online: required(self.online, path(interface::STATUS))?,
            mode: required(self.mode, path(interface::MODE))?,
            gravity: self.gravity,
            clients: self.clients,
            receive_bytes: required(self.receive_bytes, path(interface::RECEIVE_BYTES))?,
            transmit_bytes: required(self.transmit_bytes, path(interface::TRANSMIT_BYTES))?,
            receive_speed_bps: required(self.receive_speed_bps, path(interface::RECEIVE_SPEED))?,
            transmit_speed_bps: required(self.transmit_speed_bps, path(interface::TRANSMIT_SPEED))?,
            bitrate_bps: self.bitrate_bps,
            peers: self.peers,
            ifac_signature: self.ifac_signature,
            ifac_size_bytes: self.ifac_size_bytes,
            ifac_network_name: self.ifac_network_name,
            autoconnect_source: self.autoconnect_source,
            announce_queue: self.announce_queue,
            held_announces: self.held_announces,
            incoming_announce_frequency: self.incoming_announce_frequency,
            outgoing_announce_frequency: self.outgoing_announce_frequency,
            incoming_path_request_frequency: self.incoming_path_request_frequency,
            outgoing_path_request_frequency: self.outgoing_path_request_frequency,
            announce_rate_target_seconds: self.announce_rate_target_seconds,
            announce_rate_penalty_seconds: self.announce_rate_penalty_seconds,
            announce_rate_grace: self.announce_rate_grace,
            burst_active: self.burst_active,
            burst_activated_at: self.burst_activated_at,
            path_request_burst_active: self.path_request_burst_active,
            path_request_burst_activated_at: self.path_request_burst_activated_at,
            i2p_connectable: self.i2p_connectable,
            i2p_b32: self.i2p_b32,
            i2p_tunnel_state: self.i2p_tunnel_state,
            airtime_short_percent: self.airtime_short_percent,
            airtime_long_percent: self.airtime_long_percent,
            channel_load_short_percent: self.channel_load_short_percent,
            channel_load_long_percent: self.channel_load_long_percent,
            noise_floor_dbm: self.noise_floor_dbm,
            interference_dbm: self.interference_dbm,
            interference_last_at: self.interference_last_at,
            interference_last_dbm: self.interference_last_dbm,
            cpu_load_percent: self.cpu_load_percent,
            cpu_temperature_celsius: self.cpu_temperature_celsius,
            memory_load_percent: self.memory_load_percent,
            battery_percent: self.battery_percent,
            battery_state: self.battery_state,
            switch_id: self.switch_id,
            endpoint_id: self.endpoint_id,
            via_switch_id: self.via_switch_id,
            blocked_ip_list: self.blocked_ip_list,
            rssi: self.rssi,
            fleet_peers: self.fleet_peers,
        })
    }
}

pub(super) fn decode(
    bytes: &[u8],
) -> Result<RnsInterfaceStatsReport, RnsInterfaceStatsDecodeError> {
    let mut reader = MessagePackReader::new(bytes);
    let report = decode_report(&mut reader)?;
    if !reader.is_finished() {
        return Err(RnsInterfaceStatsDecodeError::TrailingData);
    }
    Ok(report)
}

pub(super) fn decode_remote(
    bytes: &[u8],
) -> Result<RnsRemoteInterfaceStatsReport, RnsInterfaceStatsDecodeError> {
    let mut reader = MessagePackReader::new(bytes);
    let marker = marker(&mut reader)?;
    let length = reader
        .array_length(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .ok_or(RnsInterfaceStatsDecodeError::ExpectedRemoteResponseArray)?;
    if length == 0 {
        return Err(RnsInterfaceStatsDecodeError::EmptyRemoteResponse);
    }
    let status = decode_report(&mut reader)?;
    let link_count = if length >= 2 {
        match read_optional_u64(&mut reader, RnsStatsFieldPath::report("link_count"))? {
            RnsOptionalField::Value(value) => Some(value),
            RnsOptionalField::Absent | RnsOptionalField::Null => None,
        }
    } else {
        None
    };
    for _ in 2..length {
        skip(&mut reader)?;
    }
    if !reader.is_finished() {
        return Err(RnsInterfaceStatsDecodeError::TrailingData);
    }
    Ok(RnsRemoteInterfaceStatsReport { status, link_count })
}

fn decode_report(
    reader: &mut MessagePackReader<'_>,
) -> Result<RnsInterfaceStatsReport, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    let length = reader
        .map_length(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .ok_or(RnsInterfaceStatsDecodeError::ExpectedReportMap)?;
    let mut fields = BTreeSet::new();
    let mut report = ReportBuilder::default();
    for _ in 0..length {
        let key = read_key(reader, RnsStatsFieldScope::Report)?;
        ensure_unique(&mut fields, RnsStatsFieldPath::report(&key))?;
        let path = RnsStatsFieldPath::report(&key);
        match key.as_str() {
            interface::INTERFACES => report.interfaces = Some(read_interfaces(reader)?),
            interface::RECEIVE_BYTES => report.receive_bytes = Some(read_u64(reader, path)?),
            interface::TRANSMIT_BYTES => report.transmit_bytes = Some(read_u64(reader, path)?),
            interface::RECEIVE_SPEED => report.receive_speed_bps = Some(read_number(reader, path)?),
            interface::TRANSMIT_SPEED => {
                report.transmit_speed_bps = Some(read_number(reader, path)?)
            }
            interface::RESIDENT_SET_SIZE => {
                report.resident_set_size_bytes = read_optional_u64(reader, path)?
            }
            transport::IDENTITY => {
                report.transport_identity = read_optional_hash(reader, path, IdentityHash::new)?
            }
            transport::NETWORK_IDENTITY => {
                report.network_identity = read_optional_hash(reader, path, IdentityHash::new)?
            }
            transport::UPTIME => {
                report.transport_uptime_seconds = read_optional_number(reader, path)?
            }
            transport::PROBE_RESPONDER => {
                report.probe_responder = read_optional_hash(reader, path, DestinationHash::new)?
            }
            _ => skip(reader)?,
        }
    }
    report.finish()
}

fn read_interfaces(
    reader: &mut MessagePackReader<'_>,
) -> Result<Vec<RnsInterfaceStatusReport>, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    let length = reader
        .array_length(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .ok_or(RnsInterfaceStatsDecodeError::ExpectedInterfacesArray)?;
    let mut interfaces = Vec::new();
    interfaces
        .try_reserve_exact(length)
        .map_err(|_| RnsInterfaceStatsDecodeError::AllocationFailed { entries: length })?;
    for index in 0..length {
        interfaces.push(read_interface(reader, index)?);
    }
    Ok(interfaces)
}

fn read_interface(
    reader: &mut MessagePackReader<'_>,
    index: usize,
) -> Result<RnsInterfaceStatusReport, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    let length = reader
        .map_length(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .ok_or(RnsInterfaceStatsDecodeError::ExpectedInterfaceMap { index })?;
    let mut fields = BTreeSet::new();
    let mut interface_report = InterfaceBuilder::default();
    for _ in 0..length {
        let key = read_key(reader, RnsStatsFieldScope::Interface(index))?;
        ensure_unique(&mut fields, RnsStatsFieldPath::interface(index, &key))?;
        let path = RnsStatsFieldPath::interface(index, &key);
        match key.as_str() {
            interface::NAME => interface_report.name = Some(read_string(reader, path)?),
            interface::SHORT_NAME => {
                interface_report.short_name = read_optional_string(reader, path)?
            }
            interface::TYPE => {
                interface_report.interface_type = read_optional_string(reader, path)?
            }
            interface::HASH => {
                interface_report.interface_hash = read_optional_binary(reader, path)?
            }
            interface::PARENT_NAME => {
                interface_report.parent_name = read_optional_string(reader, path)?
            }
            interface::PARENT_HASH => {
                interface_report.parent_hash = read_optional_binary(reader, path)?
            }
            interface::STATUS => interface_report.online = Some(read_bool(reader, path)?),
            interface::MODE => {
                interface_report.mode = Some(RnsInterfaceMode::from_wire(read_i64(reader, path)?))
            }
            interface::GRAVITY => interface_report.gravity = read_optional_i64(reader, path)?,
            interface::CLIENTS => interface_report.clients = read_optional_u64(reader, path)?,
            interface::RECEIVE_BYTES => {
                interface_report.receive_bytes = Some(read_u64(reader, path)?)
            }
            interface::TRANSMIT_BYTES => {
                interface_report.transmit_bytes = Some(read_u64(reader, path)?)
            }
            interface::RECEIVE_SPEED => {
                interface_report.receive_speed_bps = Some(read_number(reader, path)?)
            }
            interface::TRANSMIT_SPEED => {
                interface_report.transmit_speed_bps = Some(read_number(reader, path)?)
            }
            interface::BITRATE => {
                interface_report.bitrate_bps = read_optional_number(reader, path)?
            }
            interface::PEERS => interface_report.peers = read_optional_u64(reader, path)?,
            interface::IFAC_SIGNATURE => {
                interface_report.ifac_signature = read_optional_binary(reader, path)?
            }
            interface::IFAC_SIZE => {
                interface_report.ifac_size_bytes = read_optional_u64(reader, path)?
            }
            interface::IFAC_NETWORK_NAME => {
                interface_report.ifac_network_name = read_optional_string(reader, path)?
            }
            interface::AUTOCONNECT_SOURCE => {
                interface_report.autoconnect_source = read_optional_string(reader, path)?
            }
            interface::ANNOUNCE_QUEUE => {
                interface_report.announce_queue = read_optional_u64(reader, path)?
            }
            interface::HELD_ANNOUNCES => {
                interface_report.held_announces = read_optional_u64(reader, path)?
            }
            interface::INCOMING_ANNOUNCE_FREQUENCY => {
                interface_report.incoming_announce_frequency = read_optional_number(reader, path)?
            }
            interface::OUTGOING_ANNOUNCE_FREQUENCY => {
                interface_report.outgoing_announce_frequency = read_optional_number(reader, path)?
            }
            interface::INCOMING_PATH_REQUEST_FREQUENCY => {
                interface_report.incoming_path_request_frequency =
                    read_optional_number(reader, path)?
            }
            interface::OUTGOING_PATH_REQUEST_FREQUENCY => {
                interface_report.outgoing_path_request_frequency =
                    read_optional_number(reader, path)?
            }
            interface::ANNOUNCE_RATE_TARGET => {
                interface_report.announce_rate_target_seconds = read_optional_number(reader, path)?
            }
            interface::ANNOUNCE_RATE_PENALTY => {
                interface_report.announce_rate_penalty_seconds = read_optional_number(reader, path)?
            }
            interface::ANNOUNCE_RATE_GRACE => {
                interface_report.announce_rate_grace = read_optional_number(reader, path)?
            }
            interface::BURST_ACTIVE => {
                interface_report.burst_active = read_optional_bool(reader, path)?
            }
            interface::BURST_ACTIVATED => {
                interface_report.burst_activated_at = read_optional_number(reader, path)?
            }
            interface::PATH_REQUEST_BURST_ACTIVE => {
                interface_report.path_request_burst_active = read_optional_bool(reader, path)?
            }
            interface::PATH_REQUEST_BURST_ACTIVATED => {
                interface_report.path_request_burst_activated_at =
                    read_optional_number(reader, path)?
            }
            interface::I2P_CONNECTABLE => {
                interface_report.i2p_connectable = read_optional_bool(reader, path)?
            }
            interface::I2P_B32 => interface_report.i2p_b32 = read_optional_string(reader, path)?,
            interface::I2P_TUNNEL_STATE => {
                interface_report.i2p_tunnel_state = read_optional_string(reader, path)?
            }
            interface::AIRTIME_SHORT => {
                interface_report.airtime_short_percent = read_optional_number(reader, path)?
            }
            interface::AIRTIME_LONG => {
                interface_report.airtime_long_percent = read_optional_number(reader, path)?
            }
            interface::CHANNEL_LOAD_SHORT => {
                interface_report.channel_load_short_percent = read_optional_number(reader, path)?
            }
            interface::CHANNEL_LOAD_LONG => {
                interface_report.channel_load_long_percent = read_optional_number(reader, path)?
            }
            interface::NOISE_FLOOR => {
                interface_report.noise_floor_dbm = read_optional_number(reader, path)?
            }
            interface::INTERFERENCE => {
                interface_report.interference_dbm = read_optional_number(reader, path)?
            }
            interface::INTERFERENCE_LAST_AT => {
                interface_report.interference_last_at = read_optional_number(reader, path)?
            }
            interface::INTERFERENCE_LAST_DBM => {
                interface_report.interference_last_dbm = read_optional_number(reader, path)?
            }
            interface::CPU_LOAD => {
                interface_report.cpu_load_percent = read_optional_number(reader, path)?
            }
            interface::CPU_TEMPERATURE => {
                interface_report.cpu_temperature_celsius = read_optional_number(reader, path)?
            }
            interface::MEMORY_LOAD => {
                interface_report.memory_load_percent = read_optional_number(reader, path)?
            }
            interface::BATTERY_PERCENT => {
                interface_report.battery_percent = read_optional_number(reader, path)?
            }
            interface::BATTERY_STATE => {
                interface_report.battery_state = read_optional_string(reader, path)?
            }
            interface::SWITCH_ID => {
                interface_report.switch_id = read_optional_string(reader, path)?
            }
            interface::ENDPOINT_ID => {
                interface_report.endpoint_id = read_optional_string(reader, path)?
            }
            interface::VIA_SWITCH_ID => {
                interface_report.via_switch_id = read_optional_string(reader, path)?
            }
            interface::BLOCKED_IP_LIST => {
                interface_report.blocked_ip_list = read_optional_string_array(reader, path)?
            }
            interface::RSSI => interface_report.rssi = read_optional_i64(reader, path)?,
            interface::FLEET_PEERS => {
                interface_report.fleet_peers = read_fleet_peers(reader, index)?
            }
            _ => skip(reader)?,
        }
    }
    interface_report.finish(index)
}

fn read_fleet_peers(
    reader: &mut MessagePackReader<'_>,
    parent_index: usize,
) -> Result<Vec<RnsFleetPeerReport>, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    let length = reader
        .array_length(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .ok_or(RnsInterfaceStatsDecodeError::ExpectedInterfacesArray)?;
    let mut peers = Vec::new();
    peers
        .try_reserve_exact(length)
        .map_err(|_| RnsInterfaceStatsDecodeError::AllocationFailed { entries: length })?;
    for peer_index in 0..length {
        peers.push(read_fleet_peer(reader, parent_index, peer_index)?);
    }
    Ok(peers)
}

fn read_fleet_peer(
    reader: &mut MessagePackReader<'_>,
    parent_index: usize,
    peer_index: usize,
) -> Result<RnsFleetPeerReport, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    let length = reader
        .map_length(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .ok_or(RnsInterfaceStatsDecodeError::ExpectedInterfaceMap {
            index: parent_index,
        })?;
    let mut fields = BTreeSet::new();
    let mut name = None;
    let mut online = None;
    let mut receive_bytes = None;
    let mut transmit_bytes = None;
    let mut receive_speed_bps = None;
    let mut transmit_speed_bps = None;
    let mut rssi = RnsOptionalField::Absent;
    for _ in 0..length {
        let key = read_key(reader, RnsStatsFieldScope::Interface(parent_index))?;
        let path = RnsStatsFieldPath::interface(parent_index, &format!("fleet_peers[{peer_index}].{key}"));
        ensure_unique(&mut fields, path.clone())?;
        match key.as_str() {
            interface::NAME => name = Some(read_string(reader, path)?),
            interface::STATUS => online = Some(read_bool(reader, path)?),
            interface::RECEIVE_BYTES => receive_bytes = Some(read_u64(reader, path)?),
            interface::TRANSMIT_BYTES => transmit_bytes = Some(read_u64(reader, path)?),
            interface::RECEIVE_SPEED => receive_speed_bps = Some(read_number(reader, path)?),
            interface::TRANSMIT_SPEED => transmit_speed_bps = Some(read_number(reader, path)?),
            interface::RSSI => rssi = read_optional_i64(reader, path)?,
            _ => skip(reader)?,
        }
    }
    Ok(RnsFleetPeerReport {
        name: required(name, RnsStatsFieldPath::interface(parent_index, interface::NAME))?,
        online: required(online, RnsStatsFieldPath::interface(parent_index, interface::STATUS))?,
        receive_bytes: required(
            receive_bytes,
            RnsStatsFieldPath::interface(parent_index, interface::RECEIVE_BYTES),
        )?,
        transmit_bytes: required(
            transmit_bytes,
            RnsStatsFieldPath::interface(parent_index, interface::TRANSMIT_BYTES),
        )?,
        receive_speed_bps: required(
            receive_speed_bps,
            RnsStatsFieldPath::interface(parent_index, interface::RECEIVE_SPEED),
        )?,
        transmit_speed_bps: required(
            transmit_speed_bps,
            RnsStatsFieldPath::interface(parent_index, interface::TRANSMIT_SPEED),
        )?,
        rssi,
    })
}

fn required<T>(
    value: Option<T>,
    path: RnsStatsFieldPath,
) -> Result<T, RnsInterfaceStatsDecodeError> {
    value.ok_or(RnsInterfaceStatsDecodeError::MissingField(path))
}

fn ensure_unique(
    fields: &mut BTreeSet<String>,
    path: RnsStatsFieldPath,
) -> Result<(), RnsInterfaceStatsDecodeError> {
    if fields.insert(path.field.clone()) {
        Ok(())
    } else {
        Err(RnsInterfaceStatsDecodeError::DuplicateField(path))
    }
}

fn read_key(
    reader: &mut MessagePackReader<'_>,
    scope: RnsStatsFieldScope,
) -> Result<String, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    reader
        .string(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .map(ToString::to_string)
        .ok_or(RnsInterfaceStatsDecodeError::InvalidMapKey { scope })
}

fn read_string(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<String, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    reader
        .string(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .map(ToString::to_string)
        .ok_or(RnsInterfaceStatsDecodeError::InvalidFieldType(path))
}

fn read_optional_string(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<RnsOptionalField<String>, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    if marker == Marker::Null {
        return Ok(RnsOptionalField::Null);
    }
    reader
        .string(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .map(|value| RnsOptionalField::Value(value.to_string()))
        .ok_or(RnsInterfaceStatsDecodeError::InvalidFieldType(path))
}

fn read_optional_string_array(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<RnsOptionalField<Vec<String>>, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    if marker == Marker::Null {
        return Ok(RnsOptionalField::Null);
    }
    let length = reader
        .array_length(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .ok_or_else(|| RnsInterfaceStatsDecodeError::InvalidFieldType(path.clone()))?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| RnsInterfaceStatsDecodeError::AllocationFailed { entries: length })?;
    for _ in 0..length {
        values.push(read_string(reader, path.clone())?);
    }
    Ok(RnsOptionalField::Value(values))
}

fn read_optional_binary(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<RnsOptionalField<Vec<u8>>, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    if marker == Marker::Null {
        return Ok(RnsOptionalField::Null);
    }
    reader
        .binary(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .map(|value| RnsOptionalField::Value(value.to_vec()))
        .ok_or(RnsInterfaceStatsDecodeError::InvalidFieldType(path))
}

fn read_bool(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<bool, RnsInterfaceStatsDecodeError> {
    match marker(reader)? {
        Marker::True => Ok(true),
        Marker::False => Ok(false),
        _ => Err(RnsInterfaceStatsDecodeError::InvalidFieldType(path)),
    }
}

fn read_optional_bool(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<RnsOptionalField<bool>, RnsInterfaceStatsDecodeError> {
    match marker(reader)? {
        Marker::Null => Ok(RnsOptionalField::Null),
        Marker::True => Ok(RnsOptionalField::Value(true)),
        Marker::False => Ok(RnsOptionalField::Value(false)),
        _ => Err(RnsInterfaceStatsDecodeError::InvalidFieldType(path)),
    }
}

fn read_u64(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<u64, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    match reader
        .integer(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
    {
        Some(MessagePackInteger::Nonnegative(value)) => Ok(value),
        Some(MessagePackInteger::Negative(_)) | None => {
            Err(RnsInterfaceStatsDecodeError::InvalidFieldType(path))
        }
    }
}

fn read_i64(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<i64, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    match reader
        .integer(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
    {
        Some(MessagePackInteger::Negative(value)) => Ok(value),
        Some(MessagePackInteger::Nonnegative(value)) => {
            i64::try_from(value).map_err(|_| RnsInterfaceStatsDecodeError::InvalidFieldType(path))
        }
        None => Err(RnsInterfaceStatsDecodeError::InvalidFieldType(path)),
    }
}

fn read_optional_i64(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<RnsOptionalField<i64>, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    if marker == Marker::Null {
        return Ok(RnsOptionalField::Null);
    }
    match reader
        .integer(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
    {
        Some(MessagePackInteger::Negative(value)) => Ok(RnsOptionalField::Value(value)),
        Some(MessagePackInteger::Nonnegative(value)) => i64::try_from(value)
            .map(RnsOptionalField::Value)
            .map_err(|_| RnsInterfaceStatsDecodeError::InvalidFieldType(path)),
        None => Err(RnsInterfaceStatsDecodeError::InvalidFieldType(path)),
    }
}

fn read_number(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<f64, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    if let Some(integer) = reader
        .integer(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
    {
        return Ok(match integer {
            MessagePackInteger::Negative(value) => value as f64,
            MessagePackInteger::Nonnegative(value) => value as f64,
        });
    }
    reader
        .float(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .ok_or(RnsInterfaceStatsDecodeError::InvalidFieldType(path))
}

fn read_optional_u64(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<RnsOptionalField<u64>, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    if marker == Marker::Null {
        return Ok(RnsOptionalField::Null);
    }
    match reader
        .integer(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
    {
        Some(MessagePackInteger::Nonnegative(value)) => Ok(RnsOptionalField::Value(value)),
        Some(MessagePackInteger::Negative(_)) | None => {
            Err(RnsInterfaceStatsDecodeError::InvalidFieldType(path))
        }
    }
}

fn read_optional_number(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
) -> Result<RnsOptionalField<f64>, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    if marker == Marker::Null {
        return Ok(RnsOptionalField::Null);
    }
    if let Some(integer) = reader
        .integer(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
    {
        return Ok(RnsOptionalField::Value(match integer {
            MessagePackInteger::Negative(value) => value as f64,
            MessagePackInteger::Nonnegative(value) => value as f64,
        }));
    }
    reader
        .float(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .map(RnsOptionalField::Value)
        .ok_or(RnsInterfaceStatsDecodeError::InvalidFieldType(path))
}

fn read_optional_hash<T>(
    reader: &mut MessagePackReader<'_>,
    path: RnsStatsFieldPath,
    construct: impl FnOnce([u8; 16]) -> T,
) -> Result<RnsOptionalField<T>, RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    if marker == Marker::Null {
        return Ok(RnsOptionalField::Null);
    }
    let bytes = reader
        .binary(marker)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)?
        .ok_or_else(|| RnsInterfaceStatsDecodeError::InvalidFieldType(path.clone()))?;
    let actual = bytes.len();
    let bytes: [u8; 16] =
        bytes
            .try_into()
            .map_err(|_| RnsInterfaceStatsDecodeError::InvalidHashLength {
                path,
                expected: 16,
                actual,
            })?;
    Ok(RnsOptionalField::Value(construct(bytes)))
}

fn skip(reader: &mut MessagePackReader<'_>) -> Result<(), RnsInterfaceStatsDecodeError> {
    let marker = marker(reader)?;
    reader
        .skip_value(marker, 0, MAXIMUM_DEPTH)
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)
}

fn marker(reader: &mut MessagePackReader<'_>) -> Result<Marker, RnsInterfaceStatsDecodeError> {
    reader
        .marker()
        .map_err(|_| RnsInterfaceStatsDecodeError::InvalidMessagePack)
}
