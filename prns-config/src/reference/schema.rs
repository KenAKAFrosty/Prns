#[derive(Debug, Clone, Copy)]
pub(super) enum ValueKind {
    Bool,
    Mode,
    String,
    List,
    I2pPeers,
    Bitrate,
    LinkMtu,
    U64,
    U32,
    U16,
    U8,
    I16,
    I64,
    F64,
    StampCost,
    IdentityHashes,
    LogLevel,
    SharedInstanceType,
    HexBytes,
}

impl ValueKind {
    pub(super) fn accepted(self) -> &'static str {
        match self {
            ValueKind::Bool => "yes, no, true, false, on, off, 1, or 0",
            ValueKind::Mode => {
                "full, access_point, pointtopoint, roaming, boundary, gateway, internal, or their stock aliases"
            }
            ValueKind::String => "one scalar value",
            ValueKind::List => "one value or a comma-separated list",
            ValueKind::I2pPeers => {
                "comma-separated .i2p names or I2P base64 destinations"
            }
            ValueKind::Bitrate => "an integer from 5 through 18446744073709551615 bps",
            ValueKind::LinkMtu => "an integer from 1 through 524288 bytes",
            ValueKind::U64 => "a non-negative integer",
            ValueKind::U32 => "an integer from 0 through 4294967295",
            ValueKind::U16 => "an integer from 0 through 65535",
            ValueKind::U8 => "an integer from 0 through 255",
            ValueKind::I16 => "an integer from -32768 through 32767",
            ValueKind::I64 => "a signed 64-bit integer",
            ValueKind::F64 => "a number",
            ValueKind::StampCost => "0 for the default, or an integer from 1 through 255",
            ValueKind::IdentityHashes => {
                "one or more comma-separated 32-character hexadecimal identity hashes"
            }
            ValueKind::LogLevel => "an integer from 0 through 7",
            ValueKind::SharedInstanceType => "tcp or unix",
            ValueKind::HexBytes => "an even-length hexadecimal byte string",
        }
    }

    pub(super) fn example(self) -> &'static str {
        match self {
            ValueKind::Bool => "Yes",
            ValueKind::Mode => "full",
            ValueKind::String => "value",
            ValueKind::List => "first, second",
            ValueKind::I2pPeers => "example.i2p, QUJDRA==",
            ValueKind::Bitrate => "500000000",
            ValueKind::LinkMtu => "131072",
            ValueKind::U64 | ValueKind::U32 => "1000000",
            ValueKind::U16 => "4242",
            ValueKind::U8 => "8",
            ValueKind::I16 | ValueKind::I64 => "0",
            ValueKind::F64 => "1.0",
            ValueKind::StampCost => "0",
            ValueKind::IdentityHashes => "00112233445566778899aabbccddeeff",
            ValueKind::LogLevel => "4",
            ValueKind::SharedInstanceType => "tcp",
            ValueKind::HexBytes => "00112233aabbccdd",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum KeyRule {
    Validate(ValueKind),
    Recognized,
}

pub(super) const GLOBAL_RULES: &[(&str, ValueKind)] = &[
    (global_key::SHARE_INSTANCE, ValueKind::Bool),
    (global_key::INSTANCE_NAME, ValueKind::String),
    (
        global_key::SHARED_INSTANCE_TYPE,
        ValueKind::SharedInstanceType,
    ),
    (global_key::SHARED_INSTANCE_PORT, ValueKind::U16),
    (global_key::INSTANCE_CONTROL_PORT, ValueKind::U16),
    (global_key::RPC_KEY, ValueKind::HexBytes),
    (global_key::ENABLE_TRANSPORT, ValueKind::Bool),
    (global_key::STATIC_TRANSPORT_IDENTITY, ValueKind::Bool),
    (global_key::LOCAL_HOPS_DELTA, ValueKind::Bool),
    (global_key::NETWORK_IDENTITY, ValueKind::String),
    (global_key::LINK_MTU_DISCOVERY, ValueKind::Bool),
    (global_key::ENABLE_REMOTE_MANAGEMENT, ValueKind::Bool),
    (
        global_key::REMOTE_MANAGEMENT_ALLOWED,
        ValueKind::IdentityHashes,
    ),
    (global_key::RESPOND_TO_PROBES, ValueKind::Bool),
    (
        global_key::FORCE_SHARED_INSTANCE_BITRATE,
        ValueKind::Bitrate,
    ),
    (global_key::PANIC_ON_INTERFACE_ERROR, ValueKind::Bool),
    (global_key::USE_IMPLICIT_PROOF, ValueKind::Bool),
    (global_key::DISCOVER_INTERFACES, ValueKind::Bool),
    (global_key::REQUIRED_DISCOVERY_VALUE, ValueKind::StampCost),
    (global_key::PUBLISH_BLACKHOLE, ValueKind::Bool),
    (global_key::BLACKHOLE_SOURCES, ValueKind::IdentityHashes),
    (global_key::BLACKHOLE_UPDATE_INTERVAL, ValueKind::F64),
    (
        global_key::INTERFACE_DISCOVERY_SOURCES,
        ValueKind::IdentityHashes,
    ),
    (
        global_key::AUTOCONNECT_DISCOVERED_INTERFACES,
        ValueKind::I64,
    ),
    (global_key::DEFAULT_AR_TARGET, ValueKind::I64),
    (global_key::DEFAULT_AR_PENALTY, ValueKind::I64),
    (global_key::DEFAULT_AR_GRACE, ValueKind::I64),
    (common_key::IC_MAX_HELD_ANNOUNCES, ValueKind::I64),
    (common_key::IC_BURST_HOLD, ValueKind::F64),
    (common_key::IC_BURST_FREQ_NEW, ValueKind::F64),
    (common_key::IC_BURST_FREQ, ValueKind::F64),
    (common_key::IC_PR_BURST_FREQ_NEW, ValueKind::F64),
    (common_key::IC_PR_BURST_FREQ, ValueKind::F64),
    (common_key::EC_PR_FREQ, ValueKind::F64),
    (common_key::EGRESS_CONTROL, ValueKind::Bool),
    (common_key::IC_NEW_TIME, ValueKind::F64),
    (common_key::IC_BURST_PENALTY, ValueKind::F64),
    (common_key::IC_HELD_RELEASE_INTERVAL, ValueKind::F64),
];

pub(super) const LOGGING_RULES: &[(&str, ValueKind)] = &[
    (logging_key::LEVEL, ValueKind::LogLevel),
    (logging_key::TIMESTAMPS, ValueKind::Bool),
];

pub(super) const GLOBAL_FOLLOW_ON_KEYS: &[&str] = &[
    global_key::ENABLE_REMOTE_MANAGEMENT,
    global_key::REMOTE_MANAGEMENT_ALLOWED,
    global_key::RESPOND_TO_PROBES,
    global_key::PUBLISH_BLACKHOLE,
    global_key::BLACKHOLE_SOURCES,
    global_key::BLACKHOLE_UPDATE_INTERVAL,
];

pub(super) const INTERFACE_FOLLOW_ON_KEYS: &[&str] = &[
    interface_key::BOOTSTRAP_ONLY,
    interface_key::IGNORE_CONFIG_WARNINGS,
];

pub(super) const AUTO_INTERFACE_FOLLOW_ON_KEYS: &[&str] = &[
    interface_key::DISCOVERY_SCOPE,
    interface_key::DISCOVERY_PORT,
    interface_key::DATA_PORT,
    interface_key::DEVICES,
    interface_key::IGNORED_DEVICES,
    interface_key::MULTICAST_ADDRESS_TYPE,
];

pub(super) const DISCOVERY_DETAIL_KEYS: &[&str] = &[
    interface_key::ANNOUNCE_INTERVAL,
    interface_key::DISCOVERY_STAMP_VALUE,
    interface_key::DISCOVERY_NAME,
    interface_key::DISCOVERY_ENCRYPT,
    interface_key::REACHABLE_ON,
    interface_key::PUBLISH_IFAC,
    interface_key::LATITUDE,
    interface_key::LONGITUDE,
    interface_key::HEIGHT,
    interface_key::DISCOVERY_FREQUENCY,
    interface_key::DISCOVERY_BANDWIDTH,
    interface_key::DISCOVERY_MODULATION,
];

pub(super) const SUPPORTED_INTERFACES: &[&str] = &[
    "AutoInterface",
    "TCPClientInterface",
    "TCPServerInterface",
    "UDPInterface",
    "SerialInterface",
    "KISSInterface",
    "AX25KISSInterface",
    "RNodeInterface",
    "PipeInterface",
    "BackboneInterface",
    "BackboneClientInterface",
    "I2PInterface",
];

pub(super) fn interface_key_rule(
    type_name: &str,
    key: &str,
    discoverable: bool,
) -> Option<KeyRule> {
    if let Some(rule) = common_interface_key_rule(key, discoverable) {
        return Some(rule);
    }
    medium_interface_key_rule(type_name, key)
}

fn common_interface_key_rule(key: &str, discoverable: bool) -> Option<KeyRule> {
    match key {
        interface_key::TYPE => Some(KeyRule::Validate(ValueKind::String)),

        interface_key::OUTGOING
        | interface_key::DISCOVERABLE
        | interface_key::DISCOVERY_ENCRYPT
        | interface_key::PUBLISH_IFAC
        | common_key::INGRESS_CONTROL
        | common_key::EGRESS_CONTROL
        | interface_key::BOOTSTRAP_ONLY
        | interface_key::RECURSIVE_PRS
        | interface_key::ANNOUNCES_FROM_INTERNAL
        | interface_key::IGNORE_CONFIG_WARNINGS => Some(KeyRule::Validate(ValueKind::Bool)),

        interface_key::BITRATE => Some(KeyRule::Validate(ValueKind::Bitrate)),

        interface_key::ANNOUNCE_RATE_TARGET
        | interface_key::ANNOUNCE_RATE_GRACE
        | interface_key::ANNOUNCE_RATE_PENALTY => Some(KeyRule::Validate(ValueKind::U64)),

        interface_key::ANNOUNCE_CAP
        | interface_key::LATITUDE
        | interface_key::LONGITUDE
        | interface_key::HEIGHT
        | common_key::IC_BURST_HOLD
        | common_key::IC_BURST_FREQ_NEW
        | common_key::IC_BURST_FREQ
        | common_key::IC_PR_BURST_FREQ_NEW
        | common_key::IC_PR_BURST_FREQ
        | common_key::EC_PR_FREQ
        | common_key::IC_NEW_TIME
        | common_key::IC_BURST_PENALTY
        | common_key::IC_HELD_RELEASE_INTERVAL => Some(KeyRule::Validate(ValueKind::F64)),

        interface_key::IFAC_SIZE | interface_key::DISCOVERY_BANDWIDTH => {
            Some(KeyRule::Validate(ValueKind::U32))
        }

        common_key::IC_MAX_HELD_ANNOUNCES => Some(KeyRule::Validate(ValueKind::I64)),

        interface_key::ANNOUNCE_INTERVAL => {
            Some(discovery_detail_key_rule(discoverable, ValueKind::I64))
        }

        interface_key::DISCOVERY_STAMP_VALUE => Some(discovery_detail_key_rule(
            discoverable,
            ValueKind::StampCost,
        )),

        interface_key::DISCOVERY_FREQUENCY => {
            Some(discovery_detail_key_rule(discoverable, ValueKind::U64))
        }

        interface_key::DISCOVERY_NAME
        | interface_key::REACHABLE_ON
        | interface_key::DISCOVERY_MODULATION => {
            Some(discovery_detail_key_rule(discoverable, ValueKind::String))
        }

        _ => None,
    }
}

fn discovery_detail_key_rule(discoverable: bool, kind: ValueKind) -> KeyRule {
    if discoverable {
        KeyRule::Validate(kind)
    } else {
        KeyRule::Recognized
    }
}

fn medium_interface_key_rule(type_name: &str, key: &str) -> Option<KeyRule> {
    match type_name {
        "AutoInterface" => auto_interface_key_rule(key),
        "TCPClientInterface" => tcp_client_interface_key_rule(key),
        "TCPServerInterface" => tcp_server_interface_key_rule(key),
        "UDPInterface" => udp_interface_key_rule(key),
        "SerialInterface" => serial_line_key_rule(key),
        "KISSInterface" => kiss_interface_key_rule(key),
        "AX25KISSInterface" => ax25_kiss_interface_key_rule(key),
        "RNodeInterface" => rnode_interface_key_rule(key),
        "PipeInterface" => pipe_interface_key_rule(key),
        "BackboneInterface" | "BackboneClientInterface" => backbone_interface_key_rule(key),
        "I2PInterface" => i2p_interface_key_rule(key),
        _ => None,
    }
}

fn auto_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::GROUP_ID
        | interface_key::DISCOVERY_SCOPE
        | interface_key::MULTICAST_ADDRESS_TYPE => Some(KeyRule::Validate(ValueKind::String)),
        interface_key::DISCOVERY_PORT | interface_key::DATA_PORT => {
            Some(KeyRule::Validate(ValueKind::U16))
        }
        interface_key::DEVICES | interface_key::IGNORED_DEVICES => {
            Some(KeyRule::Validate(ValueKind::List))
        }
        _ => None,
    }
}

fn tcp_client_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::TARGET_HOST => Some(KeyRule::Validate(ValueKind::String)),
        interface_key::TARGET_PORT => Some(KeyRule::Validate(ValueKind::U16)),
        interface_key::KISS_FRAMING | interface_key::I2P_TUNNELED => {
            Some(KeyRule::Validate(ValueKind::Bool))
        }
        interface_key::CONNECT_TIMEOUT => Some(KeyRule::Validate(ValueKind::U64)),
        interface_key::MAX_RECONNECT_TRIES => Some(KeyRule::Validate(ValueKind::U32)),
        interface_key::FIXED_MTU => Some(KeyRule::Validate(ValueKind::LinkMtu)),
        _ => None,
    }
}

fn tcp_server_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::LISTEN_IP | interface_key::DEVICE => {
            Some(KeyRule::Validate(ValueKind::String))
        }
        interface_key::LISTEN_PORT | interface_key::PORT => Some(KeyRule::Validate(ValueKind::U16)),
        interface_key::PREFER_IPV6 | interface_key::I2P_TUNNELED | interface_key::KISS_FRAMING => {
            Some(KeyRule::Validate(ValueKind::Bool))
        }
        interface_key::FIXED_MTU => Some(KeyRule::Validate(ValueKind::LinkMtu)),
        _ => None,
    }
}

fn udp_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::LISTEN_IP | interface_key::FORWARD_IP | interface_key::DEVICE => {
            Some(KeyRule::Validate(ValueKind::String))
        }
        interface_key::LISTEN_PORT | interface_key::FORWARD_PORT | interface_key::PORT => {
            Some(KeyRule::Validate(ValueKind::U16))
        }
        _ => None,
    }
}

fn serial_line_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::PORT | interface_key::PARITY => Some(KeyRule::Validate(ValueKind::String)),
        interface_key::SPEED => Some(KeyRule::Validate(ValueKind::U32)),
        interface_key::DATABITS | interface_key::STOPBITS => Some(KeyRule::Validate(ValueKind::U8)),
        _ => None,
    }
}

fn kiss_interface_key_rule(key: &str) -> Option<KeyRule> {
    if let Some(rule) = serial_line_key_rule(key) {
        return Some(rule);
    }
    if let Some(rule) = kiss_modem_key_rule(key) {
        return Some(rule);
    }
    match key {
        interface_key::ID_CALLSIGN => Some(KeyRule::Validate(ValueKind::String)),
        interface_key::ID_INTERVAL => Some(KeyRule::Validate(ValueKind::U64)),
        _ => None,
    }
}

fn ax25_kiss_interface_key_rule(key: &str) -> Option<KeyRule> {
    if let Some(rule) = serial_line_key_rule(key) {
        return Some(rule);
    }
    if let Some(rule) = kiss_modem_key_rule(key) {
        return Some(rule);
    }
    match key {
        interface_key::CALLSIGN => Some(KeyRule::Validate(ValueKind::String)),
        interface_key::SSID => Some(KeyRule::Validate(ValueKind::U8)),
        _ => None,
    }
}

fn kiss_modem_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::FLOW_CONTROL => Some(KeyRule::Validate(ValueKind::Bool)),
        interface_key::PREAMBLE
        | interface_key::TXTAIL
        | interface_key::PERSISTENCE
        | interface_key::SLOTTIME => Some(KeyRule::Validate(ValueKind::U32)),
        _ => None,
    }
}

fn rnode_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::PORT | interface_key::ID_CALLSIGN => {
            Some(KeyRule::Validate(ValueKind::String))
        }
        interface_key::FREQUENCY => Some(KeyRule::Validate(ValueKind::U64)),
        interface_key::BANDWIDTH => Some(KeyRule::Validate(ValueKind::U32)),
        interface_key::SPREADINGFACTOR | interface_key::CODINGRATE => {
            Some(KeyRule::Validate(ValueKind::U8))
        }
        interface_key::TXPOWER => Some(KeyRule::Validate(ValueKind::I16)),
        interface_key::FLOW_CONTROL => Some(KeyRule::Validate(ValueKind::Bool)),
        interface_key::ID_INTERVAL => Some(KeyRule::Validate(ValueKind::U64)),
        interface_key::AIRTIME_LIMIT_SHORT | interface_key::AIRTIME_LIMIT_LONG => {
            Some(KeyRule::Validate(ValueKind::F64))
        }
        _ => None,
    }
}

fn pipe_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::COMMAND => Some(KeyRule::Validate(ValueKind::String)),
        interface_key::RESPAWN_DELAY => Some(KeyRule::Validate(ValueKind::F64)),
        _ => None,
    }
}

fn backbone_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::LISTEN_IP
        | interface_key::TARGET_HOST
        | interface_key::DEVICE
        | interface_key::REMOTE
        | interface_key::LISTEN_ON => Some(KeyRule::Validate(ValueKind::String)),
        interface_key::LISTEN_PORT | interface_key::TARGET_PORT | interface_key::PORT => {
            Some(KeyRule::Validate(ValueKind::U16))
        }
        interface_key::PREFER_IPV6 | interface_key::I2P_TUNNELED => {
            Some(KeyRule::Validate(ValueKind::Bool))
        }
        interface_key::CONNECT_TIMEOUT => Some(KeyRule::Validate(ValueKind::U64)),
        interface_key::MAX_RECONNECT_TRIES => Some(KeyRule::Validate(ValueKind::U32)),
        _ => None,
    }
}

fn i2p_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::PEERS => Some(KeyRule::Validate(ValueKind::I2pPeers)),
        interface_key::CONNECTABLE => Some(KeyRule::Validate(ValueKind::Bool)),
        _ => None,
    }
}

pub(super) fn known_interface_keys(type_name: &str) -> Vec<&'static str> {
    let mut known = interface_key::COMMON.to_vec();
    let medium = match type_name {
        "AutoInterface" => interface_key::AUTO,
        "TCPClientInterface" => interface_key::TCP_CLIENT,
        "TCPServerInterface" => interface_key::TCP_SERVER,
        "UDPInterface" => interface_key::UDP,
        "SerialInterface" => interface_key::SERIAL,
        "KISSInterface" => interface_key::KISS,
        "AX25KISSInterface" => interface_key::AX25_KISS,
        "RNodeInterface" => interface_key::RNODE,
        "PipeInterface" => interface_key::PIPE,
        "BackboneInterface" | "BackboneClientInterface" => interface_key::BACKBONE,
        "I2PInterface" => interface_key::I2P,
        _ => &[],
    };
    known.extend_from_slice(medium);
    known
}
use super::keys::{
    common as common_key, global as global_key, interface as interface_key, logging as logging_key,
};
