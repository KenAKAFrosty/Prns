use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use crate::configobj::{self, ConfigError, Section, Value};

pub use crate::configobj::Value as ReferenceValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceMode {
    Full,
    AccessPoint,
    PointToPoint,
    Roaming,
    Boundary,
    Gateway,
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
    pub network_name: Option<String>,
    pub passphrase: Option<String>,
    pub ifac_size_bits: Option<u32>,
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
    pub other_sections: BTreeMap<String, BTreeMap<String, ReferenceValue>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceError {
    Syntax(ConfigError),
    MissingType { interface: String },
    BadValue {
        interface: String,
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
            ReferenceError::BadValue { interface, key, reason } => {
                write!(f, "interface '{interface}', key '{key}': {reason}")
            }
        }
    }
}

impl std::error::Error for ReferenceError {}

pub fn parse(input: &str) -> Result<ReferenceConfig, ReferenceError> {
    interpret(&configobj::parse(input)?)
}

fn interpret(root: &Section) -> Result<ReferenceConfig, ReferenceError> {
    let mut config = ReferenceConfig::default();
    if let Some(reticulum) = root.section("reticulum") {
        config.globals = scalar_map(reticulum);
    }
    for (key, value) in &root.scalars {
        config
            .globals
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
    if let Some(interfaces) = root.section("interfaces") {
        for (name, section) in &interfaces.sections {
            config.interfaces.push(interpret_interface(name, section)?);
        }
    }
    for (name, section) in &root.sections {
        if name == "reticulum" || name == "interfaces" {
            continue;
        }
        config.other_sections.insert(name.clone(), scalar_map(section));
    }
    Ok(config)
}

fn scalar_map(section: &Section) -> BTreeMap<String, ReferenceValue> {
    section.scalars.iter().cloned().collect()
}

fn interpret_interface(name: &str, section: &Section) -> Result<ReferenceInterface, ReferenceError> {
    let mut rest: BTreeMap<String, Value> = section.scalars.iter().cloned().collect();

    let type_name = rest
        .remove("type")
        .and_then(|value| value.as_scalar().map(str::to_string))
        .ok_or_else(|| ReferenceError::MissingType {
            interface: name.to_string(),
        })?;

    let enabled = take_enabled(&mut rest, name)?;
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

    let params = interpret_params(&type_name, &mut rest, section, name)?;

    Ok(ReferenceInterface {
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
        params,
        extra: rest,
    })
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

fn take_radio(rest: &mut BTreeMap<String, Value>, interface: &str) -> Result<RNodeRadio, ReferenceError> {
    Ok(RNodeRadio {
        frequency: opt(rest, "frequency", interface, coerce_u64)?,
        bandwidth: opt(rest, "bandwidth", interface, coerce_u32)?,
        spreadingfactor: opt(rest, "spreadingfactor", interface, coerce_u8)?,
        codingrate: opt(rest, "codingrate", interface, coerce_u8)?,
        txpower: opt(rest, "txpower", interface, coerce_i16)?,
    })
}

fn take_enabled(rest: &mut BTreeMap<String, Value>, interface: &str) -> Result<Option<bool>, ReferenceError> {
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

fn take_mode(rest: &mut BTreeMap<String, Value>, interface: &str) -> Result<Option<ReferenceMode>, ReferenceError> {
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

fn scalar_text<'a>(value: &'a Value, interface: &str, key: &str) -> Result<&'a str, ReferenceError> {
    value.as_scalar().ok_or_else(|| ReferenceError::BadValue {
        interface: interface.to_string(),
        key: key.to_string(),
        reason: "expected a single value, found a list",
    })
}

fn coerce_parsed<T: FromStr>(
    value: &Value,
    interface: &str,
    key: &str,
    reason: &'static str,
) -> Result<T, ReferenceError> {
    scalar_text(value, interface, key)?
        .trim()
        .parse::<T>()
        .map_err(|_| ReferenceError::BadValue {
            interface: interface.to_string(),
            key: key.to_string(),
            reason,
        })
}

fn coerce_bool(value: &Value, interface: &str, key: &str) -> Result<bool, ReferenceError> {
    match scalar_text(value, interface, key)?.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(ReferenceError::BadValue {
            interface: interface.to_string(),
            key: key.to_string(),
            reason: "expected a boolean (yes/no/true/false/on/off/1/0)",
        }),
    }
}

fn coerce_mode(value: &Value, interface: &str) -> Result<ReferenceMode, ReferenceError> {
    match scalar_text(value, interface, "mode")?.to_ascii_lowercase().as_str() {
        "full" => Ok(ReferenceMode::Full),
        "access_point" | "accesspoint" | "ap" => Ok(ReferenceMode::AccessPoint),
        "pointtopoint" | "ptp" => Ok(ReferenceMode::PointToPoint),
        "roaming" => Ok(ReferenceMode::Roaming),
        "boundary" => Ok(ReferenceMode::Boundary),
        "gateway" | "gw" => Ok(ReferenceMode::Gateway),
        _ => Err(ReferenceError::BadValue {
            interface: interface.to_string(),
            key: "mode".to_string(),
            reason: "unrecognized interface mode",
        }),
    }
}

fn coerce_string(value: &Value, interface: &str, key: &str) -> Result<String, ReferenceError> {
    Ok(scalar_text(value, interface, key)?.to_string())
}

fn coerce_list(value: &Value, _interface: &str, _key: &str) -> Result<Vec<String>, ReferenceError> {
    Ok(value.as_list().into_iter().map(str::to_string).collect())
}

fn coerce_u64(value: &Value, interface: &str, key: &str) -> Result<u64, ReferenceError> {
    coerce_parsed(value, interface, key, "expected a non-negative integer")
}

fn coerce_u32(value: &Value, interface: &str, key: &str) -> Result<u32, ReferenceError> {
    coerce_parsed(value, interface, key, "expected a non-negative integer")
}

fn coerce_u16(value: &Value, interface: &str, key: &str) -> Result<u16, ReferenceError> {
    coerce_parsed(value, interface, key, "expected a port or small integer (0-65535)")
}

fn coerce_u8(value: &Value, interface: &str, key: &str) -> Result<u8, ReferenceError> {
    coerce_parsed(value, interface, key, "expected a small integer (0-255)")
}

fn coerce_i16(value: &Value, interface: &str, key: &str) -> Result<i16, ReferenceError> {
    coerce_parsed(value, interface, key, "expected an integer")
}

fn coerce_usize(value: &Value, interface: &str, key: &str) -> Result<usize, ReferenceError> {
    coerce_parsed(value, interface, key, "expected a non-negative integer")
}

fn coerce_f64(value: &Value, interface: &str, key: &str) -> Result<f64, ReferenceError> {
    coerce_parsed(value, interface, key, "expected a number")
}

#[cfg(test)]
mod tests {
    use super::*;

    const REALISTIC: &str = "[reticulum]\n\
        enable_transport = Yes\n\
        share_instance = Yes\n\
        [logging]\n\
        loglevel = 4\n\
        [interfaces]\n\
          [[Default Interface]]\n\
            type = AutoInterface\n\
            enabled = Yes\n\
          [[Hub]]\n\
            type = TCPClientInterface\n\
            interface_enabled = True\n\
            target_host = hub.example.com\n\
            target_port = 4965\n\
            mode = gw\n\
          [[Radio]]\n\
            type = RNodeMultiInterface\n\
            enabled = Yes\n\
            port = /dev/ttyUSB0\n\
            [[[Fast]]]\n\
              vport = 0\n\
              frequency = 867200000\n\
              spreadingfactor = 7\n\
            [[[Slow]]]\n\
              vport = 1\n\
              spreadingfactor = 12\n";

    #[test]
    fn parse_reads_globals_interfaces_and_other_sections() {
        let config = parse(REALISTIC).unwrap();
        assert_eq!(config.interfaces.len(), 3);
        assert_eq!(
            config.globals.get("enable_transport"),
            Some(&ReferenceValue::Scalar("Yes".to_string()))
        );
        assert_eq!(
            config.other_sections["logging"].get("loglevel"),
            Some(&ReferenceValue::Scalar("4".to_string()))
        );
    }

    #[test]
    fn parse_coerces_typed_fields_and_folds_dual_keys_and_aliases() {
        let config = parse(REALISTIC).unwrap();
        let hub = &config.interfaces[1];
        assert_eq!(hub.enabled, Some(true));
        assert_eq!(hub.mode, Some(ReferenceMode::Gateway));
        assert_eq!(
            hub.params,
            ReferenceParams::TcpClient {
                target_host: Some("hub.example.com".to_string()),
                target_port: Some(4965),
                kiss_framing: None,
                connect_timeout: None,
                max_reconnect_tries: None,
                fixed_mtu: None,
            }
        );
    }

    #[test]
    fn parse_reads_rnode_multi_subinterfaces() {
        let config = parse(REALISTIC).unwrap();
        let radio = &config.interfaces[2];
        match &radio.params {
            ReferenceParams::RnodeMulti { port, subinterfaces, .. } => {
                assert_eq!(port.as_deref(), Some("/dev/ttyUSB0"));
                assert_eq!(subinterfaces.len(), 2);
                assert_eq!(subinterfaces[0].vport.as_deref(), Some("0"));
                assert_eq!(subinterfaces[0].radio.frequency, Some(867_200_000));
                assert_eq!(subinterfaces[1].radio.spreadingfactor, Some(12));
            }
            other => panic!("expected RnodeMulti, got {other:?}"),
        }
    }

    #[test]
    fn parse_lands_unmodeled_keys_in_extra() {
        let config = parse(
            "[interfaces]\n\
               [[Custom]]\n\
                 type = TCPClientInterface\n\
                 target_host = host\n\
                 announce_interval = 30\n\
                 discovery_frequency = 867200000\n",
        )
        .unwrap();
        let extra = &config.interfaces[0].extra;
        assert!(extra.contains_key("announce_interval"));
        assert!(extra.contains_key("discovery_frequency"));
    }

    #[test]
    fn parse_keeps_an_external_module_type_as_unknown() {
        let config = parse(
            "[interfaces]\n\
               [[Custom]]\n\
                 type = MyCustomInterface\n\
                 secret = on\n",
        )
        .unwrap();
        let custom = &config.interfaces[0];
        assert!(!custom.is_known());
        assert_eq!(custom.type_name, "MyCustomInterface");
        assert!(custom.extra.contains_key("secret"));
    }

    #[test]
    fn parse_errors_on_missing_type() {
        let result = parse("[interfaces]\n[[Broken]]\nenabled = Yes\n");
        assert!(matches!(result, Err(ReferenceError::MissingType { .. })));
    }

    #[test]
    fn parse_errors_on_an_uncoercible_value() {
        let result = parse(
            "[interfaces]\n\
               [[Hub]]\n\
                 type = TCPClientInterface\n\
                 target_port = not-a-number\n",
        );
        assert!(matches!(result, Err(ReferenceError::BadValue { .. })));
    }

    #[test]
    fn an_absent_key_is_none_never_a_default() {
        let config = parse(
            "[interfaces]\n\
               [[Mesh]]\n\
                 type = UDPInterface\n\
                 enabled = Yes\n\
                 listen_ip = 0.0.0.0\n\
                 listen_port = 4242\n",
        )
        .unwrap();
        let mesh = &config.interfaces[0];
        assert_eq!(mesh.outgoing, None);
        assert_eq!(mesh.mode, None);
    }
}
