mod decode;

use alloc::string::String;
use alloc::vec::Vec;

use crate::identity::IdentityHash;
use crate::wire::DestinationHash;

pub use decode::{RnsInterfaceStatsDecodeError, RnsStatsFieldPath, RnsStatsFieldScope};

#[derive(Debug, Clone, Default, PartialEq)]
pub enum RnsOptionalField<T> {
    #[default]
    Absent,
    Null,
    Value(T),
}

impl<T> RnsOptionalField<T> {
    pub const fn value(&self) -> Option<&T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Absent | Self::Null => None,
        }
    }

    pub const fn is_present(&self) -> bool {
        !matches!(self, Self::Absent)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsInterfaceMode {
    Full,
    PointToPoint,
    AccessPoint,
    Roaming,
    Boundary,
    Gateway,
    Internal,
    Unknown(i64),
}

impl RnsInterfaceMode {
    pub const fn from_wire(value: i64) -> Self {
        match value {
            0x01 => Self::Full,
            0x02 => Self::PointToPoint,
            0x03 => Self::AccessPoint,
            0x04 => Self::Roaming,
            0x05 => Self::Boundary,
            0x06 => Self::Gateway,
            0x07 => Self::Internal,
            value => Self::Unknown(value),
        }
    }

    pub const fn wire_value(self) -> i64 {
        match self {
            Self::Full => 0x01,
            Self::PointToPoint => 0x02,
            Self::AccessPoint => 0x03,
            Self::Roaming => 0x04,
            Self::Boundary => 0x05,
            Self::Gateway => 0x06,
            Self::Internal => 0x07,
            Self::Unknown(value) => value,
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Full | Self::Unknown(_) => "Full",
            Self::PointToPoint => "Point-to-Point",
            Self::AccessPoint => "Access Point",
            Self::Roaming => "Roaming",
            Self::Boundary => "Boundary",
            Self::Gateway => "Gateway",
            Self::Internal => "Internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RnsInterfaceStatusReport {
    pub name: String,
    pub short_name: RnsOptionalField<String>,
    pub interface_type: RnsOptionalField<String>,
    pub interface_hash: RnsOptionalField<Vec<u8>>,
    pub parent_name: RnsOptionalField<String>,
    pub parent_hash: RnsOptionalField<Vec<u8>>,
    pub online: bool,
    pub mode: RnsInterfaceMode,
    pub gravity: RnsOptionalField<i64>,
    pub clients: RnsOptionalField<u64>,
    pub receive_bytes: u64,
    pub transmit_bytes: u64,
    pub receive_speed_bps: f64,
    pub transmit_speed_bps: f64,
    pub bitrate_bps: RnsOptionalField<f64>,
    pub peers: RnsOptionalField<u64>,
    pub ifac_signature: RnsOptionalField<Vec<u8>>,
    pub ifac_size_bytes: RnsOptionalField<u64>,
    pub ifac_network_name: RnsOptionalField<String>,
    pub autoconnect_source: RnsOptionalField<String>,
    pub announce_queue: RnsOptionalField<u64>,
    pub held_announces: RnsOptionalField<u64>,
    pub incoming_announce_frequency: RnsOptionalField<f64>,
    pub outgoing_announce_frequency: RnsOptionalField<f64>,
    pub incoming_path_request_frequency: RnsOptionalField<f64>,
    pub outgoing_path_request_frequency: RnsOptionalField<f64>,
    pub announce_rate_target_seconds: RnsOptionalField<f64>,
    pub announce_rate_penalty_seconds: RnsOptionalField<f64>,
    pub announce_rate_grace: RnsOptionalField<f64>,
    pub burst_active: RnsOptionalField<bool>,
    pub burst_activated_at: RnsOptionalField<f64>,
    pub path_request_burst_active: RnsOptionalField<bool>,
    pub path_request_burst_activated_at: RnsOptionalField<f64>,
    pub i2p_connectable: RnsOptionalField<bool>,
    pub i2p_b32: RnsOptionalField<String>,
    pub i2p_tunnel_state: RnsOptionalField<String>,
    pub airtime_short_percent: RnsOptionalField<f64>,
    pub airtime_long_percent: RnsOptionalField<f64>,
    pub channel_load_short_percent: RnsOptionalField<f64>,
    pub channel_load_long_percent: RnsOptionalField<f64>,
    pub noise_floor_dbm: RnsOptionalField<f64>,
    pub interference_dbm: RnsOptionalField<f64>,
    pub interference_last_at: RnsOptionalField<f64>,
    pub interference_last_dbm: RnsOptionalField<f64>,
    pub cpu_load_percent: RnsOptionalField<f64>,
    pub cpu_temperature_celsius: RnsOptionalField<f64>,
    pub memory_load_percent: RnsOptionalField<f64>,
    pub battery_percent: RnsOptionalField<f64>,
    pub battery_state: RnsOptionalField<String>,
    pub switch_id: RnsOptionalField<String>,
    pub endpoint_id: RnsOptionalField<String>,
    pub via_switch_id: RnsOptionalField<String>,
    pub blocked_ip_list: RnsOptionalField<Vec<String>>,
    pub rssi: RnsOptionalField<i64>,
    pub fleet_peers: Vec<RnsFleetPeerReport>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RnsFleetPeerReport {
    pub name: String,
    pub online: bool,
    pub receive_bytes: u64,
    pub transmit_bytes: u64,
    pub receive_speed_bps: f64,
    pub transmit_speed_bps: f64,
    pub rssi: RnsOptionalField<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RnsInterfaceStatsReport {
    pub interfaces: Vec<RnsInterfaceStatusReport>,
    pub receive_bytes: u64,
    pub transmit_bytes: u64,
    pub receive_speed_bps: f64,
    pub transmit_speed_bps: f64,
    pub resident_set_size_bytes: RnsOptionalField<u64>,
    pub transport_identity: RnsOptionalField<IdentityHash>,
    pub network_identity: RnsOptionalField<IdentityHash>,
    pub transport_uptime_seconds: RnsOptionalField<f64>,
    pub probe_responder: RnsOptionalField<DestinationHash>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RnsRemoteInterfaceStatsReport {
    pub status: RnsInterfaceStatsReport,
    pub link_count: Option<u64>,
}

impl RnsInterfaceStatsReport {
    pub fn decode_message_pack(bytes: &[u8]) -> Result<Self, RnsInterfaceStatsDecodeError> {
        decode::decode(bytes)
    }
}

impl RnsRemoteInterfaceStatsReport {
    pub fn decode_message_pack(bytes: &[u8]) -> Result<Self, RnsInterfaceStatsDecodeError> {
        decode::decode_remote(bytes)
    }
}

#[cfg(test)]
mod tests;
