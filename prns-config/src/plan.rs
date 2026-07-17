//! The reference-to-ours mapping layer: a faithful [`ReferenceConfig`] becomes a [`DaemonPlan`],
//! the host-agnostic description of the node a daemon should stand up.
//!
//! [`reference`](crate::reference) reads every interface type stock RNS knows about, exactly as RNS
//! reads it. This layer narrows that to what Prns can actually construct today, and is honest about
//! the rest: an interface Prns has no medium for, or one missing a field it needs, becomes a
//! [`DeferredInterface`] carrying *why* rather than being silently dropped; a setting Prns parses but
//! cannot yet route into construction (announce pacing and medium-specific options) is recorded as an
//! [`UnappliedSetting`] on the interface that bears it. [`PlannedMedium`] holds only variants a host
//! can stand up, so an unconstructable interface is unrepresentable as a plan member.
//!
//! [`plan`] is total: it never fails. A config that names nothing constructible yields a plan whose
//! `interfaces` is empty and whose `deferred` explains each omission, leaving the daemon to decide
//! whether an empty node is worth running.

use std::collections::BTreeMap;
use std::path::PathBuf;

use prns_core::interface_discovery::{
    AutoConnectPolicy, DiscoverySourcePolicy, InterfaceDiscoveryPolicy, StampCost,
    DEFAULT_STAMP_COST,
};
use prns_core::interfaces::ax25_kiss::core as ax25_core;
use prns_core::interfaces::ifac::IfacSize;
use prns_core::interfaces::kiss::core as kiss_core;
use prns_core::interfaces::pipe::core as pipe_core;
use prns_core::interfaces::rnode::policy as rnode_policy;
use prns_core::interfaces::serial::core as serial_core;
use prns_core::interfaces::tcp::core as tcp_core;
use prns_core::interfaces::tcp::core::TcpWireFraming;
use prns_core::interfaces::udp::core as udp_core;
use prns_core::interfaces::wifi_auto::core as wifi_core;
use prns_core::interfaces::{
    BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy, InterfaceDefaults,
    InterfaceMode, MtuBytes, MtuPolicy,
};
use prns_core::routing::links::MAX_LINK_MTU;
use prns_core::units::DurationMillis;

use crate::reference::{
    ReferenceConfig, ReferenceInterface, ReferenceMode, ReferenceParams, ReferenceValue,
};

/// The complete, host-agnostic description of a node to stand up, projected from a stock RNS config.
#[derive(Debug, Clone, PartialEq)]
pub struct DaemonPlan {
    /// Whether this node forwards traffic for others (RNS `enable_transport`, default off).
    pub transport: bool,
    /// Whether this node hosts a shared instance for local RNS apps (RNS `share_instance`, default on).
    pub shared_instance: SharedInstance,
    pub network_identity_path: Option<PathBuf>,
    pub discovery: InterfaceDiscoveryPolicy,
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

/// One interface a host can construct, with one effective policy and a record of settings the
/// current host backend does not yet honor.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedInterface {
    pub name: String,
    pub policy: EffectiveInterfacePolicy,
    pub access: InterfaceAccessPlan,
    pub medium: PlannedMedium,
    pub discovery: InterfaceDiscoveryPlan,
    /// Settings parsed from this interface's config that v1 construction does not yet pass through.
    pub unapplied: Vec<UnappliedSetting>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InterfaceDiscoveryPlan {
    Disabled,
    Announce(DiscoveryAnnouncementPlan),
    Unpublishable(DiscoveryPublicationProblem),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryAnnouncementPlan {
    pub interval: DurationMillis,
    pub stamp_cost: StampCost,
    pub name: Option<String>,
    pub encryption: DiscoveryEncryption,
    pub ifac: DiscoveryIfacPublication,
    pub location: DiscoveryLocationPlan,
    pub advertisement: DiscoveryAdvertisementPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryAdvertisementPlan {
    Backbone {
        reachable_on: String,
        port: u16,
    },
    TcpServer {
        reachable_on: String,
        port: u16,
    },
    RNode {
        frequency_hz: u64,
        bandwidth_hz: u32,
        spreading_factor: u8,
        coding_rate: u8,
    },
    Kiss {
        frequency_hz: u64,
        bandwidth_hz: u32,
        modulation: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryPublicationProblem {
    UnsupportedInterfaceType,
    MissingRequiredSetting { key: &'static str },
    IncompatibleSetting { key: &'static str },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiscoveryEncryption {
    Plaintext,
    NetworkIdentity,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiscoveryIfacPublication {
    Omit,
    Include,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryLocationPlan {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub height: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterfaceAccessPlan {
    Open,
    Ifac {
        network_name: Option<String>,
        passphrase: Option<String>,
        size: IfacSize,
    },
}

/// The wire a planned interface runs on. Only mediums a host can stand up appear here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedMedium {
    /// RNS `AutoInterface`: multicast LAN discovery plus unicast peers (our `AutoWifi`).
    AutoWifi { group: Option<String> },
    /// RNS `TCPClientInterface`: dial one peer.
    TcpClient {
        host: String,
        port: u16,
        framing: TcpWireFraming,
    },
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
    MissingRequiredField {
        key: &'static str,
    },
    InvalidSetting {
        key: &'static str,
    },
}

/// A setting parsed from config that v1 construction does not yet route into the interface it
/// belongs to. Surfaced so the daemon can report it rather than silently ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnappliedSetting {
    /// `announce_cap` egress pacing.
    AnnounceBandwidthCap,
    /// `announce_rate_target`/`_grace`/`_penalty` per-destination rate limiting.
    AnnounceRateLimit,
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
        network_identity_path: config.network_identity_path.as_deref().map(PathBuf::from),
        discovery: discovery_policy(config),
        interfaces,
        deferred,
    }
}

fn discovery_policy(config: &ReferenceConfig) -> InterfaceDiscoveryPolicy {
    if config.discovery.discover_interfaces != Some(true) {
        return InterfaceDiscoveryPolicy::Disabled;
    }
    InterfaceDiscoveryPolicy::enabled(
        config
            .discovery
            .required_stamp_cost
            .unwrap_or(DEFAULT_STAMP_COST),
        DiscoverySourcePolicy::from_sources(config.discovery.interface_sources.clone()),
        AutoConnectPolicy::from_maximum(config.discovery.auto_connect_limit.unwrap_or(0)),
    )
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
    let access = plan_access(interface, &medium)?;
    let discovery = plan_interface_discovery(interface, &medium);
    let policy = effective_policy(interface, &medium, &discovery)?;
    Ok(PlannedInterface {
        name: interface.name.clone(),
        policy,
        access,
        medium,
        discovery,
        unapplied,
    })
}

fn effective_policy(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
    discovery: &InterfaceDiscoveryPlan,
) -> Result<EffectiveInterfacePolicy, DeferReason> {
    let bitrate = interface
        .bitrate
        .map(|bitrate| {
            BitrateBps::new(bitrate).ok_or(DeferReason::InvalidSetting { key: "bitrate" })
        })
        .transpose()?;
    let mtu = configured_mtu(interface)?;
    Ok(
        interface_defaults(medium)?.configured(ConfiguredInterfacePolicy {
            mode: Some(planned_mode(interface, discovery)),
            bitrate,
            mtu,
            ..ConfiguredInterfacePolicy::default()
        }),
    )
}

fn interface_defaults(medium: &PlannedMedium) -> Result<InterfaceDefaults, DeferReason> {
    match medium {
        PlannedMedium::AutoWifi { .. } => Ok(wifi_core::DEFAULTS),
        PlannedMedium::TcpClient { .. }
        | PlannedMedium::TcpServer { .. }
        | PlannedMedium::Backbone { .. }
        | PlannedMedium::BackboneClient { .. } => Ok(tcp_core::DEFAULTS),
        PlannedMedium::Udp { .. } => Ok(udp_core::DEFAULTS),
        PlannedMedium::Serial { baud, .. } => {
            let bitrate = BitrateBps::new(u64::from(*baud))
                .ok_or(DeferReason::InvalidSetting { key: "speed" })?;
            Ok(serial_core::defaults_for_bitrate(bitrate))
        }
        PlannedMedium::Kiss { .. } => Ok(kiss_core::DEFAULTS),
        PlannedMedium::Ax25Kiss { .. } => Ok(ax25_core::DEFAULTS),
        PlannedMedium::Pipe { .. } => Ok(pipe_core::DEFAULTS),
        PlannedMedium::Rnode {
            bandwidth_hz,
            spreading_factor,
            coding_rate,
            ..
        } => {
            let raw =
                rnode_policy::nominal_bitrate_bps(*spreading_factor, *coding_rate, *bandwidth_hz);
            let bitrate = BitrateBps::new(u64::from(raw)).ok_or(DeferReason::InvalidSetting {
                key: "radio bitrate",
            })?;
            Ok(rnode_policy::defaults_for_bitrate(bitrate))
        }
    }
}

fn configured_mtu(interface: &ReferenceInterface) -> Result<Option<MtuPolicy>, DeferReason> {
    let fixed_mtu = match &interface.params {
        ReferenceParams::TcpClient { fixed_mtu, .. }
        | ReferenceParams::TcpServer { fixed_mtu, .. } => *fixed_mtu,
        _ => None,
    };
    fixed_mtu
        .map(|fixed_mtu| {
            if fixed_mtu > MAX_LINK_MTU {
                return Err(DeferReason::InvalidSetting { key: "fixed_mtu" });
            }
            MtuBytes::new(fixed_mtu)
                .map(MtuPolicy::Fixed)
                .ok_or(DeferReason::InvalidSetting { key: "fixed_mtu" })
        })
        .transpose()
}

fn plan_interface_discovery(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
) -> InterfaceDiscoveryPlan {
    if interface.discovery.discoverable != Some(true) {
        return InterfaceDiscoveryPlan::Disabled;
    }
    let advertisement = match plan_discovery_advertisement(interface, medium) {
        Ok(advertisement) => advertisement,
        Err(problem) => return InterfaceDiscoveryPlan::Unpublishable(problem),
    };
    let minutes = interface
        .discovery
        .announce_interval_minutes
        .unwrap_or(6 * 60)
        .max(5) as u64;
    InterfaceDiscoveryPlan::Announce(DiscoveryAnnouncementPlan {
        interval: DurationMillis(minutes.saturating_mul(60 * 1_000)),
        stamp_cost: interface.discovery.stamp_cost.unwrap_or(DEFAULT_STAMP_COST),
        name: interface.discovery.name.clone(),
        encryption: if interface.discovery.encrypt == Some(true) {
            DiscoveryEncryption::NetworkIdentity
        } else {
            DiscoveryEncryption::Plaintext
        },
        ifac: if interface.discovery.publish_ifac == Some(true) {
            DiscoveryIfacPublication::Include
        } else {
            DiscoveryIfacPublication::Omit
        },
        location: DiscoveryLocationPlan {
            latitude: interface.discovery.latitude,
            longitude: interface.discovery.longitude,
            height: interface.discovery.height,
        },
        advertisement,
    })
}

fn plan_discovery_advertisement(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
) -> Result<DiscoveryAdvertisementPlan, DiscoveryPublicationProblem> {
    let reachable_on = || {
        interface.discovery.reachable_on.clone().ok_or(
            DiscoveryPublicationProblem::MissingRequiredSetting {
                key: "reachable_on",
            },
        )
    };
    let kiss = || {
        Ok(DiscoveryAdvertisementPlan::Kiss {
            frequency_hz: interface.discovery.frequency_hz.ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: "discovery_frequency",
                },
            )?,
            bandwidth_hz: interface.discovery.bandwidth_hz.ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: "discovery_bandwidth",
                },
            )?,
            modulation: interface.discovery.modulation.clone().ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: "discovery_modulation",
                },
            )?,
        })
    };
    match (medium, &interface.params) {
        (
            PlannedMedium::Backbone { .. },
            ReferenceParams::Backbone {
                listen_port, port, ..
            },
        ) => Ok(DiscoveryAdvertisementPlan::Backbone {
            reachable_on: reachable_on()?,
            port: port.or(*listen_port).ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting { key: "listen_port" },
            )?,
        }),
        (PlannedMedium::TcpServer { .. }, ReferenceParams::TcpServer { listen_port, .. }) => {
            Ok(DiscoveryAdvertisementPlan::TcpServer {
                reachable_on: reachable_on()?,
                port: listen_port.ok_or(DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: "listen_port",
                })?,
            })
        }
        (
            PlannedMedium::Rnode {
                frequency_hz,
                bandwidth_hz,
                spreading_factor,
                coding_rate,
                ..
            },
            ReferenceParams::Rnode { .. },
        ) => Ok(DiscoveryAdvertisementPlan::RNode {
            frequency_hz: *frequency_hz,
            bandwidth_hz: *bandwidth_hz,
            spreading_factor: *spreading_factor,
            coding_rate: *coding_rate,
        }),
        (PlannedMedium::Kiss { .. }, ReferenceParams::Kiss { .. }) => kiss(),
        (
            PlannedMedium::TcpClient {
                framing: TcpWireFraming::Kiss,
                ..
            },
            ReferenceParams::TcpClient { .. },
        ) => kiss(),
        (PlannedMedium::TcpClient { .. }, ReferenceParams::TcpClient { .. }) => {
            Err(DiscoveryPublicationProblem::IncompatibleSetting {
                key: "kiss_framing",
            })
        }
        _ => Err(DiscoveryPublicationProblem::UnsupportedInterfaceType),
    }
}

fn planned_mode(
    interface: &ReferenceInterface,
    discovery: &InterfaceDiscoveryPlan,
) -> InterfaceMode {
    let configured = interface.mode.map(map_mode).unwrap_or(InterfaceMode::Full);
    if matches!(discovery, InterfaceDiscoveryPlan::Disabled)
        || matches!(
            configured,
            InterfaceMode::Gateway | InterfaceMode::AccessPoint
        )
    {
        return configured;
    }
    if matches!(
        interface.params,
        ReferenceParams::Rnode { .. } | ReferenceParams::RnodeMulti { .. }
    ) {
        InterfaceMode::AccessPoint
    } else {
        InterfaceMode::Gateway
    }
}

fn plan_access(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
) -> Result<InterfaceAccessPlan, DeferReason> {
    if interface.network_name.is_none() && interface.passphrase.is_none() {
        return Ok(InterfaceAccessPlan::Open);
    }
    let default_size = match medium {
        PlannedMedium::AutoWifi { .. }
        | PlannedMedium::TcpClient { .. }
        | PlannedMedium::TcpServer { .. }
        | PlannedMedium::Udp { .. }
        | PlannedMedium::Backbone { .. }
        | PlannedMedium::BackboneClient { .. } => IfacSize::WIDE,
        PlannedMedium::Serial { .. }
        | PlannedMedium::Kiss { .. }
        | PlannedMedium::Ax25Kiss { .. }
        | PlannedMedium::Pipe { .. }
        | PlannedMedium::Rnode { .. } => IfacSize::NARROW,
    };
    let size = match interface.ifac_size_bits {
        Some(bits) if bits >= 8 => IfacSize::new((bits / 8) as usize)
            .map_err(|_| DeferReason::InvalidSetting { key: "ifac_size" })?,
        Some(_) | None => default_size,
    };
    Ok(InterfaceAccessPlan::Ifac {
        network_name: interface.network_name.clone(),
        passphrase: interface.passphrase.clone(),
        size,
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
            fixed_mtu: _,
        } => {
            let host = target_host
                .clone()
                .ok_or(DeferReason::MissingRequiredField { key: "target_host" })?;
            let port =
                target_port.ok_or(DeferReason::MissingRequiredField { key: "target_port" })?;
            note_present(unapplied, "connect_timeout", connect_timeout.is_some());
            note_present(
                unapplied,
                "max_reconnect_tries",
                max_reconnect_tries.is_some(),
            );
            Ok(PlannedMedium::TcpClient {
                host,
                port,
                framing: if *kiss_framing == Some(true) {
                    TcpWireFraming::Kiss
                } else {
                    TcpWireFraming::Hdlc
                },
            })
        }
        ReferenceParams::TcpServer {
            listen_ip,
            listen_port,
            device,
            port,
            prefer_ipv6,
            kiss_framing,
            fixed_mtu: _,
        } => {
            let listen_port =
                listen_port.ok_or(DeferReason::MissingRequiredField { key: "listen_port" })?;
            let ip = listen_ip.as_deref().unwrap_or("0.0.0.0");
            note_present(unapplied, "device", device.is_some());
            note_present(unapplied, "port", port.is_some());
            note_present(unapplied, "prefer_ipv6", prefer_ipv6.is_some());
            note_present(unapplied, "kiss_framing", kiss_framing.is_some());
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

/// RNS `KISSInterface` TNC defaults, mirrored from `interfaces::kiss::core` (kept in this crate so
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
    if interface.announce_cap.is_some() {
        unapplied.push(UnappliedSetting::AnnounceBandwidthCap);
    }
    if interface.announce_rate_target.is_some()
        || interface.announce_rate_grace.is_some()
        || interface.announce_rate_penalty.is_some()
    {
        unapplied.push(UnappliedSetting::AnnounceRateLimit);
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
        assert_eq!(plan.discovery, InterfaceDiscoveryPolicy::Disabled);
        assert!(matches!(
            plan.shared_instance,
            SharedInstance::Enabled { .. }
        ));
    }

    #[test]
    fn enabled_discovery_carries_stamp_trust_and_bounded_autoconnect_policy() {
        let plan = plan_of(
            "[reticulum]\n\
               network_identity = ~/.reticulum/storage/identity/network\n\
               discover_interfaces = Yes\n\
               required_discovery_value = 18\n\
               interface_discovery_sources = 00112233445566778899aabbccddeeff\n\
               autoconnect_discovered_interfaces = 3\n",
        );
        assert_eq!(
            plan.network_identity_path.as_deref(),
            Some(std::path::Path::new(
                "~/.reticulum/storage/identity/network"
            )),
        );
        let policy = plan
            .discovery
            .enabled_policy()
            .unwrap_or_else(|| panic!("discovery should be enabled"));
        assert_eq!(policy.required_stamp_cost().get(), 18);
        assert!(policy
            .sources()
            .accepts(&prns_core::identity::IdentityHash::new([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ])));
        assert!(!policy
            .sources()
            .accepts(&prns_core::identity::IdentityHash::new([0xff; 16])));
        assert_eq!(policy.auto_connect().maximum(), Some(3));
    }

    #[test]
    fn zero_discovery_controls_use_the_stock_stamp_and_disable_autoconnect() {
        let plan = plan_of(
            "[reticulum]\ndiscover_interfaces = Yes\nrequired_discovery_value = 0\nautoconnect_discovered_interfaces = 0\n",
        );
        let policy = plan
            .discovery
            .enabled_policy()
            .unwrap_or_else(|| panic!("discovery should be enabled"));
        assert_eq!(policy.required_stamp_cost(), DEFAULT_STAMP_COST);
        assert_eq!(policy.sources().allow_list(), None);
        assert_eq!(policy.auto_connect().maximum(), None);
    }

    #[test]
    fn disabled_discovery_cannot_plan_autoconnect() {
        let plan = plan_of(
            "[reticulum]\ndiscover_interfaces = No\nautoconnect_discovered_interfaces = 4\n",
        );
        assert_eq!(plan.discovery, InterfaceDiscoveryPolicy::Disabled);
    }

    #[test]
    fn a_discoverable_listener_plans_its_announcement_and_gateway_mode() {
        let plan = plan_of(
            "[interfaces]\n\
               [[Spine]]\n\
                 type = BackboneInterface\n\
                 enabled = Yes\n\
                 listen_port = 4242\n\
                 discoverable = Yes\n\
                 announce_interval = 2\n\
                 discovery_stamp_value = 20\n\
                 discovery_name = Public Spine\n\
                 discovery_encrypt = Yes\n\
                 reachable_on = spine.example.com\n\
                 publish_ifac = Yes\n\
                 latitude = 41.88\n\
                 longitude = -87.63\n\
                 height = 181.5\n",
        );
        let spine = named(&plan, "Spine");
        assert_eq!(spine.policy.mode, InterfaceMode::Gateway);
        let InterfaceDiscoveryPlan::Announce(announcement) = &spine.discovery else {
            panic!("spine should publish discovery announces");
        };
        assert_eq!(announcement.interval, DurationMillis(5 * 60 * 1_000));
        assert_eq!(announcement.stamp_cost.get(), 20);
        assert_eq!(announcement.name.as_deref(), Some("Public Spine"));
        assert_eq!(
            announcement.encryption,
            DiscoveryEncryption::NetworkIdentity
        );
        assert_eq!(announcement.ifac, DiscoveryIfacPublication::Include);
        assert_eq!(announcement.location.latitude, Some(41.88));
        assert_eq!(announcement.location.longitude, Some(-87.63));
        assert_eq!(announcement.location.height, Some(181.5));
        assert_eq!(
            announcement.advertisement,
            DiscoveryAdvertisementPlan::Backbone {
                reachable_on: "spine.example.com".to_string(),
                port: 4242,
            }
        );
    }

    #[test]
    fn a_discoverable_rnode_defaults_to_ap_and_six_hour_announcements() {
        let plan = plan_of(
            "[interfaces]\n\
               [[Radio]]\n\
                 type = RNodeInterface\n\
                 enabled = Yes\n\
                 port = /dev/ttyUSB0\n\
                 frequency = 868000000\n\
                 bandwidth = 125000\n\
                 txpower = 7\n\
                 spreadingfactor = 8\n\
                 codingrate = 5\n\
                 discoverable = Yes\n",
        );
        let radio = named(&plan, "Radio");
        assert_eq!(radio.policy.mode, InterfaceMode::AccessPoint);
        let InterfaceDiscoveryPlan::Announce(announcement) = &radio.discovery else {
            panic!("radio should publish discovery announces");
        };
        assert_eq!(announcement.interval, DurationMillis(6 * 60 * 60 * 1_000));
        assert_eq!(announcement.stamp_cost, DEFAULT_STAMP_COST);
        assert_eq!(announcement.encryption, DiscoveryEncryption::Plaintext);
        assert_eq!(announcement.ifac, DiscoveryIfacPublication::Omit);
        assert_eq!(
            announcement.advertisement,
            DiscoveryAdvertisementPlan::RNode {
                frequency_hz: 868_000_000,
                bandwidth_hz: 125_000,
                spreading_factor: 8,
                coding_rate: 5,
            }
        );
    }

    #[test]
    fn discoverable_tcp_and_kiss_plans_are_wire_complete() {
        let plan = plan_of(
            "[interfaces]\n\
               [[Public TCP]]\n\
                 type = TCPServerInterface\n\
                 enabled = Yes\n\
                 listen_ip = 0.0.0.0\n\
                 listen_port = 4242\n\
                 discoverable = Yes\n\
                 reachable_on = tcp.example.com\n\
               [[KISS Tunnel]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = kiss.example.com\n\
                 target_port = 8001\n\
                 kiss_framing = Yes\n\
                 discoverable = Yes\n\
                 discovery_frequency = 144800000\n\
                 discovery_bandwidth = 12500\n\
                 discovery_modulation = AFSK\n",
        );
        let InterfaceDiscoveryPlan::Announce(tcp) = &named(&plan, "Public TCP").discovery else {
            panic!("the TCP listener should publish discovery announces");
        };
        assert_eq!(
            tcp.advertisement,
            DiscoveryAdvertisementPlan::TcpServer {
                reachable_on: "tcp.example.com".to_string(),
                port: 4242,
            }
        );
        let kiss_tunnel = named(&plan, "KISS Tunnel");
        assert!(matches!(
            kiss_tunnel.medium,
            PlannedMedium::TcpClient {
                framing: TcpWireFraming::Kiss,
                ..
            }
        ));
        let InterfaceDiscoveryPlan::Announce(kiss) = &kiss_tunnel.discovery else {
            panic!("the KISS tunnel should publish discovery announces");
        };
        assert_eq!(
            kiss.advertisement,
            DiscoveryAdvertisementPlan::Kiss {
                frequency_hz: 144_800_000,
                bandwidth_hz: 12_500,
                modulation: "AFSK".to_string(),
            }
        );
    }

    #[test]
    fn unpublishable_discovery_configuration_keeps_the_interface_and_the_reason() {
        let plan = plan_of(
            "[interfaces]\n\
               [[Private TCP]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = peer.example.com\n\
                 target_port = 4242\n\
                 discoverable = Yes\n\
               [[Incomplete Server]]\n\
                 type = TCPServerInterface\n\
                 enabled = Yes\n\
                 listen_ip = 0.0.0.0\n\
                 listen_port = 4243\n\
                 discoverable = Yes\n",
        );
        assert_eq!(plan.interfaces.len(), 2);
        assert_eq!(
            named(&plan, "Private TCP").discovery,
            InterfaceDiscoveryPlan::Unpublishable(
                DiscoveryPublicationProblem::IncompatibleSetting {
                    key: "kiss_framing",
                }
            )
        );
        assert_eq!(
            named(&plan, "Incomplete Server").discovery,
            InterfaceDiscoveryPlan::Unpublishable(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: "reachable_on",
                }
            )
        );
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
                framing: TcpWireFraming::Hdlc,
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
    fn a_backbone_listener_defaults_its_ip_and_accepts_the_port_alias() {
        let plan = plan_of(
            "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\n\
             port = 5959\n",
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
    fn a_disabled_interface_is_skipped_before_planning() {
        let plan = plan_of(
            "[interfaces]\n[[Off]]\ntype = TCPClientInterface\ntarget_host = h\ntarget_port = 1\n",
        );
        assert!(plan.interfaces.is_empty());
        assert!(plan.deferred.is_empty());
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
    fn an_unconstructible_kind_fails_before_planning() {
        let errors =
            parse("[interfaces]\n[[Mesh]]\ntype = WeaveInterface\nenabled = Yes\nport = 4242\n")
                .unwrap_err();
        assert!(errors.diagnostics().iter().any(|diagnostic| {
            diagnostic.code() == crate::ConfigDiagnosticCode::UnsupportedInterface
        }));
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
        assert_eq!(named(&plan, "Modem").policy.bitrate.get(), 9_600);
    }

    #[test]
    fn traversed_network_defaults_share_the_500_mbps_policy() {
        let plan = plan_of(
            "[interfaces]\n\
               [[Tcp]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = example.com\n\
                 target_port = 4242\n\
               [[Udp]]\n\
                 type = UDPInterface\n\
                 enabled = Yes\n\
                 listen_ip = 0.0.0.0\n\
                 listen_port = 4242\n\
                 forward_ip = 255.255.255.255\n\
                 forward_port = 4242\n\
               [[Backbone]]\n\
                 type = BackboneInterface\n\
                 enabled = Yes\n\
                 listen_port = 4243\n",
        );

        let tcp = named(&plan, "Tcp");
        assert_eq!(tcp.policy.bitrate.get(), 500_000_000);
        assert_eq!(tcp.policy.mtu.resolve(tcp.policy.bitrate), Some(131_072));
        let udp = named(&plan, "Udp");
        assert_eq!(udp.policy.bitrate.get(), 500_000_000);
        assert_eq!(
            udp.policy.mtu.resolve(udp.policy.bitrate),
            Some(prns_core::interfaces::udp::core::UDP_DATAGRAM_MAX)
        );
        let backbone = named(&plan, "Backbone");
        assert_eq!(backbone.policy.bitrate.get(), 500_000_000);
        assert_eq!(
            backbone.policy.mtu.resolve(backbone.policy.bitrate),
            Some(131_072)
        );
    }

    #[test]
    fn auto_wifi_keeps_its_gigabit_estimate_without_overpromising_its_datagram() {
        let plan = plan_of("[interfaces]\n[[Wifi]]\ntype = AutoInterface\nenabled = Yes\n");
        let wifi = named(&plan, "Wifi");

        assert_eq!(wifi.policy.bitrate.get(), 1_000_000_000);
        assert_eq!(
            wifi.policy.mtu.resolve(wifi.policy.bitrate),
            Some(prns_core::interfaces::wifi_auto::core::HARDWARE_MTU)
        );
    }

    #[test]
    fn configured_u64_bitrate_and_fixed_mtu_are_preserved_without_clamping() {
        let plan = plan_of(
            "[interfaces]\n\
               [[Fast]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = example.com\n\
                 target_port = 4242\n\
                 bitrate = 5000000000\n\
                 fixed_mtu = 4096\n",
        );
        let fast = named(&plan, "Fast");

        assert_eq!(fast.policy.bitrate.get(), 5_000_000_000);
        assert_eq!(fast.policy.mtu.resolve(fast.policy.bitrate), Some(4_096));
        assert!(!fast
            .unapplied
            .contains(&UnappliedSetting::MediumOption("fixed_mtu")));
    }

    #[test]
    fn lower_rate_media_own_their_effective_estimates() {
        let plan = plan_of(
            "[interfaces]\n\
               [[Serial]]\n\
                 type = SerialInterface\n\
                 enabled = Yes\n\
                 port = /dev/ttyUSB0\n\
                 speed = 115200\n\
               [[Kiss]]\n\
                 type = KISSInterface\n\
                 enabled = Yes\n\
                 port = /dev/ttyUSB1\n\
                 speed = 9600\n\
               [[Pipe]]\n\
                 type = PipeInterface\n\
                 enabled = Yes\n\
                 command = example\n\
               [[Radio]]\n\
                 type = RNodeInterface\n\
                 enabled = Yes\n\
                 port = /dev/ttyUSB2\n\
                 frequency = 868000000\n\
                 bandwidth = 125000\n\
                 txpower = 7\n\
                 spreadingfactor = 8\n\
                 codingrate = 5\n",
        );

        assert_eq!(named(&plan, "Serial").policy.bitrate.get(), 115_200);
        assert_eq!(named(&plan, "Kiss").policy.bitrate.get(), 1_200);
        assert_eq!(named(&plan, "Pipe").policy.bitrate.get(), 1_000_000);
        assert_eq!(named(&plan, "Radio").policy.bitrate.get(), 3_125);
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
        assert_eq!(hub.policy.mode, InterfaceMode::Gateway);
        assert!(hub
            .unapplied
            .contains(&UnappliedSetting::AnnounceBandwidthCap));
        assert!(hub.unapplied.contains(&UnappliedSetting::AnnounceRateLimit));
        assert_eq!(
            hub.access,
            InterfaceAccessPlan::Ifac {
                network_name: Some("secret-net".to_string()),
                passphrase: None,
                size: IfacSize::WIDE,
            }
        );
        assert!(matches!(
            hub.medium,
            PlannedMedium::TcpClient {
                framing: TcpWireFraming::Kiss,
                ..
            }
        ));
        assert!(!hub
            .unapplied
            .contains(&UnappliedSetting::MediumOption("kiss_framing")));
    }

    #[test]
    fn ifac_defaults_follow_the_reference_mediums() {
        let plan = plan_of(
            "[interfaces]\n[[Internet]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 1\nnetwork_name = n\n\
             [[Radio]]\ntype = SerialInterface\nenabled = Yes\nport = /dev/ttyUSB0\npassphrase = p\n",
        );
        assert!(matches!(
            named(&plan, "Internet").access,
            InterfaceAccessPlan::Ifac {
                size: IfacSize::WIDE,
                ..
            }
        ));
        assert!(matches!(
            named(&plan, "Radio").access,
            InterfaceAccessPlan::Ifac {
                size: IfacSize::NARROW,
                ..
            }
        ));
    }

    #[test]
    fn ifac_size_is_a_bit_count_floored_to_whole_bytes() {
        let plan = plan_of(
            "[interfaces]\n[[Seven]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 1\nnetwork_name = n\nifac_size = 7\n\
             [[SeventyOne]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 2\nnetwork_name = n\nifac_size = 71\n\
             [[FiveNineteen]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 3\nnetwork_name = n\nifac_size = 519\n",
        );
        assert!(matches!(
            named(&plan, "Seven").access,
            InterfaceAccessPlan::Ifac {
                size: IfacSize::WIDE,
                ..
            }
        ));
        assert!(matches!(
            named(&plan, "SeventyOne").access,
            InterfaceAccessPlan::Ifac { size, .. } if size.bytes() == 8
        ));
        assert!(matches!(
            named(&plan, "FiveNineteen").access,
            InterfaceAccessPlan::Ifac {
                size: IfacSize::MAX,
                ..
            }
        ));
    }

    #[test]
    fn an_oversize_ifac_defers_but_size_alone_does_not_enable_access() {
        let protected = plan_of(
            "[interfaces]\n[[TooWide]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 1\nnetwork_name = n\nifac_size = 520\n",
        );
        assert_eq!(
            protected.deferred[0].why,
            DeferReason::InvalidSetting { key: "ifac_size" }
        );

        let open = plan_of(
            "[interfaces]\n[[Open]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 1\nifac_size = 520\n",
        );
        assert_eq!(named(&open, "Open").access, InterfaceAccessPlan::Open);
    }

    #[test]
    fn a_clean_interface_carries_no_unapplied_noise() {
        let plan = plan_of(STOCK);
        assert!(named(&plan, "Hub").unapplied.is_empty());
        assert!(named(&plan, "Default Interface").unapplied.is_empty());
    }
}
