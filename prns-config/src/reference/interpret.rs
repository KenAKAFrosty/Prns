use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use prns_core::identity::IdentityHash;
use prns_core::interface_discovery::StampCost;

use crate::configobj::{ConfigError, Section, Value};

use super::types::{
    RNodeRadio, RNodeSubinterface, ReferenceConfig, ReferenceDiscoveryConfig, ReferenceInterface,
    ReferenceInterfaceDiscovery, ReferenceMode, ReferenceParams, ReferenceValue,
};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ReferenceError {
    Syntax(ConfigError),
    MissingType {
        interface: String,
    },
    BadValue {
        interface: String,
        key: String,
        reason: &'static str,
    },
    BadGlobalValue {
        key: String,
        reason: &'static str,
    },
}

impl From<ConfigError> for ReferenceError {
    fn from(error: ConfigError) -> Self {
        ReferenceError::Syntax(error)
    }
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReferenceError::Syntax(error) => write!(f, "{error}"),
            ReferenceError::MissingType { interface } => {
                write!(f, "interface '{interface}': missing required 'type'")
            }
            ReferenceError::BadValue {
                interface,
                key,
                reason,
            } => {
                write!(f, "interface '{interface}', key '{key}': {reason}")
            }
            ReferenceError::BadGlobalValue { key, reason } => {
                write!(f, "reticulum key '{key}': {reason}")
            }
        }
    }
}

impl std::error::Error for ReferenceError {}

pub(super) fn interpret(root: &Section) -> Result<ReferenceConfig, ReferenceError> {
    let mut config = ReferenceConfig::default();
    if let Some(reticulum) = root.section("reticulum") {
        config.globals = scalar_map(reticulum);
    }
    config.network_identity_path = global_string(&config.globals, "network_identity")?;
    config.discovery = interpret_discovery_config(&config.globals)?;
    if let Some(interfaces) = root.section("interfaces") {
        for (name, section) in &interfaces.sections {
            if let Some(interface) = interpret_interface(name, section)? {
                config.interfaces.push(interface);
            }
        }
    }
    for (name, section) in &root.sections {
        if name == "reticulum" || name == "interfaces" {
            continue;
        }
        config
            .other_sections
            .insert(name.clone(), scalar_map(section));
    }
    Ok(config)
}

fn scalar_map(section: &Section) -> BTreeMap<String, ReferenceValue> {
    section.scalars.iter().cloned().collect()
}

fn interpret_interface(
    name: &str,
    section: &Section,
) -> Result<Option<ReferenceInterface>, ReferenceError> {
    let mut rest: BTreeMap<String, Value> = section.scalars.iter().cloned().collect();

    let enabled = take_enabled(&mut rest, name)?;
    if enabled != Some(true) {
        return Ok(None);
    }

    let type_name = rest
        .remove("type")
        .and_then(|value| value.as_scalar().map(str::to_string))
        .ok_or_else(|| ReferenceError::MissingType {
            interface: name.to_string(),
        })?;

    let mode = take_mode(&mut rest, name)?;
    let outgoing = opt(&mut rest, "outgoing", name, coerce_bool)?;
    let bitrate = opt(&mut rest, "bitrate", name, coerce_u64)?;
    let announce_cap = opt(&mut rest, "announce_cap", name, coerce_f64)?;
    let announce_rate_target = opt(&mut rest, "announce_rate_target", name, coerce_u64)?;
    let announce_rate_grace = opt(&mut rest, "announce_rate_grace", name, coerce_u64)?;
    let announce_rate_penalty = opt(&mut rest, "announce_rate_penalty", name, coerce_u64)?;
    let network_name = take_alias_string(&mut rest, &["network_name", "networkname"]);
    let passphrase = take_alias_string(&mut rest, &["pass_phrase", "passphrase"]);
    let ifac_size_bits = opt(&mut rest, "ifac_size", name, coerce_u32)?;
    let discovery = take_interface_discovery(&mut rest, name)?;

    let params = interpret_params(&type_name, &mut rest, section, name)?;

    Ok(Some(ReferenceInterface {
        name: name.to_string(),
        type_name,
        enabled,
        mode,
        outgoing,
        bitrate,
        announce_cap,
        announce_rate_target,
        announce_rate_grace,
        announce_rate_penalty,
        network_name,
        passphrase,
        ifac_size_bits,
        discovery,
        params,
        extra: rest,
    }))
}

fn interpret_discovery_config(
    globals: &BTreeMap<String, ReferenceValue>,
) -> Result<ReferenceDiscoveryConfig, ReferenceError> {
    let discover_interfaces = global_bool(globals, "discover_interfaces")?;
    let required_stamp_cost = global_stamp_cost(globals, "required_discovery_value")?;
    let interface_sources = global_identity_hashes(globals, "interface_discovery_sources")?;
    let auto_connect_limit = global_positive_usize(globals, "autoconnect_discovered_interfaces")?;
    Ok(ReferenceDiscoveryConfig {
        discover_interfaces,
        required_stamp_cost,
        interface_sources,
        auto_connect_limit,
    })
}

fn take_interface_discovery(
    rest: &mut BTreeMap<String, Value>,
    interface: &str,
) -> Result<ReferenceInterfaceDiscovery, ReferenceError> {
    let discoverable = opt(rest, "discoverable", interface, coerce_bool)?;
    let mut discovery = ReferenceInterfaceDiscovery {
        discoverable,
        ..ReferenceInterfaceDiscovery::default()
    };
    if discoverable != Some(true) {
        return Ok(discovery);
    }
    discovery.announce_interval_minutes = opt(rest, "announce_interval", interface, coerce_i64)?;
    discovery.stamp_cost = take_interface_stamp_cost(rest, interface)?;
    discovery.name = opt(rest, "discovery_name", interface, coerce_string)?;
    discovery.encrypt = opt(rest, "discovery_encrypt", interface, coerce_bool)?;
    discovery.reachable_on = opt(rest, "reachable_on", interface, coerce_string)?;
    discovery.publish_ifac = opt(rest, "publish_ifac", interface, coerce_bool)?;
    discovery.latitude = opt(rest, "latitude", interface, coerce_f64)?;
    discovery.longitude = opt(rest, "longitude", interface, coerce_f64)?;
    discovery.height = opt(rest, "height", interface, coerce_f64)?;
    discovery.frequency_hz = opt(rest, "discovery_frequency", interface, coerce_u64)?;
    discovery.bandwidth_hz = opt(rest, "discovery_bandwidth", interface, coerce_u32)?;
    discovery.modulation = opt(rest, "discovery_modulation", interface, coerce_string)?;
    Ok(discovery)
}

fn take_interface_stamp_cost(
    rest: &mut BTreeMap<String, Value>,
    interface: &str,
) -> Result<Option<StampCost>, ReferenceError> {
    let value = opt(rest, "discovery_stamp_value", interface, coerce_i64)?;
    match value {
        Some(value) if value > 0 => {
            let value = u16::try_from(value).map_err(|_| {
                bad_value(
                    interface,
                    "discovery_stamp_value",
                    "expected a stamp cost between 1 and 255",
                )
            })?;
            StampCost::new(value).map(Some).map_err(|_| {
                bad_value(
                    interface,
                    "discovery_stamp_value",
                    "expected a stamp cost between 1 and 255",
                )
            })
        }
        Some(0) | None => Ok(None),
        Some(_) => Err(bad_value(
            interface,
            "discovery_stamp_value",
            "expected a stamp cost between 1 and 255, or zero for the default",
        )),
    }
}

fn interpret_params(
    type_name: &str,
    rest: &mut BTreeMap<String, Value>,
    section: &Section,
    interface: &str,
) -> Result<ReferenceParams, ReferenceError> {
    Ok(match type_name {
        "AutoInterface" => ReferenceParams::Auto {
            group_id: opt(rest, "group_id", interface, coerce_string)?,
            discovery_scope: opt(rest, "discovery_scope", interface, coerce_string)?,
            discovery_port: opt(rest, "discovery_port", interface, coerce_u16)?,
            data_port: opt(rest, "data_port", interface, coerce_u16)?,
            devices: opt(rest, "devices", interface, coerce_list)?,
            ignored_devices: opt(rest, "ignored_devices", interface, coerce_list)?,
            multicast_address_type: opt(rest, "multicast_address_type", interface, coerce_string)?,
        },
        "TCPClientInterface" => ReferenceParams::TcpClient {
            target_host: opt(rest, "target_host", interface, coerce_string)?,
            target_port: opt(rest, "target_port", interface, coerce_u16)?,
            kiss_framing: opt(rest, "kiss_framing", interface, coerce_bool)?,
            connect_timeout: opt(rest, "connect_timeout", interface, coerce_u64)?,
            max_reconnect_tries: opt(rest, "max_reconnect_tries", interface, coerce_u32)?,
            fixed_mtu: opt(rest, "fixed_mtu", interface, coerce_usize)?,
        },
        "TCPServerInterface" => ReferenceParams::TcpServer {
            listen_ip: opt(rest, "listen_ip", interface, coerce_string)?,
            listen_port: opt(rest, "listen_port", interface, coerce_u16)?,
            device: opt(rest, "device", interface, coerce_string)?,
            port: opt(rest, "port", interface, coerce_u16)?,
            prefer_ipv6: opt(rest, "prefer_ipv6", interface, coerce_bool)?,
            kiss_framing: opt(rest, "kiss_framing", interface, coerce_bool)?,
            fixed_mtu: opt(rest, "fixed_mtu", interface, coerce_usize)?,
        },
        "UDPInterface" => ReferenceParams::Udp {
            listen_ip: opt(rest, "listen_ip", interface, coerce_string)?,
            listen_port: opt(rest, "listen_port", interface, coerce_u16)?,
            forward_ip: opt(rest, "forward_ip", interface, coerce_string)?,
            forward_port: opt(rest, "forward_port", interface, coerce_u16)?,
            device: opt(rest, "device", interface, coerce_string)?,
            port: opt(rest, "port", interface, coerce_u16)?,
        },
        "SerialInterface" => ReferenceParams::Serial {
            port: opt(rest, "port", interface, coerce_string)?,
            speed: opt(rest, "speed", interface, coerce_u32)?,
            databits: opt(rest, "databits", interface, coerce_u8)?,
            parity: opt(rest, "parity", interface, coerce_string)?,
            stopbits: opt(rest, "stopbits", interface, coerce_u8)?,
        },
        "RNodeInterface" => ReferenceParams::Rnode {
            port: opt(rest, "port", interface, coerce_string)?,
            radio: take_radio(rest, interface)?,
            flow_control: opt(rest, "flow_control", interface, coerce_bool)?,
            id_callsign: opt(rest, "id_callsign", interface, coerce_string)?,
            id_interval: opt(rest, "id_interval", interface, coerce_u64)?,
            airtime_limit_short: opt(rest, "airtime_limit_short", interface, coerce_f64)?,
            airtime_limit_long: opt(rest, "airtime_limit_long", interface, coerce_f64)?,
        },
        "RNodeMultiInterface" => ReferenceParams::RnodeMulti {
            port: opt(rest, "port", interface, coerce_string)?,
            flow_control: opt(rest, "flow_control", interface, coerce_bool)?,
            id_callsign: opt(rest, "id_callsign", interface, coerce_string)?,
            id_interval: opt(rest, "id_interval", interface, coerce_u64)?,
            airtime_limit_short: opt(rest, "airtime_limit_short", interface, coerce_f64)?,
            airtime_limit_long: opt(rest, "airtime_limit_long", interface, coerce_f64)?,
            subinterfaces: interpret_subinterfaces(section)?,
        },
        "KISSInterface" => ReferenceParams::Kiss {
            port: opt(rest, "port", interface, coerce_string)?,
            speed: opt(rest, "speed", interface, coerce_u32)?,
            databits: opt(rest, "databits", interface, coerce_u8)?,
            parity: opt(rest, "parity", interface, coerce_string)?,
            stopbits: opt(rest, "stopbits", interface, coerce_u8)?,
            flow_control: opt(rest, "flow_control", interface, coerce_bool)?,
            preamble: opt(rest, "preamble", interface, coerce_u32)?,
            txtail: opt(rest, "txtail", interface, coerce_u32)?,
            persistence: opt(rest, "persistence", interface, coerce_u32)?,
            slottime: opt(rest, "slottime", interface, coerce_u32)?,
            id_callsign: opt(rest, "id_callsign", interface, coerce_string)?,
            id_interval: opt(rest, "id_interval", interface, coerce_u64)?,
        },
        "AX25KISSInterface" => ReferenceParams::Ax25Kiss {
            port: opt(rest, "port", interface, coerce_string)?,
            speed: opt(rest, "speed", interface, coerce_u32)?,
            databits: opt(rest, "databits", interface, coerce_u8)?,
            parity: opt(rest, "parity", interface, coerce_string)?,
            stopbits: opt(rest, "stopbits", interface, coerce_u8)?,
            flow_control: opt(rest, "flow_control", interface, coerce_bool)?,
            preamble: opt(rest, "preamble", interface, coerce_u32)?,
            txtail: opt(rest, "txtail", interface, coerce_u32)?,
            persistence: opt(rest, "persistence", interface, coerce_u32)?,
            slottime: opt(rest, "slottime", interface, coerce_u32)?,
            callsign: opt(rest, "callsign", interface, coerce_string)?,
            ssid: opt(rest, "ssid", interface, coerce_u8)?,
        },
        "PipeInterface" => ReferenceParams::Pipe {
            command: opt(rest, "command", interface, coerce_string)?,
            respawn_delay: opt(rest, "respawn_delay", interface, coerce_f64)?,
        },
        "I2PInterface" => ReferenceParams::I2p {
            peers: opt(rest, "peers", interface, coerce_list)?,
            connectable: opt(rest, "connectable", interface, coerce_bool)?,
        },
        "BackboneInterface" | "BackboneClientInterface" => ReferenceParams::Backbone {
            listen_ip: opt(rest, "listen_ip", interface, coerce_string)?,
            listen_port: opt(rest, "listen_port", interface, coerce_u16)?,
            target_host: opt(rest, "target_host", interface, coerce_string)?,
            target_port: opt(rest, "target_port", interface, coerce_u16)?,
            port: opt(rest, "port", interface, coerce_u16)?,
            device: opt(rest, "device", interface, coerce_string)?,
            prefer_ipv6: opt(rest, "prefer_ipv6", interface, coerce_bool)?,
            i2p_tunneled: opt(rest, "i2p_tunneled", interface, coerce_bool)?,
            connect_timeout: opt(rest, "connect_timeout", interface, coerce_u64)?,
            max_reconnect_tries: opt(rest, "max_reconnect_tries", interface, coerce_u32)?,
        },
        "WeaveInterface" => ReferenceParams::Weave {
            port: opt(rest, "port", interface, coerce_u16)?,
        },
        _ => ReferenceParams::Unknown,
    })
}

fn interpret_subinterfaces(section: &Section) -> Result<Vec<RNodeSubinterface>, ReferenceError> {
    let mut subinterfaces = Vec::new();
    for (name, sub) in &section.sections {
        let mut rest: BTreeMap<String, Value> = sub.scalars.iter().cloned().collect();
        let enabled = take_enabled(&mut rest, name)?;
        let vport = opt(&mut rest, "vport", name, coerce_string)?;
        let radio = take_radio(&mut rest, name)?;
        subinterfaces.push(RNodeSubinterface {
            name: name.clone(),
            enabled,
            vport,
            radio,
            extra: rest,
        });
    }
    Ok(subinterfaces)
}

fn take_radio(
    rest: &mut BTreeMap<String, Value>,
    interface: &str,
) -> Result<RNodeRadio, ReferenceError> {
    Ok(RNodeRadio {
        frequency: opt(rest, "frequency", interface, coerce_u64)?,
        bandwidth: opt(rest, "bandwidth", interface, coerce_u32)?,
        spreadingfactor: opt(rest, "spreadingfactor", interface, coerce_u8)?,
        codingrate: opt(rest, "codingrate", interface, coerce_u8)?,
        txpower: opt(rest, "txpower", interface, coerce_i16)?,
    })
}

fn take_enabled(
    rest: &mut BTreeMap<String, Value>,
    interface: &str,
) -> Result<Option<bool>, ReferenceError> {
    let explicit = rest.remove("interface_enabled");
    let shorthand = rest.remove("enabled");
    if explicit.is_none() && shorthand.is_none() {
        return Ok(None);
    }
    let explicit = match explicit {
        Some(value) => coerce_bool(&value, interface, "interface_enabled")?,
        None => false,
    };
    let shorthand = match shorthand {
        Some(value) => coerce_bool(&value, interface, "enabled")?,
        None => false,
    };
    Ok(Some(explicit || shorthand))
}

fn take_mode(
    rest: &mut BTreeMap<String, Value>,
    interface: &str,
) -> Result<Option<ReferenceMode>, ReferenceError> {
    let explicit = rest.remove("interface_mode");
    let shorthand = rest.remove("mode");
    match explicit.or(shorthand) {
        Some(value) => Ok(Some(coerce_mode(&value, interface)?)),
        None => Ok(None),
    }
}

fn take_alias_string(rest: &mut BTreeMap<String, Value>, keys: &[&str]) -> Option<String> {
    let mut chosen = None;
    for key in keys {
        if let Some(value) = rest.remove(*key) {
            if chosen.is_none() {
                if let Some(text) = value.as_scalar() {
                    if !text.is_empty() {
                        chosen = Some(text.to_string());
                    }
                }
            }
        }
    }
    chosen
}

fn opt<T>(
    rest: &mut BTreeMap<String, Value>,
    key: &str,
    interface: &str,
    coerce: impl Fn(&Value, &str, &str) -> Result<T, ReferenceError>,
) -> Result<Option<T>, ReferenceError> {
    match rest.remove(key) {
        Some(value) => Ok(Some(coerce(&value, interface, key)?)),
        None => Ok(None),
    }
}

fn bad_value(interface: &str, key: &str, reason: &'static str) -> ReferenceError {
    ReferenceError::BadValue {
        interface: interface.to_string(),
        key: key.to_string(),
        reason,
    }
}

fn bad_global_value(key: &str, reason: &'static str) -> ReferenceError {
    ReferenceError::BadGlobalValue {
        key: key.to_string(),
        reason,
    }
}

fn global_scalar_text<'a>(
    globals: &'a BTreeMap<String, ReferenceValue>,
    key: &str,
) -> Result<Option<&'a str>, ReferenceError> {
    match globals.get(key) {
        Some(value) => value
            .as_scalar()
            .map(Some)
            .ok_or_else(|| bad_global_value(key, "expected a single value, found a list")),
        None => Ok(None),
    }
}

fn global_bool(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
) -> Result<Option<bool>, ReferenceError> {
    let Some(value) = global_scalar_text(globals, key)? else {
        return Ok(None);
    };
    parse_bool(value)
        .map(Some)
        .ok_or_else(|| bad_global_value(key, "expected a boolean (yes/no/true/false/on/off/1/0)"))
}

fn global_string(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
) -> Result<Option<String>, ReferenceError> {
    global_scalar_text(globals, key).map(|value| value.map(str::to_string))
}

fn global_integer(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
) -> Result<Option<i128>, ReferenceError> {
    let Some(value) = global_scalar_text(globals, key)? else {
        return Ok(None);
    };
    let raw = value.trim();
    let cleaned =
        cleaned_number(raw).ok_or_else(|| bad_global_value(key, "expected an integer"))?;
    cleaned
        .parse()
        .map(Some)
        .map_err(|_| bad_global_value(key, "expected an integer"))
}

fn global_stamp_cost(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
) -> Result<Option<StampCost>, ReferenceError> {
    match global_integer(globals, key)? {
        Some(value) if value > 0 => {
            let value = u16::try_from(value)
                .map_err(|_| bad_global_value(key, "expected a stamp cost between 1 and 255"))?;
            StampCost::new(value)
                .map(Some)
                .map_err(|_| bad_global_value(key, "expected a stamp cost between 1 and 255"))
        }
        Some(_) | None => Ok(None),
    }
}

fn global_positive_usize(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
) -> Result<Option<usize>, ReferenceError> {
    match global_integer(globals, key)? {
        Some(value) if value > 0 => usize::try_from(value)
            .map(Some)
            .map_err(|_| bad_global_value(key, "expected a positive integer")),
        Some(_) | None => Ok(None),
    }
}

fn global_identity_hashes(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
) -> Result<Vec<IdentityHash>, ReferenceError> {
    let Some(value) = globals.get(key) else {
        return Ok(Vec::new());
    };
    let mut hashes = Vec::new();
    for text in value.as_list() {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let hash = parse_identity_hash(text).ok_or_else(|| {
            bad_global_value(key, "expected a 32-character hexadecimal identity hash")
        })?;
        if !hashes.contains(&hash) {
            hashes.push(hash);
        }
    }
    Ok(hashes)
}

pub(super) fn parse_identity_hash(text: &str) -> Option<IdentityHash> {
    if text.len() != 32 || !text.is_ascii() {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(IdentityHash::new(bytes))
}

fn scalar_text<'a>(
    value: &'a Value,
    interface: &str,
    key: &str,
) -> Result<&'a str, ReferenceError> {
    value
        .as_scalar()
        .ok_or_else(|| bad_value(interface, key, "expected a single value, found a list"))
}

pub(super) fn cleaned_number(raw: &str) -> Option<Cow<'_, str>> {
    if raw.contains('_') {
        strip_digit_underscores(raw).map(Cow::Owned)
    } else {
        Some(Cow::Borrowed(raw))
    }
}

fn coerce_int<T: TryFrom<i128>>(
    value: &Value,
    interface: &str,
    key: &str,
    reason: &'static str,
) -> Result<T, ReferenceError> {
    let raw = scalar_text(value, interface, key)?.trim();
    let cleaned = cleaned_number(raw).ok_or_else(|| bad_value(interface, key, reason))?;
    let parsed: i128 = cleaned
        .parse()
        .map_err(|_| bad_value(interface, key, reason))?;
    T::try_from(parsed).map_err(|_| bad_value(interface, key, reason))
}

fn strip_digit_underscores(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        if byte == b'_' {
            let left = index.checked_sub(1).map(|i| bytes[i]);
            let right = bytes.get(index + 1).copied();
            let between_digits = left.is_some_and(|b| b.is_ascii_digit())
                && right.is_some_and(|b| b.is_ascii_digit());
            if !between_digits {
                return None;
            }
        }
    }
    Some(text.chars().filter(|c| *c != '_').collect())
}

fn coerce_bool(value: &Value, interface: &str, key: &str) -> Result<bool, ReferenceError> {
    parse_bool(scalar_text(value, interface, key)?).ok_or_else(|| {
        bad_value(
            interface,
            key,
            "expected a boolean (yes/no/true/false/on/off/1/0)",
        )
    })
}

pub(super) fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn coerce_mode(value: &Value, interface: &str) -> Result<ReferenceMode, ReferenceError> {
    match scalar_text(value, interface, "mode")?
        .to_ascii_lowercase()
        .as_str()
    {
        "full" => Ok(ReferenceMode::Full),
        "access_point" | "accesspoint" | "ap" => Ok(ReferenceMode::AccessPoint),
        "pointtopoint" | "ptp" => Ok(ReferenceMode::PointToPoint),
        "roaming" => Ok(ReferenceMode::Roaming),
        "boundary" => Ok(ReferenceMode::Boundary),
        "gateway" | "gw" => Ok(ReferenceMode::Gateway),
        _ => Err(bad_value(interface, "mode", "unrecognized interface mode")),
    }
}

fn coerce_string(value: &Value, interface: &str, key: &str) -> Result<String, ReferenceError> {
    Ok(scalar_text(value, interface, key)?.to_string())
}

fn coerce_list(value: &Value, _interface: &str, _key: &str) -> Result<Vec<String>, ReferenceError> {
    Ok(value.as_list().into_iter().map(str::to_string).collect())
}

fn coerce_u64(value: &Value, interface: &str, key: &str) -> Result<u64, ReferenceError> {
    coerce_int(value, interface, key, "expected a non-negative integer")
}

fn coerce_u32(value: &Value, interface: &str, key: &str) -> Result<u32, ReferenceError> {
    coerce_int(value, interface, key, "expected a non-negative integer")
}

fn coerce_u16(value: &Value, interface: &str, key: &str) -> Result<u16, ReferenceError> {
    coerce_int(
        value,
        interface,
        key,
        "expected a port or small integer (0-65535)",
    )
}

fn coerce_u8(value: &Value, interface: &str, key: &str) -> Result<u8, ReferenceError> {
    coerce_int(value, interface, key, "expected a small integer (0-255)")
}

fn coerce_i16(value: &Value, interface: &str, key: &str) -> Result<i16, ReferenceError> {
    coerce_int(value, interface, key, "expected an integer")
}

fn coerce_i64(value: &Value, interface: &str, key: &str) -> Result<i64, ReferenceError> {
    coerce_int(value, interface, key, "expected an integer")
}

fn coerce_usize(value: &Value, interface: &str, key: &str) -> Result<usize, ReferenceError> {
    coerce_int(value, interface, key, "expected a non-negative integer")
}

fn coerce_f64(value: &Value, interface: &str, key: &str) -> Result<f64, ReferenceError> {
    let raw = scalar_text(value, interface, key)?.trim();
    let cleaned =
        cleaned_number(raw).ok_or_else(|| bad_value(interface, key, "expected a number"))?;
    cleaned
        .parse::<f64>()
        .map_err(|_| bad_value(interface, key, "expected a number"))
}
