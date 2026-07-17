use std::collections::BTreeMap;

use prns_core::identity::IdentityHash;
use prns_core::interface_discovery::StampCost;

pub use crate::configobj::Value as ReferenceValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceMode {
    Full,
    AccessPoint,
    PointToPoint,
    Roaming,
    Boundary,
    Gateway,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RNodeRadio {
    pub frequency: Option<u64>,
    pub bandwidth: Option<u32>,
    pub spreadingfactor: Option<u8>,
    pub codingrate: Option<u8>,
    pub txpower: Option<i16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RNodeSubinterface {
    pub name: String,
    pub enabled: Option<bool>,
    pub vport: Option<String>,
    pub radio: RNodeRadio,
    pub extra: BTreeMap<String, ReferenceValue>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReferenceDiscoveryConfig {
    pub discover_interfaces: Option<bool>,
    pub required_stamp_cost: Option<StampCost>,
    pub interface_sources: Vec<IdentityHash>,
    pub auto_connect_limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReferenceInterfaceDiscovery {
    pub discoverable: Option<bool>,
    pub announce_interval_minutes: Option<i64>,
    pub stamp_cost: Option<StampCost>,
    pub name: Option<String>,
    pub encrypt: Option<bool>,
    pub reachable_on: Option<String>,
    pub publish_ifac: Option<bool>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub height: Option<f64>,
    pub frequency_hz: Option<u64>,
    pub bandwidth_hz: Option<u32>,
    pub modulation: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceParams {
    Auto {
        group_id: Option<String>,
        discovery_scope: Option<String>,
        discovery_port: Option<u16>,
        data_port: Option<u16>,
        devices: Option<Vec<String>>,
        ignored_devices: Option<Vec<String>>,
        multicast_address_type: Option<String>,
    },
    TcpClient {
        target_host: Option<String>,
        target_port: Option<u16>,
        kiss_framing: Option<bool>,
        i2p_tunneled: Option<bool>,
        connect_timeout: Option<u64>,
        max_reconnect_tries: Option<u32>,
        fixed_mtu: Option<usize>,
    },
    TcpServer {
        listen_ip: Option<String>,
        listen_port: Option<u16>,
        device: Option<String>,
        port: Option<u16>,
        prefer_ipv6: Option<bool>,
        i2p_tunneled: Option<bool>,
        kiss_framing: Option<bool>,
        fixed_mtu: Option<usize>,
    },
    Udp {
        listen_ip: Option<String>,
        listen_port: Option<u16>,
        forward_ip: Option<String>,
        forward_port: Option<u16>,
        device: Option<String>,
        port: Option<u16>,
    },
    Serial {
        port: Option<String>,
        speed: Option<u32>,
        databits: Option<u8>,
        parity: Option<String>,
        stopbits: Option<u8>,
    },
    Rnode {
        port: Option<String>,
        radio: RNodeRadio,
        flow_control: Option<bool>,
        id_callsign: Option<String>,
        id_interval: Option<u64>,
        airtime_limit_short: Option<f64>,
        airtime_limit_long: Option<f64>,
    },
    RnodeMulti {
        port: Option<String>,
        flow_control: Option<bool>,
        id_callsign: Option<String>,
        id_interval: Option<u64>,
        airtime_limit_short: Option<f64>,
        airtime_limit_long: Option<f64>,
        subinterfaces: Vec<RNodeSubinterface>,
    },
    Kiss {
        port: Option<String>,
        speed: Option<u32>,
        databits: Option<u8>,
        parity: Option<String>,
        stopbits: Option<u8>,
        flow_control: Option<bool>,
        preamble: Option<u32>,
        txtail: Option<u32>,
        persistence: Option<u32>,
        slottime: Option<u32>,
        id_callsign: Option<String>,
        id_interval: Option<u64>,
    },
    Ax25Kiss {
        port: Option<String>,
        speed: Option<u32>,
        databits: Option<u8>,
        parity: Option<String>,
        stopbits: Option<u8>,
        flow_control: Option<bool>,
        preamble: Option<u32>,
        txtail: Option<u32>,
        persistence: Option<u32>,
        slottime: Option<u32>,
        callsign: Option<String>,
        ssid: Option<u8>,
    },
    Pipe {
        command: Option<String>,
        respawn_delay: Option<f64>,
    },
    I2p {
        peers: Option<Vec<String>>,
        connectable: Option<bool>,
    },
    Backbone {
        listen_ip: Option<String>,
        listen_port: Option<u16>,
        target_host: Option<String>,
        target_port: Option<u16>,
        port: Option<u16>,
        device: Option<String>,
        prefer_ipv6: Option<bool>,
        i2p_tunneled: Option<bool>,
        connect_timeout: Option<u64>,
        max_reconnect_tries: Option<u32>,
    },
    Weave {
        port: Option<u16>,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceInterface {
    pub name: String,
    pub type_name: String,
    pub enabled: Option<bool>,
    pub mode: Option<ReferenceMode>,
    pub outgoing: Option<bool>,
    pub bitrate: Option<u64>,
    pub announce_cap: Option<f64>,
    pub announce_rate_target: Option<u64>,
    pub announce_rate_grace: Option<u64>,
    pub announce_rate_penalty: Option<u64>,
    pub ingress_control: Option<bool>,
    pub egress_control: Option<bool>,
    pub recursive_prs: Option<bool>,
    pub announces_from_internal: Option<bool>,
    pub ic_max_held_announces: Option<i64>,
    pub ic_new_time: Option<f64>,
    pub ic_burst_hold: Option<f64>,
    pub ic_burst_freq_new: Option<f64>,
    pub ic_burst_freq: Option<f64>,
    pub ic_pr_burst_freq_new: Option<f64>,
    pub ic_pr_burst_freq: Option<f64>,
    pub ic_burst_penalty: Option<f64>,
    pub ic_held_release_interval: Option<f64>,
    pub ec_pr_freq: Option<f64>,
    pub network_name: Option<String>,
    pub passphrase: Option<String>,
    pub ifac_size_bits: Option<u32>,
    pub discovery: ReferenceInterfaceDiscovery,
    pub params: ReferenceParams,
    pub extra: BTreeMap<String, ReferenceValue>,
}

impl ReferenceInterface {
    pub fn is_known(&self) -> bool {
        !matches!(self.params, ReferenceParams::Unknown)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReferenceConfig {
    pub interfaces: Vec<ReferenceInterface>,
    pub globals: BTreeMap<String, ReferenceValue>,
    pub network_identity_path: Option<String>,
    pub discovery: ReferenceDiscoveryConfig,
    pub other_sections: BTreeMap<String, BTreeMap<String, ReferenceValue>>,
}
