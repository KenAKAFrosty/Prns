#[derive(Debug, Clone, Copy)]
pub(super) enum ValueKind {
    Bool,
    Mode,
    String,
    List,
    U64,
    U32,
    U16,
    U8,
    I16,
    I64,
    Usize,
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
                "full, access_point, pointtopoint, roaming, boundary, gateway, or their stock aliases"
            }
            ValueKind::String => "one scalar value",
            ValueKind::List => "one value or a comma-separated list",
            ValueKind::U64 => "a non-negative integer",
            ValueKind::U32 => "an integer from 0 through 4294967295",
            ValueKind::U16 => "an integer from 0 through 65535",
            ValueKind::U8 => "an integer from 0 through 255",
            ValueKind::I16 => "an integer from -32768 through 32767",
            ValueKind::I64 => "a signed 64-bit integer",
            ValueKind::Usize => "a non-negative integer supported by this platform",
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
            ValueKind::U64 | ValueKind::U32 | ValueKind::Usize => "1000000",
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
    ("share_instance", ValueKind::Bool),
    ("instance_name", ValueKind::String),
    ("shared_instance_type", ValueKind::SharedInstanceType),
    ("shared_instance_port", ValueKind::U16),
    ("instance_control_port", ValueKind::U16),
    ("rpc_key", ValueKind::HexBytes),
    ("enable_transport", ValueKind::Bool),
    ("static_transport_identity", ValueKind::Bool),
    ("local_hops_delta", ValueKind::Bool),
    ("network_identity", ValueKind::String),
    ("link_mtu_discovery", ValueKind::Bool),
    ("enable_remote_management", ValueKind::Bool),
    ("remote_management_allowed", ValueKind::IdentityHashes),
    ("respond_to_probes", ValueKind::Bool),
    ("force_shared_instance_bitrate", ValueKind::U64),
    ("panic_on_interface_error", ValueKind::Bool),
    ("use_implicit_proof", ValueKind::Bool),
    ("discover_interfaces", ValueKind::Bool),
    ("required_discovery_value", ValueKind::StampCost),
    ("publish_blackhole", ValueKind::Bool),
    ("blackhole_sources", ValueKind::IdentityHashes),
    ("blackhole_update_interval", ValueKind::F64),
    ("interface_discovery_sources", ValueKind::IdentityHashes),
    ("autoconnect_discovered_interfaces", ValueKind::I64),
    ("default_ar_target", ValueKind::I64),
    ("default_ar_penalty", ValueKind::I64),
    ("default_ar_grace", ValueKind::I64),
    ("ic_max_held_announces", ValueKind::I64),
    ("ic_burst_hold", ValueKind::F64),
    ("ic_burst_freq_new", ValueKind::F64),
    ("ic_burst_freq", ValueKind::F64),
    ("ic_pr_burst_freq_new", ValueKind::F64),
    ("ic_pr_burst_freq", ValueKind::F64),
    ("ec_pr_freq", ValueKind::F64),
    ("egress_control", ValueKind::Bool),
    ("ic_new_time", ValueKind::F64),
    ("ic_burst_penalty", ValueKind::F64),
    ("ic_held_release_interval", ValueKind::F64),
];

pub(super) const LOGGING_RULES: &[(&str, ValueKind)] = &[
    ("loglevel", ValueKind::LogLevel),
    ("logtimestamps", ValueKind::Bool),
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
];

pub(super) fn interface_key_rule(
    type_name: &str,
    key: &str,
    discoverable: bool,
) -> Option<KeyRule> {
    let common = match key {
        "type" => Some(KeyRule::Validate(ValueKind::String)),
        "outgoing"
        | "discoverable"
        | "discovery_encrypt"
        | "publish_ifac"
        | "ingress_control"
        | "egress_control"
        | "bootstrap_only"
        | "recursive_prs"
        | "announces_from_internal"
        | "ignore_config_warnings" => Some(KeyRule::Validate(ValueKind::Bool)),
        "bitrate" | "announce_rate_target" | "announce_rate_grace" | "announce_rate_penalty" => {
            Some(KeyRule::Validate(ValueKind::U64))
        }
        "announce_cap"
        | "latitude"
        | "longitude"
        | "height"
        | "ic_burst_hold"
        | "ic_burst_freq_new"
        | "ic_burst_freq"
        | "ic_pr_burst_freq_new"
        | "ic_pr_burst_freq"
        | "ec_pr_freq"
        | "ic_new_time"
        | "ic_burst_penalty"
        | "ic_held_release_interval" => Some(KeyRule::Validate(ValueKind::F64)),
        "ifac_size" | "discovery_bandwidth" => Some(KeyRule::Validate(ValueKind::U32)),
        "ic_max_held_announces" => Some(KeyRule::Validate(ValueKind::I64)),
        "announce_interval" => discoverable
            .then_some(KeyRule::Validate(ValueKind::I64))
            .or(Some(KeyRule::Recognized)),
        "discovery_stamp_value" => discoverable
            .then_some(KeyRule::Validate(ValueKind::StampCost))
            .or(Some(KeyRule::Recognized)),
        "discovery_frequency" => discoverable
            .then_some(KeyRule::Validate(ValueKind::U64))
            .or(Some(KeyRule::Recognized)),
        "discovery_name" | "reachable_on" | "discovery_modulation" => discoverable
            .then_some(KeyRule::Validate(ValueKind::String))
            .or(Some(KeyRule::Recognized)),
        _ => None,
    };
    if common.is_some() {
        return common;
    }
    match (type_name, key) {
        ("AutoInterface", "group_id" | "discovery_scope" | "multicast_address_type") => {
            Some(KeyRule::Validate(ValueKind::String))
        }
        ("AutoInterface", "discovery_port" | "data_port") => {
            Some(KeyRule::Validate(ValueKind::U16))
        }
        ("AutoInterface", "devices" | "ignored_devices") => {
            Some(KeyRule::Validate(ValueKind::List))
        }
        ("TCPClientInterface", "target_host") => Some(KeyRule::Validate(ValueKind::String)),
        ("TCPClientInterface", "target_port") => Some(KeyRule::Validate(ValueKind::U16)),
        ("TCPClientInterface", "kiss_framing" | "i2p_tunneled") => {
            Some(KeyRule::Validate(ValueKind::Bool))
        }
        ("TCPClientInterface", "connect_timeout") => Some(KeyRule::Validate(ValueKind::U64)),
        ("TCPClientInterface", "max_reconnect_tries") => Some(KeyRule::Validate(ValueKind::U32)),
        ("TCPClientInterface", "fixed_mtu") => Some(KeyRule::Validate(ValueKind::Usize)),
        ("TCPServerInterface", "listen_ip" | "device") => {
            Some(KeyRule::Validate(ValueKind::String))
        }
        ("TCPServerInterface", "listen_port" | "port") => Some(KeyRule::Validate(ValueKind::U16)),
        ("TCPServerInterface", "prefer_ipv6" | "i2p_tunneled" | "kiss_framing") => {
            Some(KeyRule::Validate(ValueKind::Bool))
        }
        ("TCPServerInterface", "fixed_mtu") => Some(KeyRule::Validate(ValueKind::Usize)),
        ("UDPInterface", "listen_ip" | "forward_ip" | "device") => {
            Some(KeyRule::Validate(ValueKind::String))
        }
        ("UDPInterface", "listen_port" | "forward_port" | "port") => {
            Some(KeyRule::Validate(ValueKind::U16))
        }
        ("SerialInterface" | "KISSInterface" | "AX25KISSInterface", "port" | "parity") => {
            Some(KeyRule::Validate(ValueKind::String))
        }
        ("SerialInterface" | "KISSInterface" | "AX25KISSInterface", "speed") => {
            Some(KeyRule::Validate(ValueKind::U32))
        }
        ("SerialInterface" | "KISSInterface" | "AX25KISSInterface", "databits" | "stopbits") => {
            Some(KeyRule::Validate(ValueKind::U8))
        }
        ("KISSInterface" | "AX25KISSInterface", "flow_control") => {
            Some(KeyRule::Validate(ValueKind::Bool))
        }
        (
            "KISSInterface" | "AX25KISSInterface",
            "preamble" | "txtail" | "persistence" | "slottime",
        ) => Some(KeyRule::Validate(ValueKind::U32)),
        ("KISSInterface", "id_callsign") => Some(KeyRule::Validate(ValueKind::String)),
        ("KISSInterface", "id_interval") => Some(KeyRule::Validate(ValueKind::U64)),
        ("AX25KISSInterface", "callsign") => Some(KeyRule::Validate(ValueKind::String)),
        ("AX25KISSInterface", "ssid") => Some(KeyRule::Validate(ValueKind::U8)),
        ("RNodeInterface", "port" | "id_callsign") => Some(KeyRule::Validate(ValueKind::String)),
        ("RNodeInterface", "frequency") => Some(KeyRule::Validate(ValueKind::U64)),
        ("RNodeInterface", "bandwidth") => Some(KeyRule::Validate(ValueKind::U32)),
        ("RNodeInterface", "spreadingfactor" | "codingrate") => {
            Some(KeyRule::Validate(ValueKind::U8))
        }
        ("RNodeInterface", "txpower") => Some(KeyRule::Validate(ValueKind::I16)),
        ("RNodeInterface", "flow_control") => Some(KeyRule::Validate(ValueKind::Bool)),
        ("RNodeInterface", "id_interval") => Some(KeyRule::Validate(ValueKind::U64)),
        ("RNodeInterface", "airtime_limit_short" | "airtime_limit_long") => {
            Some(KeyRule::Validate(ValueKind::F64))
        }
        ("PipeInterface", "command") => Some(KeyRule::Validate(ValueKind::String)),
        ("PipeInterface", "respawn_delay") => Some(KeyRule::Validate(ValueKind::F64)),
        (
            "BackboneInterface" | "BackboneClientInterface",
            "listen_ip" | "target_host" | "device" | "remote" | "listen_on",
        ) => Some(KeyRule::Validate(ValueKind::String)),
        (
            "BackboneInterface" | "BackboneClientInterface",
            "listen_port" | "target_port" | "port",
        ) => Some(KeyRule::Validate(ValueKind::U16)),
        ("BackboneInterface" | "BackboneClientInterface", "prefer_ipv6" | "i2p_tunneled") => {
            Some(KeyRule::Validate(ValueKind::Bool))
        }
        ("BackboneInterface" | "BackboneClientInterface", "connect_timeout") => {
            Some(KeyRule::Validate(ValueKind::U64))
        }
        ("BackboneInterface" | "BackboneClientInterface", "max_reconnect_tries") => {
            Some(KeyRule::Validate(ValueKind::U32))
        }
        _ => None,
    }
}

pub(super) fn known_interface_keys(type_name: &str) -> Vec<&'static str> {
    let mut known = vec![
        "type",
        "interface_enabled",
        "enabled",
        "interface_mode",
        "mode",
        "outgoing",
        "bitrate",
        "announce_cap",
        "announce_rate_target",
        "announce_rate_grace",
        "announce_rate_penalty",
        "network_name",
        "networkname",
        "pass_phrase",
        "passphrase",
        "ifac_size",
        "discoverable",
        "announce_interval",
        "discovery_stamp_value",
        "discovery_name",
        "discovery_encrypt",
        "reachable_on",
        "publish_ifac",
        "latitude",
        "longitude",
        "height",
        "discovery_frequency",
        "discovery_bandwidth",
        "discovery_modulation",
        "ingress_control",
        "egress_control",
        "ic_max_held_announces",
        "ic_burst_hold",
        "ic_burst_freq_new",
        "ic_burst_freq",
        "ic_pr_burst_freq_new",
        "ic_pr_burst_freq",
        "ec_pr_freq",
        "ic_new_time",
        "ic_burst_penalty",
        "ic_held_release_interval",
        "bootstrap_only",
        "recursive_prs",
        "announces_from_internal",
        "ignore_config_warnings",
    ];
    let medium = match type_name {
        "AutoInterface" => &[
            "group_id",
            "discovery_scope",
            "discovery_port",
            "data_port",
            "devices",
            "ignored_devices",
            "multicast_address_type",
        ][..],
        "TCPClientInterface" => &[
            "target_host",
            "target_port",
            "kiss_framing",
            "i2p_tunneled",
            "connect_timeout",
            "max_reconnect_tries",
            "fixed_mtu",
        ],
        "TCPServerInterface" => &[
            "listen_ip",
            "listen_port",
            "device",
            "port",
            "prefer_ipv6",
            "i2p_tunneled",
            "kiss_framing",
            "fixed_mtu",
        ],
        "UDPInterface" => &[
            "listen_ip",
            "listen_port",
            "forward_ip",
            "forward_port",
            "device",
            "port",
        ],
        "SerialInterface" => &["port", "speed", "databits", "parity", "stopbits"],
        "KISSInterface" => &[
            "port",
            "speed",
            "databits",
            "parity",
            "stopbits",
            "flow_control",
            "preamble",
            "txtail",
            "persistence",
            "slottime",
            "id_callsign",
            "id_interval",
        ],
        "AX25KISSInterface" => &[
            "port",
            "speed",
            "databits",
            "parity",
            "stopbits",
            "flow_control",
            "preamble",
            "txtail",
            "persistence",
            "slottime",
            "callsign",
            "ssid",
        ],
        "RNodeInterface" => &[
            "port",
            "frequency",
            "bandwidth",
            "spreadingfactor",
            "codingrate",
            "txpower",
            "flow_control",
            "id_callsign",
            "id_interval",
            "airtime_limit_short",
            "airtime_limit_long",
        ],
        "PipeInterface" => &["command", "respawn_delay"],
        "BackboneInterface" | "BackboneClientInterface" => &[
            "listen_ip",
            "listen_port",
            "target_host",
            "target_port",
            "port",
            "device",
            "prefer_ipv6",
            "i2p_tunneled",
            "connect_timeout",
            "max_reconnect_tries",
            "remote",
            "listen_on",
        ],
        _ => &[],
    };
    known.extend_from_slice(medium);
    known
}
