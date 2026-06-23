//! The reference-to-ours mapping layer: a faithful [`ReferenceConfig`] becomes a [`DaemonPlan`],
//! the host-agnostic description of the node a daemon should stand up.
//!
//! [`reference`](crate::reference) reads every interface type stock RNS knows about, exactly as RNS
//! reads it. This layer narrows that to what Prns can actually construct today, and is honest about
//! the rest: an interface Prns has no medium for, or one missing a field it needs, becomes a
//! [`DeferredInterface`] carrying *why* rather than being silently dropped; a setting Prns parses but
//! cannot yet route into construction (mode, announce pacing, IFAC) is recorded as an
//! [`UnappliedSetting`] on the interface that bears it. [`PlannedMedium`] holds only variants a host
//! can stand up, so an unconstructable interface is unrepresentable as a plan member.
//!
//! [`plan`] is total: it never fails. A config that names nothing constructible yields a plan whose
//! `interfaces` is empty and whose `deferred` explains each omission, leaving the daemon to decide
//! whether an empty node is worth running.

use std::collections::BTreeMap;

use personal_rns::interfaces::InterfaceMode;

use crate::reference::{
    ReferenceConfig, ReferenceInterface, ReferenceMode, ReferenceParams, ReferenceValue,
};

/// The complete, host-agnostic description of a node to stand up, projected from a stock RNS config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonPlan {
    /// Whether this node forwards traffic for others (RNS `enable_transport`, default off).
    pub transport: bool,
    /// Whether this node hosts a shared instance for local RNS apps (RNS `share_instance`, default on).
    pub shared_instance: SharedInstance,
    /// The interfaces a host can construct from this config, in config order.
    pub interfaces: Vec<PlannedInterface>,
    /// The interfaces this config named that the node will not stand up, each with its reason.
    pub deferred: Vec<DeferredInterface>,
}

/// Whether the node hosts a shared instance, and on which ports if so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedInstance {
    /// The local data bus and its control RPC are served. `None` ports take the RNS defaults
    /// (37428 for the bus, 37429 for control).
    Enabled {
        instance_port: Option<u16>,
        control_port: Option<u16>,
    },
    Disabled,
}

/// One interface a host can construct, with the settings construction honors today and a record of
/// those it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedInterface {
    pub name: String,
    /// The mode the interface should run in once mode plumbing reaches the host constructors. v1 does
    /// not yet apply it; an explicitly configured mode is also recorded in [`Self::unapplied`].
    pub mode: InterfaceMode,
    /// The host's declared bitrate for this pipe. `None` lets construction pick the medium's default.
    pub bitrate_bps: Option<u32>,
    pub medium: PlannedMedium,
    /// Settings parsed from this interface's config that v1 construction does not yet pass through.
    pub unapplied: Vec<UnappliedSetting>,
}

/// The wire a planned interface runs on. Only mediums a host can stand up appear here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedMedium {
    /// RNS `AutoInterface`: multicast LAN discovery plus unicast peers (our `AutoWifi`).
    AutoWifi { group: Option<String> },
    /// RNS `TCPClientInterface`: dial one peer.
    TcpClient { host: String, port: u16 },
    /// RNS `TCPServerInterface`: accept peers on `bind` (`ip:port`).
    TcpServer { bind: String },
    /// RNS `UDPInterface`: receive on `listen`, send to `forward` (each `ip:port`). A datagram
    /// socket needs a send target, so `forward` is required to construct the medium.
    Udp { listen: String, forward: String },
    /// RNS `SerialInterface`: a serial device at `baud`.
    Serial { device: String, baud: u32 },
    /// RNS `KISSInterface`: a KISS TNC on a serial device at `baud`, with the CSMA/timing config
    /// written to the TNC at startup (the millisecond values as the operator gave them).
    Kiss {
        device: String,
        baud: u32,
        preamble_ms: u32,
        txtail_ms: u32,
        persistence: u8,
        slottime_ms: u32,
    },
    /// RNS `AX25KISSInterface`: a KISS TNC carrying AX.25 UI frames, sourced from `callsign`/`ssid`.
    /// The callsign/SSID are validated when the interface is constructed, as RNS does.
    Ax25Kiss {
        device: String,
        baud: u32,
        preamble_ms: u32,
        txtail_ms: u32,
        persistence: u8,
        slottime_ms: u32,
        callsign: String,
        ssid: u8,
    },
    /// RNS `PipeInterface`: a subprocess `command` whose stdout/stdin carries HDLC-framed packets,
    /// respawned `respawn_delay_ms` after it exits.
    Pipe {
        command: String,
        respawn_delay_ms: u64,
    },
    /// RNS `RNodeInterface`: a LoRa RNode driven over a USB-serial KISS link, configured to a radio
    /// channel at bring-up. The radio parameters are required; the airtime locks are the wire-scaled
    /// `int(percent * 100)` values, absent when unconfigured. Range validation happens at
    /// construction (as RNS leaves it to the device's echo-back), so the plan only carries the
    /// values; an out-of-range radio fails to stand up rather than deferring.
    Rnode {
        device: String,
        frequency_hz: u64,
        bandwidth_hz: u32,
        txpower_dbm: i16,
        spreading_factor: u8,
        coding_rate: u8,
        airtime_limit_short_centi: Option<u16>,
        airtime_limit_long_centi: Option<u16>,
    },
    /// RNS `BackboneInterface`: the listening end of a TCP backbone link, accepting peers on `bind`
    /// (`ip:port`). Wire-identical to [`TcpServer`](Self::TcpServer) — a high-throughput transport-node
    /// listener; unlike the reference it is not Linux-gated (tokio replaces RNS's epoll backend).
    Backbone { bind: String },
    /// RNS `BackboneClientInterface`: dial one backbone peer. Wire-identical to
    /// [`TcpClient`](Self::TcpClient).
    BackboneClient { host: String, port: u16 },
}

/// An interface this config named that the node will not stand up, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredInterface {
    pub name: String,
    pub type_name: String,
    pub why: DeferReason,
}

/// Why a configured interface was not turned into a [`PlannedInterface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferReason {
    /// The interface was not enabled (RNS skips an interface unless it is explicitly enabled).
    Disabled,
    /// Prns has no host medium for this interface type yet (I2P, RNodeMulti, Weave).
    UnsupportedKind,
    /// A field the medium needs to be constructed was absent.
    MissingRequiredField { key: &'static str },
}

/// A setting parsed from config that v1 construction does not yet route into the interface it
/// belongs to. Surfaced so the daemon can report it rather than silently ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnappliedSetting {
    /// An explicit `interface_mode`/`mode` (host constructors still pick the medium's fixed mode).
    Mode(InterfaceMode),
    /// `announce_cap` egress pacing.
    AnnounceBandwidthCap,
    /// `announce_rate_target`/`_grace`/`_penalty` per-destination rate limiting.
    AnnounceRateLimit,
    /// `network_name`/`pass_phrase`/`ifac_size` IFAC authentication (not yet plumbed on the host).
    IfacAuthentication,
    /// A medium-specific key parsed but not passed to the constructor (e.g. `kiss_framing`).
    MediumOption(&'static str),
}

/// Project a faithful reference config onto the node a host should stand up.
#[must_use]
pub fn plan(config: &ReferenceConfig) -> DaemonPlan {
    let mut interfaces = Vec::new();
    let mut deferred = Vec::new();
    for interface in &config.interfaces {
        match plan_interface(interface) {
            Ok(planned) => interfaces.push(planned),
            Err(reason) => deferred.push(DeferredInterface {
                name: interface.name.clone(),
                type_name: interface.type_name.clone(),
                why: reason,
            }),
        }
    }
    DaemonPlan {
        transport: global_bool(&config.globals, "enable_transport", false),
        shared_instance: shared_instance(config),
        interfaces,
        deferred,
    }
}

fn shared_instance(config: &ReferenceConfig) -> SharedInstance {
    if global_bool(&config.globals, "share_instance", true) {
        SharedInstance::Enabled {
            instance_port: global_u16(&config.globals, "shared_instance_port"),
            control_port: global_u16(&config.globals, "instance_control_port"),
        }
    } else {
        SharedInstance::Disabled
    }
}

fn plan_interface(interface: &ReferenceInterface) -> Result<PlannedInterface, DeferReason> {
    if !interface.enabled.unwrap_or(false) {
        return Err(DeferReason::Disabled);
    }
    let mut unapplied = common_unapplied(interface);
    let medium = plan_medium(interface, &mut unapplied)?;
    Ok(PlannedInterface {
        name: interface.name.clone(),
        mode: interface.mode.map(map_mode).unwrap_or(InterfaceMode::Full),
        bitrate_bps: interface.bitrate.map(clamp_bitrate),
        medium,
        unapplied,
    })
}

fn plan_medium(
    interface: &ReferenceInterface,
    unapplied: &mut Vec<UnappliedSetting>,
) -> Result<PlannedMedium, DeferReason> {
    match &interface.params {
        ReferenceParams::Auto {
            group_id,
            discovery_scope,
            discovery_port,
            data_port,
            devices,
            ignored_devices,
            multicast_address_type,
        } => {
            note_present(unapplied, "discovery_scope", discovery_scope.is_some());
            note_present(unapplied, "discovery_port", discovery_port.is_some());
            note_present(unapplied, "data_port", data_port.is_some());
            note_present(unapplied, "devices", devices.is_some());
            note_present(unapplied, "ignored_devices", ignored_devices.is_some());
            note_present(
                unapplied,
                "multicast_address_type",
                multicast_address_type.is_some(),
            );
            Ok(PlannedMedium::AutoWifi {
                group: group_id.clone(),
            })
        }
        ReferenceParams::TcpClient {
            target_host,
            target_port,
            kiss_framing,
            connect_timeout,
            max_reconnect_tries,
            fixed_mtu,
        } => {
            let host = target_host
                .clone()
                .ok_or(DeferReason::MissingRequiredField { key: "target_host" })?;
            let port =
                target_port.ok_or(DeferReason::MissingRequiredField { key: "target_port" })?;
            note_present(unapplied, "kiss_framing", kiss_framing.is_some());
            note_present(unapplied, "connect_timeout", connect_timeout.is_some());
            note_present(
                unapplied,
                "max_reconnect_tries",
                max_reconnect_tries.is_some(),
            );
            note_present(unapplied, "fixed_mtu", fixed_mtu.is_some());
            Ok(PlannedMedium::TcpClient { host, port })
        }
        ReferenceParams::TcpServer {
            listen_ip,
            listen_port,
            device,
            port,
            prefer_ipv6,
            kiss_framing,
            fixed_mtu,
        } => {
            let listen_port =
                listen_port.ok_or(DeferReason::MissingRequiredField { key: "listen_port" })?;
            let ip = listen_ip.as_deref().unwrap_or("0.0.0.0");
            note_present(unapplied, "device", device.is_some());
            note_present(unapplied, "port", port.is_some());
            note_present(unapplied, "prefer_ipv6", prefer_ipv6.is_some());
            note_present(unapplied, "kiss_framing", kiss_framing.is_some());
            note_present(unapplied, "fixed_mtu", fixed_mtu.is_some());
            Ok(PlannedMedium::TcpServer {
                bind: format!("{ip}:{listen_port}"),
            })
        }
        ReferenceParams::Udp {
            listen_ip,
            listen_port,
            forward_ip,
            forward_port,
            device,
            port,
        } => {
            let listen_port =
                listen_port.ok_or(DeferReason::MissingRequiredField { key: "listen_port" })?;
            let listen_ip = listen_ip
                .clone()
                .ok_or(DeferReason::MissingRequiredField { key: "listen_ip" })?;
            let forward_ip = forward_ip
                .clone()
                .ok_or(DeferReason::MissingRequiredField { key: "forward_ip" })?;
            let forward_port = forward_port.ok_or(DeferReason::MissingRequiredField {
                key: "forward_port",
            })?;
            note_present(unapplied, "device", device.is_some());
            note_present(unapplied, "port", port.is_some());
            Ok(PlannedMedium::Udp {
                listen: format!("{listen_ip}:{listen_port}"),
                forward: format!("{forward_ip}:{forward_port}"),
            })
        }
        ReferenceParams::Serial {
            port,
            speed,
            databits,
            parity,
            stopbits,
        } => {
            let device = port
                .clone()
                .ok_or(DeferReason::MissingRequiredField { key: "port" })?;
            note_present(unapplied, "databits", databits.is_some());
            note_present(unapplied, "parity", parity.is_some());
            note_present(unapplied, "stopbits", stopbits.is_some());
            Ok(PlannedMedium::Serial {
                device,
                baud: speed.unwrap_or(RNS_DEFAULT_SERIAL_BAUD),
            })
        }
        ReferenceParams::Kiss {
            port,
            speed,
            databits,
            parity,
            stopbits,
            flow_control,
            preamble,
            txtail,
            persistence,
            slottime,
            id_callsign,
            id_interval,
        } => {
            let device = port
                .clone()
                .ok_or(DeferReason::MissingRequiredField { key: "port" })?;
            note_present(unapplied, "databits", databits.is_some());
            note_present(unapplied, "parity", parity.is_some());
            note_present(unapplied, "stopbits", stopbits.is_some());
            // Flow-control TX gating and station-ID beaconing are not yet honored by the host KISS
            // interface; surface them rather than pretend they took effect.
            note_present(unapplied, "flow_control", flow_control.is_some());
            note_present(unapplied, "id_callsign", id_callsign.is_some());
            note_present(unapplied, "id_interval", id_interval.is_some());
            Ok(PlannedMedium::Kiss {
                device,
                baud: speed.unwrap_or(RNS_DEFAULT_SERIAL_BAUD),
                preamble_ms: preamble.unwrap_or(RNS_KISS_DEFAULT_PREAMBLE_MS),
                txtail_ms: txtail.unwrap_or(RNS_KISS_DEFAULT_TXTAIL_MS),
                persistence: persistence
                    .map(|p| p.min(u8::MAX as u32) as u8)
                    .unwrap_or(RNS_KISS_DEFAULT_PERSISTENCE),
                slottime_ms: slottime.unwrap_or(RNS_KISS_DEFAULT_SLOTTIME_MS),
            })
        }
        ReferenceParams::Ax25Kiss {
            port,
            speed,
            databits,
            parity,
            stopbits,
            flow_control,
            preamble,
            txtail,
            persistence,
            slottime,
            callsign,
            ssid,
        } => {
            let device = port
                .clone()
                .ok_or(DeferReason::MissingRequiredField { key: "port" })?;
            let callsign = callsign
                .clone()
                .ok_or(DeferReason::MissingRequiredField { key: "callsign" })?;
            let ssid = ssid.ok_or(DeferReason::MissingRequiredField { key: "ssid" })?;
            note_present(unapplied, "databits", databits.is_some());
            note_present(unapplied, "parity", parity.is_some());
            note_present(unapplied, "stopbits", stopbits.is_some());
            // Flow-control TX gating is not yet honored by the host AX.25 interface.
            note_present(unapplied, "flow_control", flow_control.is_some());
            Ok(PlannedMedium::Ax25Kiss {
                device,
                baud: speed.unwrap_or(RNS_DEFAULT_SERIAL_BAUD),
                preamble_ms: preamble.unwrap_or(RNS_KISS_DEFAULT_PREAMBLE_MS),
                txtail_ms: txtail.unwrap_or(RNS_KISS_DEFAULT_TXTAIL_MS),
                persistence: persistence
                    .map(|p| p.min(u8::MAX as u32) as u8)
                    .unwrap_or(RNS_KISS_DEFAULT_PERSISTENCE),
                slottime_ms: slottime.unwrap_or(RNS_KISS_DEFAULT_SLOTTIME_MS),
                callsign,
                ssid,
            })
        }
        ReferenceParams::Rnode {
            port,
            radio,
            flow_control,
            id_callsign,
            id_interval,
            airtime_limit_short,
            airtime_limit_long,
        } => {
            let device = port
                .clone()
                .ok_or(DeferReason::MissingRequiredField { key: "port" })?;
            let frequency_hz = radio
                .frequency
                .ok_or(DeferReason::MissingRequiredField { key: "frequency" })?;
            let bandwidth_hz = radio
                .bandwidth
                .ok_or(DeferReason::MissingRequiredField { key: "bandwidth" })?;
            let spreading_factor =
                radio
                    .spreadingfactor
                    .ok_or(DeferReason::MissingRequiredField {
                        key: "spreadingfactor",
                    })?;
            let coding_rate = radio
                .codingrate
                .ok_or(DeferReason::MissingRequiredField { key: "codingrate" })?;
            let txpower_dbm = radio
                .txpower
                .ok_or(DeferReason::MissingRequiredField { key: "txpower" })?;
            // Flow-control TX gating and station-ID beaconing are not yet honored by the host RNode
            // interface; surface them rather than pretend they took effect.
            note_present(unapplied, "flow_control", flow_control.is_some());
            note_present(unapplied, "id_callsign", id_callsign.is_some());
            note_present(unapplied, "id_interval", id_interval.is_some());
            Ok(PlannedMedium::Rnode {
                device,
                frequency_hz,
                bandwidth_hz,
                txpower_dbm,
                spreading_factor,
                coding_rate,
                airtime_limit_short_centi: airtime_limit_short.map(pct_to_centi),
                airtime_limit_long_centi: airtime_limit_long.map(pct_to_centi),
            })
        }
        ReferenceParams::Pipe {
            command,
            respawn_delay,
        } => {
            let command = command
                .clone()
                .ok_or(DeferReason::MissingRequiredField { key: "command" })?;
            Ok(PlannedMedium::Pipe {
                command,
                respawn_delay_ms: respawn_delay
                    .map(|secs| (secs.max(0.0) * 1000.0) as u64)
                    .unwrap_or(RNS_PIPE_DEFAULT_RESPAWN_MS),
            })
        }
        ReferenceParams::Backbone {
            listen_ip,
            listen_port,
            target_host,
            target_port,
            port,
            device,
            prefer_ipv6,
            i2p_tunneled,
            connect_timeout,
            max_reconnect_tries,
        } => {
            // RNS collapses `BackboneInterface` (the listener) and `BackboneClientInterface` (the
            // outbound connector) into one config shape; the type name is the role. Backbone is
            // wire-identical to TCP, so each role maps to the matching TCP-shaped medium under its own
            // Backbone kind. `prefer_ipv6` is parsed but not honored — construction binds/dials the
            // address as given rather than re-ordering a hostname's resolved families.
            note_present(unapplied, "prefer_ipv6", prefer_ipv6.is_some());
            if interface.type_name == "BackboneClientInterface" {
                let host = target_host
                    .clone()
                    .ok_or(DeferReason::MissingRequiredField { key: "target_host" })?;
                let port =
                    target_port.ok_or(DeferReason::MissingRequiredField { key: "target_port" })?;
                note_present(unapplied, "i2p_tunneled", i2p_tunneled.is_some());
                note_present(unapplied, "connect_timeout", connect_timeout.is_some());
                note_present(
                    unapplied,
                    "max_reconnect_tries",
                    max_reconnect_tries.is_some(),
                );
                Ok(PlannedMedium::BackboneClient { host, port })
            } else {
                // The `BackboneInterface` listener: `port` overrides `listen_port` for the bind port
                // (RNS `if port != None: bindport = port`); `listen_ip` defaults to all-interfaces.
                // Binding to a named kernel interface (`device`) is not yet supported on the host.
                let bind_port = (*port)
                    .or(*listen_port)
                    .ok_or(DeferReason::MissingRequiredField { key: "listen_port" })?;
                let ip = listen_ip.as_deref().unwrap_or("0.0.0.0");
                note_present(unapplied, "device", device.is_some());
                Ok(PlannedMedium::Backbone {
                    bind: format!("{ip}:{bind_port}"),
                })
            }
        }
        _ => Err(DeferReason::UnsupportedKind),
    }
}

const RNS_DEFAULT_SERIAL_BAUD: u32 = 9_600;

/// RNS `KISSInterface` TNC defaults, mirrored from `rns_parity::kiss::core` (kept in this crate so
/// the config planner stays independent of the interface crate): 350 ms preamble, 20 ms TX-tail,
/// persistence 64, 20 ms slot time.
const RNS_KISS_DEFAULT_PREAMBLE_MS: u32 = 350;
const RNS_KISS_DEFAULT_TXTAIL_MS: u32 = 20;
const RNS_KISS_DEFAULT_PERSISTENCE: u8 = 64;
const RNS_KISS_DEFAULT_SLOTTIME_MS: u32 = 20;

/// RNS `PipeInterface` default respawn delay: 5 seconds.
const RNS_PIPE_DEFAULT_RESPAWN_MS: u64 = 5_000;

/// An RNode airtime-limit percentage as the wire-scaled value RNS sends: `int(percent * 100)`,
/// clamped to the two-byte width the device command carries.
fn pct_to_centi(percent: f64) -> u16 {
    (percent.max(0.0) * 100.0).min(f64::from(u16::MAX)) as u16
}

fn common_unapplied(interface: &ReferenceInterface) -> Vec<UnappliedSetting> {
    let mut unapplied = Vec::new();
    if let Some(mode) = interface.mode {
        unapplied.push(UnappliedSetting::Mode(map_mode(mode)));
    }
    if interface.announce_cap.is_some() {
        unapplied.push(UnappliedSetting::AnnounceBandwidthCap);
    }
    if interface.announce_rate_target.is_some()
        || interface.announce_rate_grace.is_some()
        || interface.announce_rate_penalty.is_some()
    {
        unapplied.push(UnappliedSetting::AnnounceRateLimit);
    }
    if interface.network_name.is_some()
        || interface.passphrase.is_some()
        || interface.ifac_size_bits.is_some()
    {
        unapplied.push(UnappliedSetting::IfacAuthentication);
    }
    unapplied
}

fn note_present(unapplied: &mut Vec<UnappliedSetting>, key: &'static str, present: bool) {
    if present {
        unapplied.push(UnappliedSetting::MediumOption(key));
    }
}

fn map_mode(mode: ReferenceMode) -> InterfaceMode {
    match mode {
        ReferenceMode::Full => InterfaceMode::Full,
        ReferenceMode::AccessPoint => InterfaceMode::AccessPoint,
        ReferenceMode::PointToPoint => InterfaceMode::PointToPoint,
        ReferenceMode::Roaming => InterfaceMode::Roaming,
        ReferenceMode::Boundary => InterfaceMode::Boundary,
        ReferenceMode::Gateway => InterfaceMode::Gateway,
    }
}

fn clamp_bitrate(bitrate: u64) -> u32 {
    bitrate.min(u32::MAX as u64) as u32
}

fn global_bool(globals: &BTreeMap<String, ReferenceValue>, key: &str, default: bool) -> bool {
    match globals.get(key).and_then(ReferenceValue::as_scalar) {
        Some(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => true,
            "false" | "no" | "off" | "0" => false,
            _ => default,
        },
        None => default,
    }
}

fn global_u16(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<u16> {
    globals
        .get(key)
        .and_then(ReferenceValue::as_scalar)
        .and_then(|text| text.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::parse;

    fn plan_of(config: &str) -> DaemonPlan {
        plan(&parse(config).expect("config parses"))
    }

    fn named<'a>(plan: &'a DaemonPlan, name: &str) -> &'a PlannedInterface {
        plan.interfaces
            .iter()
            .find(|interface| interface.name == name)
            .unwrap_or_else(|| panic!("interface '{name}' was planned"))
    }

    const STOCK: &str = "[reticulum]\n\
        enable_transport = Yes\n\
        share_instance = Yes\n\
        [interfaces]\n\
          [[Default Interface]]\n\
            type = AutoInterface\n\
            interface_enabled = Yes\n\
          [[Hub]]\n\
            type = TCPClientInterface\n\
            interface_enabled = Yes\n\
            target_host = hub.example.com\n\
            target_port = 4965\n\
          [[Listener]]\n\
            type = TCPServerInterface\n\
            interface_enabled = Yes\n\
            listen_ip = 0.0.0.0\n\
            listen_port = 4242\n\
          [[Mesh]]\n\
            type = UDPInterface\n\
            interface_enabled = Yes\n\
            listen_ip = 0.0.0.0\n\
            listen_port = 4848\n\
            forward_ip = 255.255.255.255\n\
            forward_port = 4848\n\
          [[Modem]]\n\
            type = SerialInterface\n\
            interface_enabled = Yes\n\
            port = /dev/ttyUSB0\n\
            speed = 115200\n";

    #[test]
    fn global_flags_follow_the_reticulum_section() {
        let plan = plan_of(STOCK);
        assert!(plan.transport);
        assert_eq!(
            plan.shared_instance,
            SharedInstance::Enabled {
                instance_port: None,
                control_port: None,
            }
        );
    }

    #[test]
    fn transport_is_off_and_sharing_on_by_default() {
        let plan = plan_of("[interfaces]\n[[A]]\ntype = AutoInterface\nenabled = Yes\n");
        assert!(!plan.transport);
        assert!(matches!(
            plan.shared_instance,
            SharedInstance::Enabled { .. }
        ));
    }

    #[test]
    fn sharing_off_when_disabled_and_carries_explicit_ports() {
        let plan = plan_of(
            "[reticulum]\nshare_instance = No\n[interfaces]\n[[A]]\ntype = AutoInterface\nenabled = Yes\n",
        );
        assert_eq!(plan.shared_instance, SharedInstance::Disabled);

        let ported = plan_of(
            "[reticulum]\nshared_instance_port = 40000\ninstance_control_port = 40001\n\
             [interfaces]\n[[A]]\ntype = AutoInterface\nenabled = Yes\n",
        );
        assert_eq!(
            ported.shared_instance,
            SharedInstance::Enabled {
                instance_port: Some(40000),
                control_port: Some(40001),
            }
        );
    }

    #[test]
    fn every_host_constructible_medium_maps() {
        let plan = plan_of(STOCK);
        assert_eq!(plan.interfaces.len(), 5);
        assert!(plan.deferred.is_empty());
        assert_eq!(
            named(&plan, "Default Interface").medium,
            PlannedMedium::AutoWifi { group: None }
        );
        assert_eq!(
            named(&plan, "Hub").medium,
            PlannedMedium::TcpClient {
                host: "hub.example.com".to_string(),
                port: 4965,
            }
        );
        assert_eq!(
            named(&plan, "Listener").medium,
            PlannedMedium::TcpServer {
                bind: "0.0.0.0:4242".to_string(),
            }
        );
        assert_eq!(
            named(&plan, "Mesh").medium,
            PlannedMedium::Udp {
                listen: "0.0.0.0:4848".to_string(),
                forward: "255.255.255.255:4848".to_string(),
            }
        );
        assert_eq!(
            named(&plan, "Modem").medium,
            PlannedMedium::Serial {
                device: "/dev/ttyUSB0".to_string(),
                baud: 115200,
            }
        );
    }

    #[test]
    fn a_kiss_tnc_plans_on_its_serial_device_with_reference_tnc_defaults() {
        let plan = plan_of(
            "[interfaces]\n[[TNC]]\ntype = KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\nspeed = 115200\n",
        );
        assert_eq!(
            named(&plan, "TNC").medium,
            PlannedMedium::Kiss {
                device: "/dev/ttyUSB0".to_string(),
                baud: 115200,
                preamble_ms: 350,
                txtail_ms: 20,
                persistence: 64,
                slottime_ms: 20,
            }
        );
    }

    #[test]
    fn a_kiss_tnc_carries_configured_timing_and_notes_what_it_cannot_honor() {
        let plan = plan_of(
            "[interfaces]\n[[TNC]]\ntype = KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             preamble = 150\ntxtail = 50\npersistence = 200\nslottime = 30\nflow_control = Yes\n\
             id_callsign = N0CALL\nid_interval = 600\n",
        );
        let tnc = named(&plan, "TNC");
        assert_eq!(
            tnc.medium,
            PlannedMedium::Kiss {
                device: "/dev/ttyUSB0".to_string(),
                baud: RNS_DEFAULT_SERIAL_BAUD,
                preamble_ms: 150,
                txtail_ms: 50,
                persistence: 200,
                slottime_ms: 30,
            }
        );
        // Flow-control gating and station-ID beaconing are parsed but not yet honored by the host.
        assert!(tnc
            .unapplied
            .contains(&UnappliedSetting::MediumOption("flow_control")));
        assert!(tnc
            .unapplied
            .contains(&UnappliedSetting::MediumOption("id_callsign")));
        assert!(tnc
            .unapplied
            .contains(&UnappliedSetting::MediumOption("id_interval")));
    }

    #[test]
    fn an_ax25_tnc_plans_with_its_callsign_ssid_and_tnc_defaults() {
        let plan = plan_of(
            "[interfaces]\n[[Packet]]\ntype = AX25KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             callsign = N0CALL\nssid = 2\n",
        );
        assert_eq!(
            named(&plan, "Packet").medium,
            PlannedMedium::Ax25Kiss {
                device: "/dev/ttyUSB0".to_string(),
                baud: RNS_DEFAULT_SERIAL_BAUD,
                preamble_ms: 350,
                txtail_ms: 20,
                persistence: 64,
                slottime_ms: 20,
                callsign: "N0CALL".to_string(),
                ssid: 2,
            }
        );
    }

    #[test]
    fn an_ax25_tnc_without_a_callsign_or_ssid_defers_with_the_missing_key() {
        let no_call = plan_of(
            "[interfaces]\n[[Packet]]\ntype = AX25KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\nssid = 0\n",
        );
        assert_eq!(
            no_call.deferred[0].why,
            DeferReason::MissingRequiredField { key: "callsign" }
        );
        let no_ssid = plan_of(
            "[interfaces]\n[[Packet]]\ntype = AX25KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\ncallsign = N0CALL\n",
        );
        assert_eq!(
            no_ssid.deferred[0].why,
            DeferReason::MissingRequiredField { key: "ssid" }
        );
    }

    #[test]
    fn a_pipe_plans_with_its_command_and_the_default_respawn_delay() {
        let plan = plan_of(
            "[interfaces]\n[[Subprocess]]\ntype = PipeInterface\nenabled = Yes\ncommand = nc -l 4242\n",
        );
        assert_eq!(
            named(&plan, "Subprocess").medium,
            PlannedMedium::Pipe {
                command: "nc -l 4242".to_string(),
                respawn_delay_ms: 5_000,
            }
        );
    }

    #[test]
    fn a_pipe_respawn_delay_is_read_in_seconds() {
        let plan = plan_of(
            "[interfaces]\n[[Subprocess]]\ntype = PipeInterface\nenabled = Yes\ncommand = prog\nrespawn_delay = 2.5\n",
        );
        assert_eq!(
            named(&plan, "Subprocess").medium,
            PlannedMedium::Pipe {
                command: "prog".to_string(),
                respawn_delay_ms: 2_500,
            }
        );
    }

    #[test]
    fn a_pipe_without_a_command_defers_with_the_missing_key() {
        let plan = plan_of("[interfaces]\n[[Subprocess]]\ntype = PipeInterface\nenabled = Yes\n");
        assert_eq!(
            plan.deferred[0].why,
            DeferReason::MissingRequiredField { key: "command" }
        );
    }

    #[test]
    fn a_backbone_listener_plans_on_its_bind_address() {
        let plan = plan_of(
            "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\n\
             listen_ip = 0.0.0.0\nlisten_port = 4242\n",
        );
        assert_eq!(
            named(&plan, "Spine").medium,
            PlannedMedium::Backbone {
                bind: "0.0.0.0:4242".to_string(),
            }
        );
    }

    #[test]
    fn a_backbone_listener_defaults_its_ip_and_lets_port_override_listen_port() {
        // `listen_ip` absent → all-interfaces; `port` present → it wins over `listen_port`, mirroring
        // RNS's `if port != None: bindport = port`.
        let plan = plan_of(
            "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\n\
             listen_port = 4242\nport = 5959\n",
        );
        assert_eq!(
            named(&plan, "Spine").medium,
            PlannedMedium::Backbone {
                bind: "0.0.0.0:5959".to_string(),
            }
        );
    }

    #[test]
    fn a_backbone_client_plans_on_its_target() {
        let plan = plan_of(
            "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\n\
             target_host = spine.example.com\ntarget_port = 4242\n",
        );
        assert_eq!(
            named(&plan, "Uplink").medium,
            PlannedMedium::BackboneClient {
                host: "spine.example.com".to_string(),
                port: 4242,
            }
        );
    }

    #[test]
    fn a_backbone_listener_without_a_port_defers_with_the_missing_key() {
        let plan = plan_of(
            "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\nlisten_ip = 0.0.0.0\n",
        );
        assert!(plan.interfaces.is_empty());
        assert_eq!(
            plan.deferred[0].why,
            DeferReason::MissingRequiredField { key: "listen_port" }
        );
    }

    #[test]
    fn a_backbone_client_without_a_target_defers_with_the_missing_key() {
        let no_host = plan_of(
            "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\ntarget_port = 4242\n",
        );
        assert_eq!(
            no_host.deferred[0].why,
            DeferReason::MissingRequiredField { key: "target_host" }
        );
        let no_port = plan_of(
            "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\ntarget_host = spine\n",
        );
        assert_eq!(
            no_port.deferred[0].why,
            DeferReason::MissingRequiredField { key: "target_port" }
        );
    }

    #[test]
    fn a_backbone_interface_surfaces_unhonored_options_rather_than_dropping_them() {
        let listener = plan_of(
            "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\n\
             listen_port = 4242\ndevice = eth0\nprefer_ipv6 = Yes\n",
        );
        let spine = named(&listener, "Spine");
        assert!(spine
            .unapplied
            .contains(&UnappliedSetting::MediumOption("device")));
        assert!(spine
            .unapplied
            .contains(&UnappliedSetting::MediumOption("prefer_ipv6")));

        let client = plan_of(
            "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\n\
             target_host = spine\ntarget_port = 4242\ni2p_tunneled = Yes\nconnect_timeout = 10\n\
             max_reconnect_tries = 3\n",
        );
        let uplink = named(&client, "Uplink");
        assert!(uplink
            .unapplied
            .contains(&UnappliedSetting::MediumOption("i2p_tunneled")));
        assert!(uplink
            .unapplied
            .contains(&UnappliedSetting::MediumOption("connect_timeout")));
        assert!(uplink
            .unapplied
            .contains(&UnappliedSetting::MediumOption("max_reconnect_tries")));
    }

    #[test]
    fn a_disabled_interface_defers_rather_than_constructs() {
        let plan = plan_of(
            "[interfaces]\n[[Off]]\ntype = TCPClientInterface\ntarget_host = h\ntarget_port = 1\n",
        );
        assert!(plan.interfaces.is_empty());
        assert_eq!(plan.deferred[0].why, DeferReason::Disabled);
    }

    #[test]
    fn a_missing_required_field_defers_with_the_key() {
        let plan = plan_of(
            "[interfaces]\n[[Hub]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\n",
        );
        assert_eq!(
            plan.deferred[0].why,
            DeferReason::MissingRequiredField { key: "target_port" }
        );
    }

    #[test]
    fn an_unconstructible_kind_defers_as_unsupported() {
        let plan =
            plan_of("[interfaces]\n[[Mesh]]\ntype = WeaveInterface\nenabled = Yes\nport = 4242\n");
        assert!(plan.interfaces.is_empty());
        assert_eq!(
            plan.deferred[0],
            DeferredInterface {
                name: "Mesh".to_string(),
                type_name: "WeaveInterface".to_string(),
                why: DeferReason::UnsupportedKind,
            }
        );
    }

    #[test]
    fn an_rnode_plans_with_its_radio_channel_and_scales_its_airtime_locks() {
        let plan = plan_of(
            "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
             codingrate = 5\nairtime_limit_short = 1.5\nairtime_limit_long = 5.0\n",
        );
        assert_eq!(
            named(&plan, "Radio").medium,
            PlannedMedium::Rnode {
                device: "/dev/ttyUSB0".to_string(),
                frequency_hz: 868_000_000,
                bandwidth_hz: 125_000,
                txpower_dbm: 7,
                spreading_factor: 8,
                coding_rate: 5,
                airtime_limit_short_centi: Some(150),
                airtime_limit_long_centi: Some(500),
            }
        );
    }

    #[test]
    fn an_rnode_without_a_radio_field_defers_with_the_missing_key() {
        let no_freq = plan_of(
            "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             bandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n",
        );
        assert!(no_freq.interfaces.is_empty());
        assert_eq!(
            no_freq.deferred[0].why,
            DeferReason::MissingRequiredField { key: "frequency" }
        );
        let no_sf = plan_of(
            "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\ncodingrate = 5\n",
        );
        assert_eq!(
            no_sf.deferred[0].why,
            DeferReason::MissingRequiredField {
                key: "spreadingfactor"
            }
        );
    }

    #[test]
    fn an_rnode_surfaces_flow_control_and_beaconing_as_unapplied() {
        let plan = plan_of(
            "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
             codingrate = 5\nflow_control = Yes\nid_callsign = N0CALL\nid_interval = 600\n",
        );
        let radio = named(&plan, "Radio");
        assert!(radio
            .unapplied
            .contains(&UnappliedSetting::MediumOption("flow_control")));
        assert!(radio
            .unapplied
            .contains(&UnappliedSetting::MediumOption("id_callsign")));
        assert!(radio
            .unapplied
            .contains(&UnappliedSetting::MediumOption("id_interval")));
    }

    #[test]
    fn a_listen_only_udp_defers_for_want_of_a_forward_target() {
        let plan = plan_of(
            "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\n\
             listen_ip = 0.0.0.0\nlisten_port = 4848\n",
        );
        assert!(plan.interfaces.is_empty());
        assert_eq!(
            plan.deferred[0].why,
            DeferReason::MissingRequiredField { key: "forward_ip" }
        );
    }

    #[test]
    fn the_serial_baud_defaults_to_the_rns_default_when_unset() {
        let plan = plan_of(
            "[interfaces]\n[[Modem]]\ntype = SerialInterface\nenabled = Yes\nport = /dev/ttyUSB0\n",
        );
        assert_eq!(
            named(&plan, "Modem").medium,
            PlannedMedium::Serial {
                device: "/dev/ttyUSB0".to_string(),
                baud: RNS_DEFAULT_SERIAL_BAUD,
            }
        );
    }

    #[test]
    fn parsed_but_unhonored_settings_are_surfaced_not_dropped() {
        let plan = plan_of(
            "[interfaces]\n\
               [[Hub]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = h\n\
                 target_port = 1\n\
                 mode = gateway\n\
                 announce_cap = 5.0\n\
                 announce_rate_target = 3600\n\
                 network_name = secret-net\n\
                 kiss_framing = Yes\n",
        );
        let hub = named(&plan, "Hub");
        assert!(hub
            .unapplied
            .contains(&UnappliedSetting::Mode(InterfaceMode::Gateway)));
        assert!(hub
            .unapplied
            .contains(&UnappliedSetting::AnnounceBandwidthCap));
        assert!(hub.unapplied.contains(&UnappliedSetting::AnnounceRateLimit));
        assert!(hub
            .unapplied
            .contains(&UnappliedSetting::IfacAuthentication));
        assert!(hub
            .unapplied
            .contains(&UnappliedSetting::MediumOption("kiss_framing")));
    }

    #[test]
    fn a_clean_interface_carries_no_unapplied_noise() {
        let plan = plan_of(STOCK);
        assert!(named(&plan, "Hub").unapplied.is_empty());
        assert!(named(&plan, "Default Interface").unapplied.is_empty());
    }
}
