use super::interface_type::InterfaceType;
use super::keys::{
    common as common_key, global as global_key, interface as interface_key, logging as logging_key,
    prns as prns_key,
};

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
    SecondsOrOff,
    U32,
    U16,
    NonZeroU16,
    U8,
    I16,
    I64,
    F64,
    StampCost,
    IdentityHashes,
    LogLevel,
    SharedInstanceType,
    HexBytes,
    RnodeMultiVport,
    RnodeMultiFrequency,
    RnodeMultiTxPower,
    BlackholeUpdateInterval,
    WebSocketFramingSelection,
    ByteQuantity,
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
            ValueKind::SecondsOrOff => {
                "off, no, false, or a non-negative integer number of seconds"
            }
            ValueKind::U32 => "an integer from 0 through 4294967295",
            ValueKind::U16 => "an integer from 0 through 65535",
            ValueKind::NonZeroU16 => "an integer from 1 through 65535",
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
            ValueKind::RnodeMultiVport => "an integer from 0 through 10",
            ValueKind::RnodeMultiFrequency => {
                "137000000 through 1000000000 Hz, or 2200000000 through 2600000000 Hz"
            }
            ValueKind::RnodeMultiTxPower => "an integer from -9 through 37 dBm",
            ValueKind::BlackholeUpdateInterval => {
                "a finite number of minutes representable by the host; values below 2 use 2 minutes"
            }
            ValueKind::WebSocketFramingSelection => "one of auto, raw, hdlc, or kiss",
            ValueKind::ByteQuantity => {
                "a non-negative integer optionally followed by B, KiB, MiB, or GiB, whose byte total is representable by this host"
            }
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
            ValueKind::SecondsOrOff => "3600",
            ValueKind::U16 => "4242",
            ValueKind::NonZeroU16 => "4242",
            ValueKind::U8 => "8",
            ValueKind::I16 | ValueKind::I64 => "0",
            ValueKind::F64 => "1.0",
            ValueKind::StampCost => "0",
            ValueKind::IdentityHashes => "00112233445566778899aabbccddeeff",
            ValueKind::LogLevel => "4",
            ValueKind::SharedInstanceType => "tcp",
            ValueKind::HexBytes => "00112233aabbccdd",
            ValueKind::RnodeMultiVport => "0",
            ValueKind::RnodeMultiFrequency => "868000000",
            ValueKind::RnodeMultiTxPower => "7",
            ValueKind::BlackholeUpdateInterval => "60.0",
            ValueKind::WebSocketFramingSelection => "auto",
            ValueKind::ByteQuantity => "64 MiB",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum KeyRule {
    Applied(ValueKind),
    FollowOn(ValueKind),
    DiscoveryOnly(ValueKind),
}

use KeyRule::{Applied, DiscoveryOnly, FollowOn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyApplication {
    Applied,
    FollowOn,
    DiscoveryOnly,
}

impl KeyRule {
    pub(super) const fn value_kind(self) -> ValueKind {
        match self {
            Self::Applied(kind) | Self::FollowOn(kind) | Self::DiscoveryOnly(kind) => kind,
        }
    }

    pub(super) const fn application(self) -> KeyApplication {
        match self {
            Self::Applied(_) => KeyApplication::Applied,
            Self::FollowOn(_) => KeyApplication::FollowOn,
            Self::DiscoveryOnly(_) => KeyApplication::DiscoveryOnly,
        }
    }

    pub(super) const fn validation_kind(self, discovery_enabled: bool) -> Option<ValueKind> {
        match self {
            Self::DiscoveryOnly(_) if !discovery_enabled => None,
            _ => Some(self.value_kind()),
        }
    }
}

pub(super) const GLOBAL_RULES: &[(&str, KeyRule)] = &[
    (global_key::SHARE_INSTANCE, Applied(ValueKind::Bool)),
    (global_key::INSTANCE_NAME, Applied(ValueKind::String)),
    (
        global_key::SHARED_INSTANCE_TYPE,
        Applied(ValueKind::SharedInstanceType),
    ),
    (global_key::SHARED_INSTANCE_PORT, Applied(ValueKind::U16)),
    (global_key::INSTANCE_CONTROL_PORT, Applied(ValueKind::U16)),
    (global_key::RPC_KEY, Applied(ValueKind::HexBytes)),
    (global_key::ENABLE_TRANSPORT, Applied(ValueKind::Bool)),
    (
        global_key::STATIC_TRANSPORT_IDENTITY,
        Applied(ValueKind::Bool),
    ),
    (global_key::LOCAL_HOPS_DELTA, Applied(ValueKind::Bool)),
    (global_key::NETWORK_IDENTITY, Applied(ValueKind::String)),
    (global_key::LINK_MTU_DISCOVERY, Applied(ValueKind::Bool)),
    (
        global_key::ENABLE_REMOTE_MANAGEMENT,
        Applied(ValueKind::Bool),
    ),
    (
        global_key::REMOTE_MANAGEMENT_ALLOWED,
        Applied(ValueKind::IdentityHashes),
    ),
    (global_key::RESPOND_TO_PROBES, Applied(ValueKind::Bool)),
    (
        global_key::FORCE_SHARED_INSTANCE_BITRATE,
        Applied(ValueKind::Bitrate),
    ),
    (
        global_key::PANIC_ON_INTERFACE_ERROR,
        Applied(ValueKind::Bool),
    ),
    (global_key::USE_IMPLICIT_PROOF, Applied(ValueKind::Bool)),
    (global_key::DISCOVER_INTERFACES, Applied(ValueKind::Bool)),
    (
        global_key::REQUIRED_DISCOVERY_VALUE,
        Applied(ValueKind::StampCost),
    ),
    (global_key::PUBLISH_BLACKHOLE, Applied(ValueKind::Bool)),
    (
        global_key::BLACKHOLE_SOURCES,
        Applied(ValueKind::IdentityHashes),
    ),
    (
        global_key::BLACKHOLE_UPDATE_INTERVAL,
        Applied(ValueKind::BlackholeUpdateInterval),
    ),
    (
        global_key::INTERFACE_DISCOVERY_SOURCES,
        Applied(ValueKind::IdentityHashes),
    ),
    (
        global_key::AUTOCONNECT_DISCOVERED_INTERFACES,
        Applied(ValueKind::I64),
    ),
    (
        global_key::AUTOCONNECT_INTERFACE_GRAVITY,
        Applied(ValueKind::I64),
    ),
    (
        global_key::AUTOCONNECT_ANNOUNCES_TO_INTERNAL,
        Applied(ValueKind::Bool),
    ),
    (global_key::DEFAULT_GRAVITY, Applied(ValueKind::I64)),
    (
        global_key::DEFAULT_AR_TARGET,
        Applied(ValueKind::SecondsOrOff),
    ),
    (global_key::DEFAULT_AR_PENALTY, Applied(ValueKind::I64)),
    (global_key::DEFAULT_AR_GRACE, Applied(ValueKind::I64)),
    (common_key::IC_MAX_HELD_ANNOUNCES, Applied(ValueKind::I64)),
    (common_key::IC_BURST_HOLD, Applied(ValueKind::F64)),
    (common_key::IC_BURST_FREQ_NEW, Applied(ValueKind::F64)),
    (common_key::IC_BURST_FREQ, Applied(ValueKind::F64)),
    (common_key::IC_PR_BURST_FREQ_NEW, Applied(ValueKind::F64)),
    (common_key::IC_PR_BURST_FREQ, Applied(ValueKind::F64)),
    (common_key::EC_PR_FREQ, Applied(ValueKind::F64)),
    (common_key::EGRESS_CONTROL, Applied(ValueKind::Bool)),
    (common_key::IC_NEW_TIME, Applied(ValueKind::F64)),
    (common_key::IC_BURST_PENALTY, Applied(ValueKind::F64)),
    (
        common_key::IC_HELD_RELEASE_INTERVAL,
        Applied(ValueKind::F64),
    ),
];

pub(super) const LOGGING_RULES: &[(&str, KeyRule)] = &[
    (logging_key::LEVEL, Applied(ValueKind::LogLevel)),
    (logging_key::TIMESTAMPS, Applied(ValueKind::Bool)),
];

pub(super) const PRNS_RULES: &[(&str, KeyRule)] = &[
    (prns_key::RESOURCE_MEM_IN, Applied(ValueKind::ByteQuantity)),
    (prns_key::RESOURCE_MEM_OUT, Applied(ValueKind::ByteQuantity)),
];

pub(super) const SUPPORTED_INTERFACES: &[&str] = InterfaceType::CANONICAL_NAMES;

pub(super) fn interface_key_rule(type_name: &str, key: &str) -> Option<KeyRule> {
    if let Some(rule) = common_interface_key_rule(key) {
        return Some(rule);
    }
    medium_interface_key_rule(type_name, key)
}

fn common_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::TYPE => Some(Applied(ValueKind::String)),

        interface_key::OUTGOING
        | interface_key::DISCOVERABLE
        | common_key::INGRESS_CONTROL
        | common_key::EGRESS_CONTROL
        | interface_key::RECURSIVE_PRS
        | interface_key::ANNOUNCES_FROM_INTERNAL
        | interface_key::ANNOUNCES_TO_INTERNAL => Some(Applied(ValueKind::Bool)),

        interface_key::BOOTSTRAP_ONLY => Some(Applied(ValueKind::Bool)),
        interface_key::IGNORE_CONFIG_WARNINGS => Some(FollowOn(ValueKind::Bool)),

        interface_key::BITRATE => Some(Applied(ValueKind::Bitrate)),

        interface_key::GRAVITY => Some(Applied(ValueKind::I64)),

        interface_key::ANNOUNCE_RATE_TARGET => Some(Applied(ValueKind::SecondsOrOff)),

        interface_key::ANNOUNCE_RATE_GRACE | interface_key::ANNOUNCE_RATE_PENALTY => {
            Some(Applied(ValueKind::U64))
        }

        interface_key::ANNOUNCE_CAP
        | common_key::IC_BURST_HOLD
        | common_key::IC_BURST_FREQ_NEW
        | common_key::IC_BURST_FREQ
        | common_key::IC_PR_BURST_FREQ_NEW
        | common_key::IC_PR_BURST_FREQ
        | common_key::EC_PR_FREQ
        | common_key::IC_NEW_TIME
        | common_key::IC_BURST_PENALTY
        | common_key::IC_HELD_RELEASE_INTERVAL => Some(Applied(ValueKind::F64)),

        interface_key::IFAC_SIZE => Some(Applied(ValueKind::U32)),

        common_key::IC_MAX_HELD_ANNOUNCES => Some(Applied(ValueKind::I64)),

        interface_key::ANNOUNCE_INTERVAL => Some(discovery_detail_key_rule(ValueKind::I64)),

        interface_key::DISCOVERY_STAMP_VALUE => {
            Some(discovery_detail_key_rule(ValueKind::StampCost))
        }

        interface_key::DISCOVERY_ENCRYPT | interface_key::PUBLISH_IFAC => {
            Some(discovery_detail_key_rule(ValueKind::Bool))
        }

        interface_key::REACHABLE_PORT => Some(discovery_detail_key_rule(ValueKind::NonZeroU16)),

        interface_key::LATITUDE | interface_key::LONGITUDE | interface_key::HEIGHT => {
            Some(discovery_detail_key_rule(ValueKind::F64))
        }

        interface_key::DISCOVERY_FREQUENCY => Some(discovery_detail_key_rule(ValueKind::U64)),

        interface_key::DISCOVERY_BANDWIDTH => Some(discovery_detail_key_rule(ValueKind::U32)),

        interface_key::DISCOVERY_NAME
        | interface_key::REACHABLE_ON
        | interface_key::DISCOVERY_MODULATION => Some(discovery_detail_key_rule(ValueKind::String)),

        _ => None,
    }
}

fn discovery_detail_key_rule(kind: ValueKind) -> KeyRule {
    DiscoveryOnly(kind)
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
        "RNodeMultiInterface" => rnode_multi_interface_key_rule(key),
        "PipeInterface" => pipe_interface_key_rule(key),
        "BackboneInterface" | "BackboneClientInterface" => backbone_interface_key_rule(key),
        "I2PInterface" => i2p_interface_key_rule(key),
        "WeaveInterface" => weave_interface_key_rule(key),
        "PrnsUsbAuto" | "PrnsBluetoothAuto" => None,
        "PrnsWebSocketClient" => prns_websocket_client_key_rule(key),
        "PrnsWebSocketServer" => prns_websocket_server_key_rule(key),
        _ => None,
    }
}

fn auto_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::GROUP_ID => Some(Applied(ValueKind::String)),
        interface_key::DISCOVERY_SCOPE | interface_key::MULTICAST_ADDRESS_TYPE => {
            Some(Applied(ValueKind::String))
        }
        interface_key::DISCOVERY_PORT | interface_key::DATA_PORT => Some(Applied(ValueKind::U16)),
        interface_key::DEVICES | interface_key::IGNORED_DEVICES => Some(Applied(ValueKind::List)),
        _ => None,
    }
}

fn tcp_client_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::TARGET_HOST => Some(Applied(ValueKind::String)),
        interface_key::TARGET_PORT => Some(Applied(ValueKind::U16)),
        interface_key::KISS_FRAMING | interface_key::I2P_TUNNELED => Some(Applied(ValueKind::Bool)),
        interface_key::CONNECT_TIMEOUT => Some(Applied(ValueKind::U64)),
        interface_key::MAX_RECONNECT_TRIES => Some(Applied(ValueKind::U32)),
        interface_key::FIXED_MTU => Some(Applied(ValueKind::LinkMtu)),
        _ => None,
    }
}

fn tcp_server_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::LISTEN_IP | interface_key::DEVICE => Some(Applied(ValueKind::String)),
        interface_key::LISTEN_PORT | interface_key::PORT => Some(Applied(ValueKind::U16)),
        interface_key::PREFER_IPV6 | interface_key::I2P_TUNNELED | interface_key::KISS_FRAMING => {
            Some(Applied(ValueKind::Bool))
        }
        interface_key::FIXED_MTU => Some(Applied(ValueKind::LinkMtu)),
        _ => None,
    }
}

fn udp_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::LISTEN_IP | interface_key::FORWARD_IP | interface_key::DEVICE => {
            Some(Applied(ValueKind::String))
        }
        interface_key::LISTEN_PORT | interface_key::FORWARD_PORT | interface_key::PORT => {
            Some(Applied(ValueKind::U16))
        }
        _ => None,
    }
}

fn serial_line_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::PORT | interface_key::PARITY => Some(Applied(ValueKind::String)),
        interface_key::SPEED => Some(Applied(ValueKind::U32)),
        interface_key::DATABITS | interface_key::STOPBITS => Some(Applied(ValueKind::U8)),
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
        interface_key::ID_CALLSIGN => Some(Applied(ValueKind::String)),
        interface_key::ID_INTERVAL => Some(Applied(ValueKind::U64)),
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
        interface_key::CALLSIGN => Some(Applied(ValueKind::String)),
        interface_key::SSID => Some(Applied(ValueKind::U8)),
        _ => None,
    }
}

fn kiss_modem_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::FLOW_CONTROL => Some(Applied(ValueKind::Bool)),
        interface_key::PREAMBLE
        | interface_key::TXTAIL
        | interface_key::PERSISTENCE
        | interface_key::SLOTTIME => Some(Applied(ValueKind::U32)),
        _ => None,
    }
}

fn rnode_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::PORT | interface_key::ID_CALLSIGN => Some(Applied(ValueKind::String)),
        interface_key::FREQUENCY => Some(Applied(ValueKind::U64)),
        interface_key::BANDWIDTH => Some(Applied(ValueKind::U32)),
        interface_key::SPREADINGFACTOR | interface_key::CODINGRATE => Some(Applied(ValueKind::U8)),
        interface_key::TXPOWER => Some(Applied(ValueKind::I16)),
        interface_key::FLOW_CONTROL => Some(Applied(ValueKind::Bool)),
        interface_key::ID_INTERVAL => Some(Applied(ValueKind::U64)),
        interface_key::AIRTIME_LIMIT_SHORT | interface_key::AIRTIME_LIMIT_LONG => {
            Some(Applied(ValueKind::F64))
        }
        _ => None,
    }
}

fn rnode_multi_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::PORT | interface_key::ID_CALLSIGN => Some(Applied(ValueKind::String)),
        interface_key::ID_INTERVAL => Some(Applied(ValueKind::U64)),
        _ => None,
    }
}

pub(super) fn rnode_multi_subinterface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::INTERFACE_ENABLED | interface_key::ENABLED => Some(Applied(ValueKind::Bool)),
        interface_key::VPORT => Some(Applied(ValueKind::RnodeMultiVport)),
        interface_key::FREQUENCY => Some(Applied(ValueKind::RnodeMultiFrequency)),
        interface_key::TXPOWER => Some(Applied(ValueKind::RnodeMultiTxPower)),
        interface_key::BANDWIDTH => Some(Applied(ValueKind::U32)),
        interface_key::SPREADINGFACTOR | interface_key::CODINGRATE => Some(Applied(ValueKind::U8)),
        interface_key::FLOW_CONTROL | interface_key::OUTGOING => Some(Applied(ValueKind::Bool)),
        interface_key::AIRTIME_LIMIT_SHORT | interface_key::AIRTIME_LIMIT_LONG => {
            Some(Applied(ValueKind::F64))
        }
        _ => None,
    }
}

fn pipe_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::COMMAND => Some(Applied(ValueKind::String)),
        interface_key::RESPAWN_DELAY => Some(Applied(ValueKind::F64)),
        _ => None,
    }
}

fn backbone_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::LISTEN_IP
        | interface_key::TARGET_HOST
        | interface_key::DEVICE
        | interface_key::REMOTE
        | interface_key::LISTEN_ON => Some(Applied(ValueKind::String)),
        interface_key::LISTEN_PORT | interface_key::TARGET_PORT | interface_key::PORT => {
            Some(Applied(ValueKind::U16))
        }
        interface_key::PREFER_IPV6 | interface_key::I2P_TUNNELED => Some(Applied(ValueKind::Bool)),
        interface_key::CONNECT_TIMEOUT => Some(Applied(ValueKind::U64)),
        interface_key::MAX_RECONNECT_TRIES => Some(Applied(ValueKind::U32)),
        _ => None,
    }
}

fn i2p_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::PEERS => Some(Applied(ValueKind::I2pPeers)),
        interface_key::CONNECTABLE => Some(Applied(ValueKind::Bool)),
        _ => None,
    }
}

fn weave_interface_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::PORT => Some(Applied(ValueKind::String)),
        _ => None,
    }
}

fn prns_websocket_client_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::TARGET => Some(Applied(ValueKind::String)),
        interface_key::FRAMING => Some(Applied(ValueKind::WebSocketFramingSelection)),
        _ => None,
    }
}

fn prns_websocket_server_key_rule(key: &str) -> Option<KeyRule> {
    match key {
        interface_key::LISTEN_IP | interface_key::DEVICE => Some(Applied(ValueKind::String)),
        interface_key::LISTEN_PORT | interface_key::PORT => Some(Applied(ValueKind::U16)),
        interface_key::PREFER_IPV6 => Some(Applied(ValueKind::Bool)),
        interface_key::FRAMING => Some(Applied(ValueKind::WebSocketFramingSelection)),
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
        "RNodeMultiInterface" => interface_key::RNODE_MULTI,
        "PipeInterface" => interface_key::PIPE,
        "BackboneInterface" | "BackboneClientInterface" => interface_key::BACKBONE,
        "I2PInterface" => interface_key::I2P,
        "WeaveInterface" => interface_key::WEAVE,
        "PrnsUsbAuto" | "PrnsBluetoothAuto" => &[],
        "PrnsWebSocketClient" => interface_key::PRNS_WEBSOCKET_CLIENT,
        "PrnsWebSocketServer" => interface_key::PRNS_WEBSOCKET_SERVER,
        _ => &[],
    };
    known.extend_from_slice(medium);
    known
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_non_alias_interface_key_has_an_application_rule() {
        for type_name in SUPPORTED_INTERFACES {
            for key in known_interface_keys(type_name) {
                if interface_key::ALIASES.contains(&key) {
                    continue;
                }
                assert!(
                    interface_key_rule(type_name, key).is_some(),
                    "{type_name} key {key:?} has no application rule"
                );
            }
        }
    }

    #[test]
    fn every_rnode_multi_subinterface_key_has_an_application_rule() {
        for key in interface_key::RNODE_MULTI_SUBINTERFACE {
            assert!(
                rnode_multi_subinterface_key_rule(key).is_some(),
                "RNodeMulti subinterface key {key:?} has no application rule"
            );
        }
    }

    #[test]
    fn application_status_is_attached_to_the_authoritative_key_rule() {
        let global_application = |selected| {
            GLOBAL_RULES
                .iter()
                .find(|(key, _)| *key == selected)
                .map(|(_, rule)| rule.application())
        };
        assert_eq!(
            global_application(global_key::RESPOND_TO_PROBES),
            Some(KeyApplication::Applied)
        );
        assert_eq!(
            global_application(global_key::PUBLISH_BLACKHOLE),
            Some(KeyApplication::Applied)
        );
        assert_eq!(
            interface_key_rule("AutoInterface", interface_key::DISCOVERY_PORT)
                .map(KeyRule::application),
            Some(KeyApplication::Applied)
        );
        assert_eq!(
            interface_key_rule("TCPClientInterface", interface_key::BOOTSTRAP_ONLY)
                .map(KeyRule::application),
            Some(KeyApplication::Applied)
        );
        assert_eq!(
            interface_key_rule("TCPClientInterface", interface_key::ANNOUNCE_INTERVAL)
                .map(KeyRule::application),
            Some(KeyApplication::DiscoveryOnly)
        );
        assert_eq!(
            interface_key_rule("TCPClientInterface", interface_key::TARGET_HOST)
                .map(KeyRule::application),
            Some(KeyApplication::Applied)
        );
    }

    #[test]
    fn discovery_only_rules_validate_only_when_publication_is_enabled() {
        for key in [
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
        ] {
            let rule = interface_key_rule("TCPClientInterface", key)
                .unwrap_or_else(|| panic!("{key} must have a rule"));
            assert_eq!(rule.application(), KeyApplication::DiscoveryOnly);
            assert!(rule.validation_kind(false).is_none());
            assert!(rule.validation_kind(true).is_some());
        }
    }
}
