use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use prns_core::identity::IdentityHash;
use prns_core::interface_discovery::StampCost;

use crate::configobj::{ConfigError, Section, Value};

use super::interface_type::InterfaceType;
use super::keys::{
    common as common_key, global as global_key, interface as interface_key, prns as prns_key,
    section as section_key,
};
use super::types::{
    RNodeRadio, RNodeSubinterface, ReferenceAnnounceRateTarget, ReferenceBlackholeExchange,
    ReferenceConfig, ReferenceConfigParams, ReferenceDiscoveryConfig, ReferenceInterface,
    ReferenceInterfaceDiscovery, ReferenceMode, ReferencePrnsConfig, ReferenceRemoteManagement,
    ReferenceValue,
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
    BadPrnsValue {
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
            ReferenceError::BadPrnsValue { key, reason } => {
                write!(f, "prns key '{key}': {reason}")
            }
        }
    }
}

impl std::error::Error for ReferenceError {}

pub(super) fn interpret(root: &Section) -> Result<ReferenceConfig, ReferenceError> {
    let mut config = ReferenceConfig::default();
    if let Some(reticulum) = root.section(section_key::RETICULUM) {
        config.globals = scalar_map(reticulum);
    }
    config.network_identity_path = global_string(&config.globals, global_key::NETWORK_IDENTITY)?;
    config.discovery = interpret_discovery_config(&config.globals)?;
    config.blackhole_exchange = interpret_blackhole_exchange(&config.globals)?;
    config.remote_management = interpret_remote_management(&config.globals)?;
    if let Some(prns) = root.section(section_key::PRNS) {
        config.prns = interpret_prns_config(prns)?;
    }
    if let Some(interfaces) = root.section(section_key::INTERFACES) {
        for (name, section) in &interfaces.sections {
            if let Some(interface) = interpret_interface(name, section)? {
                config.interfaces.push(interface);
            }
        }
    }
    for (name, section) in &root.sections {
        if name == section_key::RETICULUM
            || name == section_key::PRNS
            || name == section_key::INTERFACES
        {
            continue;
        }
        config
            .other_sections
            .insert(name.clone(), scalar_map(section));
    }
    Ok(config)
}

fn interpret_prns_config(section: &Section) -> Result<ReferencePrnsConfig, ReferenceError> {
    Ok(ReferencePrnsConfig {
        resource_mem_in: prns_byte_quantity(section, prns_key::RESOURCE_MEM_IN)?,
        resource_mem_out: prns_byte_quantity(section, prns_key::RESOURCE_MEM_OUT)?,
    })
}

fn prns_byte_quantity(section: &Section, key: &str) -> Result<Option<usize>, ReferenceError> {
    let Some(value) = section.get(key) else {
        return Ok(None);
    };
    let text = value
        .as_scalar()
        .ok_or_else(|| bad_prns_value(key, "expected one byte quantity, found a list"))?;
    parse_byte_quantity(text)
        .map(Some)
        .map_err(|_error| bad_prns_value(key, "expected an integer byte quantity"))
}

pub(super) struct ByteQuantityParseError;

pub(super) fn parse_byte_quantity(text: &str) -> Result<usize, ByteQuantityParseError> {
    let text = text.trim();
    let (number, multiplier) = if let Some(number) = text.strip_suffix("GiB") {
        (number, 1024_u128.pow(3))
    } else if let Some(number) = text.strip_suffix("MiB") {
        (number, 1024_u128.pow(2))
    } else if let Some(number) = text.strip_suffix("KiB") {
        (number, 1024_u128)
    } else if let Some(number) = text.strip_suffix('B') {
        (number, 1)
    } else {
        (text, 1)
    };
    let cleaned = cleaned_number(number.trim()).ok_or(ByteQuantityParseError)?;
    let quantity = cleaned
        .parse::<u128>()
        .map_err(|_error| ByteQuantityParseError)?;
    let bytes = quantity
        .checked_mul(multiplier)
        .ok_or(ByteQuantityParseError)?;
    usize::try_from(bytes).map_err(|_error| ByteQuantityParseError)
}

fn bad_prns_value(key: &str, reason: &'static str) -> ReferenceError {
    ReferenceError::BadPrnsValue {
        key: key.to_string(),
        reason,
    }
}

fn interpret_blackhole_exchange(
    globals: &BTreeMap<String, ReferenceValue>,
) -> Result<ReferenceBlackholeExchange, ReferenceError> {
    Ok(ReferenceBlackholeExchange {
        publish: global_bool(globals, global_key::PUBLISH_BLACKHOLE)?,
        sources: global_identity_hashes(globals, global_key::BLACKHOLE_SOURCES)?,
        update_interval_minutes: global_float(globals, global_key::BLACKHOLE_UPDATE_INTERVAL)?,
    })
}

fn interpret_remote_management(
    globals: &BTreeMap<String, ReferenceValue>,
) -> Result<ReferenceRemoteManagement, ReferenceError> {
    if global_bool(globals, global_key::ENABLE_REMOTE_MANAGEMENT)? != Some(true) {
        return Ok(ReferenceRemoteManagement::Disabled);
    }
    Ok(ReferenceRemoteManagement::Enabled {
        allowed: global_identity_hashes(globals, global_key::REMOTE_MANAGEMENT_ALLOWED)?,
    })
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

    let configured_type_name = rest
        .remove(interface_key::TYPE)
        .and_then(|value| value.as_scalar().map(str::to_string))
        .ok_or_else(|| ReferenceError::MissingType {
            interface: name.to_string(),
        })?;
    let type_name = InterfaceType::parse(&configured_type_name)
        .map(InterfaceType::canonical_name)
        .unwrap_or(configured_type_name.as_str())
        .to_string();

    let mode = take_mode(&mut rest, name)?;
    let outgoing = opt(&mut rest, interface_key::OUTGOING, name, coerce_bool)?;
    let bootstrap_only = opt(&mut rest, interface_key::BOOTSTRAP_ONLY, name, coerce_bool)?;
    let bitrate = opt(&mut rest, interface_key::BITRATE, name, coerce_u64)?;
    let gravity = opt(&mut rest, interface_key::GRAVITY, name, coerce_i64)?;
    let announce_cap = opt(&mut rest, interface_key::ANNOUNCE_CAP, name, coerce_f64)?;
    let announce_rate_target = opt(
        &mut rest,
        interface_key::ANNOUNCE_RATE_TARGET,
        name,
        coerce_announce_rate_target,
    )?;
    let announce_rate_grace = opt(
        &mut rest,
        interface_key::ANNOUNCE_RATE_GRACE,
        name,
        coerce_u64,
    )?;
    let announce_rate_penalty = opt(
        &mut rest,
        interface_key::ANNOUNCE_RATE_PENALTY,
        name,
        coerce_u64,
    )?;
    let ingress_control = opt(&mut rest, common_key::INGRESS_CONTROL, name, coerce_bool)?;
    let egress_control = opt(&mut rest, common_key::EGRESS_CONTROL, name, coerce_bool)?;
    let recursive_prs = opt(&mut rest, interface_key::RECURSIVE_PRS, name, coerce_bool)?;
    let announces_from_internal = opt(
        &mut rest,
        interface_key::ANNOUNCES_FROM_INTERNAL,
        name,
        coerce_bool,
    )?;
    let announces_to_internal = opt(
        &mut rest,
        interface_key::ANNOUNCES_TO_INTERNAL,
        name,
        coerce_bool,
    )?;
    let ic_max_held_announces = opt(
        &mut rest,
        common_key::IC_MAX_HELD_ANNOUNCES,
        name,
        coerce_i64,
    )?;
    let ic_new_time = opt(&mut rest, common_key::IC_NEW_TIME, name, coerce_f64)?;
    let ic_burst_hold = opt(&mut rest, common_key::IC_BURST_HOLD, name, coerce_f64)?;
    let ic_burst_freq_new = opt(&mut rest, common_key::IC_BURST_FREQ_NEW, name, coerce_f64)?;
    let ic_burst_freq = opt(&mut rest, common_key::IC_BURST_FREQ, name, coerce_f64)?;
    let ic_pr_burst_freq_new = opt(
        &mut rest,
        common_key::IC_PR_BURST_FREQ_NEW,
        name,
        coerce_f64,
    )?;
    let ic_pr_burst_freq = opt(&mut rest, common_key::IC_PR_BURST_FREQ, name, coerce_f64)?;
    let ic_burst_penalty = opt(&mut rest, common_key::IC_BURST_PENALTY, name, coerce_f64)?;
    let ic_held_release_interval = opt(
        &mut rest,
        common_key::IC_HELD_RELEASE_INTERVAL,
        name,
        coerce_f64,
    )?;
    let ec_pr_freq = opt(&mut rest, common_key::EC_PR_FREQ, name, coerce_f64)?;
    let network_name = take_alias_string(&mut rest, interface_key::NETWORK_NAME_ALIASES);
    let passphrase = take_alias_string(&mut rest, interface_key::PASSPHRASE_ALIASES);
    let ifac_size_bits = opt(&mut rest, interface_key::IFAC_SIZE, name, coerce_u32)?;
    let discovery = take_interface_discovery(&mut rest, name)?;

    let params = interpret_params(&type_name, &mut rest, section, name)?;

    Ok(Some(ReferenceInterface {
        name: name.to_string(),
        type_name,
        enabled,
        mode,
        outgoing,
        bootstrap_only,
        bitrate,
        gravity,
        announce_cap,
        announce_rate_target,
        announce_rate_grace,
        announce_rate_penalty,
        ingress_control,
        egress_control,
        recursive_prs,
        announces_from_internal,
        announces_to_internal,
        ic_max_held_announces,
        ic_new_time,
        ic_burst_hold,
        ic_burst_freq_new,
        ic_burst_freq,
        ic_pr_burst_freq_new,
        ic_pr_burst_freq,
        ic_burst_penalty,
        ic_held_release_interval,
        ec_pr_freq,
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
    let discover_interfaces = global_bool(globals, global_key::DISCOVER_INTERFACES)?;
    let required_stamp_cost = global_stamp_cost(globals, global_key::REQUIRED_DISCOVERY_VALUE)?;
    let interface_sources =
        global_identity_hashes(globals, global_key::INTERFACE_DISCOVERY_SOURCES)?;
    let auto_connect_limit =
        global_positive_usize(globals, global_key::AUTOCONNECT_DISCOVERED_INTERFACES)?;
    let auto_connect_gravity = global_i64(globals, global_key::AUTOCONNECT_INTERFACE_GRAVITY)?;
    let auto_connect_announces_to_internal =
        global_bool(globals, global_key::AUTOCONNECT_ANNOUNCES_TO_INTERNAL)?;
    Ok(ReferenceDiscoveryConfig {
        discover_interfaces,
        required_stamp_cost,
        interface_sources,
        auto_connect_limit,
        auto_connect_gravity,
        auto_connect_announces_to_internal,
    })
}

fn take_interface_discovery(
    rest: &mut BTreeMap<String, Value>,
    interface: &str,
) -> Result<ReferenceInterfaceDiscovery, ReferenceError> {
    let discoverable = opt(rest, interface_key::DISCOVERABLE, interface, coerce_bool)?;
    let mut discovery = ReferenceInterfaceDiscovery {
        discoverable,
        ..ReferenceInterfaceDiscovery::default()
    };
    if discoverable != Some(true) {
        return Ok(discovery);
    }
    discovery.announce_interval_minutes = opt(
        rest,
        interface_key::ANNOUNCE_INTERVAL,
        interface,
        coerce_i64,
    )?;
    discovery.stamp_cost = take_interface_stamp_cost(rest, interface)?;
    discovery.name = opt(
        rest,
        interface_key::DISCOVERY_NAME,
        interface,
        coerce_string,
    )?;
    discovery.encrypt = opt(
        rest,
        interface_key::DISCOVERY_ENCRYPT,
        interface,
        coerce_bool,
    )?;
    discovery.reachable_on = opt(rest, interface_key::REACHABLE_ON, interface, coerce_string)?;
    discovery.reachable_port = opt(
        rest,
        interface_key::REACHABLE_PORT,
        interface,
        coerce_nonzero_u16,
    )?;
    discovery.publish_ifac = opt(rest, interface_key::PUBLISH_IFAC, interface, coerce_bool)?;
    discovery.latitude = opt(rest, interface_key::LATITUDE, interface, coerce_f64)?;
    discovery.longitude = opt(rest, interface_key::LONGITUDE, interface, coerce_f64)?;
    discovery.height = opt(rest, interface_key::HEIGHT, interface, coerce_f64)?;
    discovery.frequency_hz = opt(
        rest,
        interface_key::DISCOVERY_FREQUENCY,
        interface,
        coerce_u64,
    )?;
    discovery.bandwidth_hz = opt(
        rest,
        interface_key::DISCOVERY_BANDWIDTH,
        interface,
        coerce_u32,
    )?;
    discovery.modulation = opt(
        rest,
        interface_key::DISCOVERY_MODULATION,
        interface,
        coerce_string,
    )?;
    Ok(discovery)
}

fn take_interface_stamp_cost(
    rest: &mut BTreeMap<String, Value>,
    interface: &str,
) -> Result<Option<StampCost>, ReferenceError> {
    let value = opt(
        rest,
        interface_key::DISCOVERY_STAMP_VALUE,
        interface,
        coerce_i64,
    )?;
    match value {
        Some(value) if value > 0 => {
            let value = u16::try_from(value).map_err(|_| {
                bad_value(
                    interface,
                    interface_key::DISCOVERY_STAMP_VALUE,
                    "expected a stamp cost between 1 and 255",
                )
            })?;
            StampCost::new(value).map(Some).map_err(|_| {
                bad_value(
                    interface,
                    interface_key::DISCOVERY_STAMP_VALUE,
                    "expected a stamp cost between 1 and 255",
                )
            })
        }
        Some(0) | None => Ok(None),
        Some(_) => Err(bad_value(
            interface,
            interface_key::DISCOVERY_STAMP_VALUE,
            "expected a stamp cost between 1 and 255, or zero for the default",
        )),
    }
}

fn interpret_params(
    type_name: &str,
    rest: &mut BTreeMap<String, Value>,
    section: &Section,
    interface: &str,
) -> Result<ReferenceConfigParams, ReferenceError> {
    Ok(match type_name {
        "AutoInterface" => ReferenceConfigParams::Auto {
            group_id: opt(rest, interface_key::GROUP_ID, interface, coerce_string)?,
            discovery_scope: opt(
                rest,
                interface_key::DISCOVERY_SCOPE,
                interface,
                coerce_string,
            )?,
            discovery_port: opt(rest, interface_key::DISCOVERY_PORT, interface, coerce_u16)?,
            data_port: opt(rest, interface_key::DATA_PORT, interface, coerce_u16)?,
            devices: opt(rest, interface_key::DEVICES, interface, coerce_list)?,
            ignored_devices: opt(rest, interface_key::IGNORED_DEVICES, interface, coerce_list)?,
            multicast_address_type: opt(
                rest,
                interface_key::MULTICAST_ADDRESS_TYPE,
                interface,
                coerce_string,
            )?,
        },
        "TCPClientInterface" => ReferenceConfigParams::TcpClient {
            target_host: opt(rest, interface_key::TARGET_HOST, interface, coerce_string)?,
            target_port: opt(rest, interface_key::TARGET_PORT, interface, coerce_u16)?,
            kiss_framing: opt(rest, interface_key::KISS_FRAMING, interface, coerce_bool)?,
            i2p_tunneled: opt(rest, interface_key::I2P_TUNNELED, interface, coerce_bool)?,
            connect_timeout: opt(rest, interface_key::CONNECT_TIMEOUT, interface, coerce_u64)?,
            max_reconnect_tries: opt(
                rest,
                interface_key::MAX_RECONNECT_TRIES,
                interface,
                coerce_u32,
            )?,
            fixed_mtu: opt(rest, interface_key::FIXED_MTU, interface, coerce_usize)?,
        },
        "TCPServerInterface" => ReferenceConfigParams::TcpServer {
            listen_ip: opt(rest, interface_key::LISTEN_IP, interface, coerce_string)?,
            listen_port: opt(rest, interface_key::LISTEN_PORT, interface, coerce_u16)?,
            device: opt(rest, interface_key::DEVICE, interface, coerce_string)?,
            port: opt(rest, interface_key::PORT, interface, coerce_u16)?,
            prefer_ipv6: opt(rest, interface_key::PREFER_IPV6, interface, coerce_bool)?,
            i2p_tunneled: opt(rest, interface_key::I2P_TUNNELED, interface, coerce_bool)?,
            kiss_framing: opt(rest, interface_key::KISS_FRAMING, interface, coerce_bool)?,
            fixed_mtu: opt(rest, interface_key::FIXED_MTU, interface, coerce_usize)?,
        },
        "UDPInterface" => ReferenceConfigParams::Udp {
            listen_ip: opt(rest, interface_key::LISTEN_IP, interface, coerce_string)?,
            listen_port: opt(rest, interface_key::LISTEN_PORT, interface, coerce_u16)?,
            forward_ip: opt(rest, interface_key::FORWARD_IP, interface, coerce_string)?,
            forward_port: opt(rest, interface_key::FORWARD_PORT, interface, coerce_u16)?,
            device: opt(rest, interface_key::DEVICE, interface, coerce_string)?,
            port: opt(rest, interface_key::PORT, interface, coerce_u16)?,
        },
        "SerialInterface" => ReferenceConfigParams::Serial {
            port: opt(rest, interface_key::PORT, interface, coerce_string)?,
            speed: opt(rest, interface_key::SPEED, interface, coerce_u32)?,
            databits: opt(rest, interface_key::DATABITS, interface, coerce_u8)?,
            parity: opt(rest, interface_key::PARITY, interface, coerce_string)?,
            stopbits: opt(rest, interface_key::STOPBITS, interface, coerce_u8)?,
        },
        "RNodeInterface" => ReferenceConfigParams::Rnode {
            port: opt(rest, interface_key::PORT, interface, coerce_string)?,
            radio: take_radio(rest, interface)?,
            flow_control: opt(rest, interface_key::FLOW_CONTROL, interface, coerce_bool)?,
            id_callsign: opt(rest, interface_key::ID_CALLSIGN, interface, coerce_string)?,
            id_interval: opt(rest, interface_key::ID_INTERVAL, interface, coerce_u64)?,
            airtime_limit_short: opt(
                rest,
                interface_key::AIRTIME_LIMIT_SHORT,
                interface,
                coerce_f64,
            )?,
            airtime_limit_long: opt(
                rest,
                interface_key::AIRTIME_LIMIT_LONG,
                interface,
                coerce_f64,
            )?,
        },
        "RNodeMultiInterface" => ReferenceConfigParams::RnodeMulti {
            port: opt(rest, interface_key::PORT, interface, coerce_string)?,
            id_callsign: opt(rest, interface_key::ID_CALLSIGN, interface, coerce_string)?,
            id_interval: opt(rest, interface_key::ID_INTERVAL, interface, coerce_u64)?,
            subinterfaces: interpret_subinterfaces(section)?,
        },
        "KISSInterface" => ReferenceConfigParams::Kiss {
            port: opt(rest, interface_key::PORT, interface, coerce_string)?,
            speed: opt(rest, interface_key::SPEED, interface, coerce_u32)?,
            databits: opt(rest, interface_key::DATABITS, interface, coerce_u8)?,
            parity: opt(rest, interface_key::PARITY, interface, coerce_string)?,
            stopbits: opt(rest, interface_key::STOPBITS, interface, coerce_u8)?,
            flow_control: opt(rest, interface_key::FLOW_CONTROL, interface, coerce_bool)?,
            preamble: opt(rest, interface_key::PREAMBLE, interface, coerce_u32)?,
            txtail: opt(rest, interface_key::TXTAIL, interface, coerce_u32)?,
            persistence: opt(rest, interface_key::PERSISTENCE, interface, coerce_u32)?,
            slottime: opt(rest, interface_key::SLOTTIME, interface, coerce_u32)?,
            id_callsign: opt(rest, interface_key::ID_CALLSIGN, interface, coerce_string)?,
            id_interval: opt(rest, interface_key::ID_INTERVAL, interface, coerce_u64)?,
        },
        "AX25KISSInterface" => ReferenceConfigParams::Ax25Kiss {
            port: opt(rest, interface_key::PORT, interface, coerce_string)?,
            speed: opt(rest, interface_key::SPEED, interface, coerce_u32)?,
            databits: opt(rest, interface_key::DATABITS, interface, coerce_u8)?,
            parity: opt(rest, interface_key::PARITY, interface, coerce_string)?,
            stopbits: opt(rest, interface_key::STOPBITS, interface, coerce_u8)?,
            flow_control: opt(rest, interface_key::FLOW_CONTROL, interface, coerce_bool)?,
            preamble: opt(rest, interface_key::PREAMBLE, interface, coerce_u32)?,
            txtail: opt(rest, interface_key::TXTAIL, interface, coerce_u32)?,
            persistence: opt(rest, interface_key::PERSISTENCE, interface, coerce_u32)?,
            slottime: opt(rest, interface_key::SLOTTIME, interface, coerce_u32)?,
            callsign: opt(rest, interface_key::CALLSIGN, interface, coerce_string)?,
            ssid: opt(rest, interface_key::SSID, interface, coerce_u8)?,
        },
        "PipeInterface" => ReferenceConfigParams::Pipe {
            command: opt(rest, interface_key::COMMAND, interface, coerce_string)?,
            respawn_delay: opt(rest, interface_key::RESPAWN_DELAY, interface, coerce_f64)?,
        },
        "I2PInterface" => ReferenceConfigParams::I2p {
            peers: opt(rest, interface_key::PEERS, interface, coerce_list)?,
            connectable: opt(rest, interface_key::CONNECTABLE, interface, coerce_bool)?,
        },
        "BackboneInterface" | "BackboneClientInterface" => ReferenceConfigParams::Backbone {
            listen_ip: take_alias_string(
                rest,
                &[interface_key::LISTEN_IP, interface_key::LISTEN_ON],
            ),
            listen_port: opt(rest, interface_key::LISTEN_PORT, interface, coerce_u16)?,
            target_host: take_alias_string(
                rest,
                &[interface_key::TARGET_HOST, interface_key::REMOTE],
            ),
            target_port: opt(rest, interface_key::TARGET_PORT, interface, coerce_u16)?,
            port: opt(rest, interface_key::PORT, interface, coerce_u16)?,
            device: opt(rest, interface_key::DEVICE, interface, coerce_string)?,
            prefer_ipv6: opt(rest, interface_key::PREFER_IPV6, interface, coerce_bool)?,
            i2p_tunneled: opt(rest, interface_key::I2P_TUNNELED, interface, coerce_bool)?,
            connect_timeout: opt(rest, interface_key::CONNECT_TIMEOUT, interface, coerce_u64)?,
            max_reconnect_tries: opt(
                rest,
                interface_key::MAX_RECONNECT_TRIES,
                interface,
                coerce_u32,
            )?,
        },
        "WeaveInterface" => ReferenceConfigParams::Weave {
            port: opt(rest, interface_key::PORT, interface, coerce_string)?,
        },
        "PrnsUsbAuto" => ReferenceConfigParams::PrnsUsbAuto,
        "PrnsBluetoothAuto" => ReferenceConfigParams::PrnsBluetoothAuto,
        "PrnsWebSocketClient" => ReferenceConfigParams::PrnsWebSocketClient {
            target: opt(rest, interface_key::TARGET, interface, coerce_string)?,
            framing: opt(rest, interface_key::FRAMING, interface, coerce_string)?,
        },
        "PrnsWebSocketServer" => ReferenceConfigParams::PrnsWebSocketServer {
            listen_ip: opt(rest, interface_key::LISTEN_IP, interface, coerce_string)?,
            listen_port: opt(rest, interface_key::LISTEN_PORT, interface, coerce_u16)?,
            device: opt(rest, interface_key::DEVICE, interface, coerce_string)?,
            port: opt(rest, interface_key::PORT, interface, coerce_u16)?,
            prefer_ipv6: opt(rest, interface_key::PREFER_IPV6, interface, coerce_bool)?,
            framing: opt(rest, interface_key::FRAMING, interface, coerce_string)?,
        },
        _ => ReferenceConfigParams::Unknown,
    })
}

fn interpret_subinterfaces(section: &Section) -> Result<Vec<RNodeSubinterface>, ReferenceError> {
    let mut subinterfaces = Vec::new();
    for (name, sub) in &section.sections {
        let mut rest: BTreeMap<String, Value> = sub.scalars.iter().cloned().collect();
        let enabled = take_enabled(&mut rest, name)?;
        if enabled != Some(true) {
            continue;
        }
        let vport = opt(&mut rest, interface_key::VPORT, name, coerce_u8)?;
        let radio = take_radio(&mut rest, name)?;
        subinterfaces.push(RNodeSubinterface {
            name: name.clone(),
            vport,
            radio,
            flow_control: opt(&mut rest, interface_key::FLOW_CONTROL, name, coerce_bool)?,
            outgoing: opt(&mut rest, interface_key::OUTGOING, name, coerce_bool)?,
            airtime_limit_short: opt(
                &mut rest,
                interface_key::AIRTIME_LIMIT_SHORT,
                name,
                coerce_f64,
            )?,
            airtime_limit_long: opt(
                &mut rest,
                interface_key::AIRTIME_LIMIT_LONG,
                name,
                coerce_f64,
            )?,
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
        frequency: opt(rest, interface_key::FREQUENCY, interface, coerce_u64)?,
        bandwidth: opt(rest, interface_key::BANDWIDTH, interface, coerce_u32)?,
        spreadingfactor: opt(rest, interface_key::SPREADINGFACTOR, interface, coerce_u8)?,
        codingrate: opt(rest, interface_key::CODINGRATE, interface, coerce_u8)?,
        txpower: opt(rest, interface_key::TXPOWER, interface, coerce_i16)?,
    })
}

fn take_enabled(
    rest: &mut BTreeMap<String, Value>,
    interface: &str,
) -> Result<Option<bool>, ReferenceError> {
    let explicit = rest.remove(interface_key::INTERFACE_ENABLED);
    let shorthand = rest.remove(interface_key::ENABLED);
    if explicit.is_none() && shorthand.is_none() {
        return Ok(None);
    }
    let explicit = match explicit {
        Some(value) => coerce_bool(&value, interface, interface_key::INTERFACE_ENABLED)?,
        None => false,
    };
    let shorthand = match shorthand {
        Some(value) => coerce_bool(&value, interface, interface_key::ENABLED)?,
        None => false,
    };
    Ok(Some(explicit || shorthand))
}

fn take_mode(
    rest: &mut BTreeMap<String, Value>,
    interface: &str,
) -> Result<Option<ReferenceMode>, ReferenceError> {
    let explicit = rest.remove(interface_key::INTERFACE_MODE);
    let shorthand = rest.remove(interface_key::MODE);
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

fn global_float(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
) -> Result<Option<f64>, ReferenceError> {
    let Some(value) = global_scalar_text(globals, key)? else {
        return Ok(None);
    };
    let cleaned =
        cleaned_number(value.trim()).ok_or_else(|| bad_global_value(key, "expected a number"))?;
    cleaned
        .parse()
        .map(Some)
        .map_err(|_| bad_global_value(key, "expected a number"))
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

fn global_i64(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &str,
) -> Result<Option<i64>, ReferenceError> {
    global_integer(globals, key)?
        .map(|value| {
            i64::try_from(value)
                .map_err(|_| bad_global_value(key, "expected a signed 64-bit integer"))
        })
        .transpose()
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

pub(crate) fn cleaned_number(raw: &str) -> Option<Cow<'_, str>> {
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

pub(crate) fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn coerce_mode(value: &Value, interface: &str) -> Result<ReferenceMode, ReferenceError> {
    match scalar_text(value, interface, interface_key::MODE)?
        .to_ascii_lowercase()
        .as_str()
    {
        "full" => Ok(ReferenceMode::Full),
        "access_point" | "accesspoint" | "ap" => Ok(ReferenceMode::AccessPoint),
        "pointtopoint" | "ptp" => Ok(ReferenceMode::PointToPoint),
        "roaming" => Ok(ReferenceMode::Roaming),
        "boundary" => Ok(ReferenceMode::Boundary),
        "gateway" | "gw" => Ok(ReferenceMode::Gateway),
        "internal" => Ok(ReferenceMode::Internal),
        _ => Err(bad_value(
            interface,
            interface_key::MODE,
            "unrecognized interface mode",
        )),
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

fn coerce_announce_rate_target(
    value: &Value,
    interface: &str,
    key: &str,
) -> Result<ReferenceAnnounceRateTarget, ReferenceError> {
    if super::announce_rate_target_is_explicit_off(scalar_text(value, interface, key)?) {
        return Ok(ReferenceAnnounceRateTarget::Off);
    }
    let seconds = coerce_int(
        value,
        interface,
        key,
        "expected off, no, false, or a non-negative integer number of seconds",
    )?;
    Ok(match core::num::NonZeroU64::new(seconds) {
        None => ReferenceAnnounceRateTarget::Off,
        Some(seconds) => ReferenceAnnounceRateTarget::Seconds(seconds),
    })
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

fn coerce_nonzero_u16(value: &Value, interface: &str, key: &str) -> Result<u16, ReferenceError> {
    let value = coerce_u16(value, interface, key)?;
    if value == 0 {
        return Err(bad_value(
            interface,
            key,
            "expected a port from 1 through 65535",
        ));
    }
    Ok(value)
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
