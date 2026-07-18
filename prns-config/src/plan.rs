//! The reference-to-ours mapping layer: a faithful [`ReferenceConfig`] becomes a [`DaemonPlan`],
//! the host-agnostic description of the node a daemon should stand up.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use prns_core::interface_discovery::{
    AutoConnectPolicy, DiscoverySourcePolicy, InterfaceDiscoveryPolicy, StampCost,
    DEFAULT_STAMP_COST,
};
use prns_core::interfaces::ax25_kiss::core as ax25_core;
use prns_core::interfaces::i2p::core as i2p_core;
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
    AnnounceBandwidthCap, AnnounceRateLimit, BitrateBps, ConfiguredInterfacePolicy,
    EffectiveInterfacePolicy, EgressCapability, FrequencyMilliHertz, IngressCapability,
    InterfaceCommonPolicy, InterfaceDefaults, InterfaceForwardingPolicy, InterfaceMode, MtuBytes,
    MtuPolicy,
};
use prns_core::routing::links::MAX_LINK_MTU;
use prns_core::units::DurationMillis;

use crate::reference::i2p::{validate_peer, validate_peers};
use crate::reference::keys::{
    common as common_key, global as global_key, interface as interface_key, logging as logging_key,
    section as section_key,
};
use crate::reference::{
    ReferenceConfig, ReferenceInterface, ReferenceMode, ReferenceParams, ReferenceValue,
};
use crate::{ConfigDiagnostic, ConfigDiagnosticCode, ConfigErrors, ConfigReport, SourceLocations};

mod rnode_multi;

pub use rnode_multi::{RNodeMultiDevicePlan, RNodeMultiMemberPlan};

/// The complete, host-agnostic description of a node to stand up, projected from a stock RNS config.
#[derive(Debug, Clone, PartialEq)]
pub struct DaemonPlan {
    pub transport: TransportPlan,
    /// Whether this node hosts a shared instance for local RNS apps (RNS `share_instance`, default on).
    pub shared_instance: SharedInstance,
    pub protocol: ProtocolPlan,
    pub logging: LoggingPlan,
    pub panic_on_interface_error: bool,
    pub network_identity_path: Option<PathBuf>,
    pub discovery: InterfaceDiscoveryPolicy,
    pub interfaces: Vec<PlannedInterface>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportPlan {
    Routing,
    Leaf(TransportIdentityPolicy),
}

impl TransportPlan {
    pub const fn routing_enabled(self) -> bool {
        matches!(self, Self::Routing)
    }

    pub const fn identity_policy(self) -> TransportIdentityPolicy {
        match self {
            Self::Routing => TransportIdentityPolicy::Persistent,
            Self::Leaf(identity) => identity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportIdentityPolicy {
    Persistent,
    Ephemeral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolPlan {
    pub randomize_local_hop_count: bool,
    pub link_mtu_discovery: bool,
    pub use_implicit_proof: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoggingPlan {
    pub level: LogLevel,
    pub timestamps: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogLevel(u8);

impl LogLevel {
    pub const DEFAULT: Self = Self(4);

    pub const fn new(level: u8) -> Option<Self> {
        if level <= 7 {
            Some(Self(level))
        } else {
            None
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Whether the node hosts a shared instance, and on which ports if so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedInstance {
    /// The local data bus and its control RPC are served.
    Enabled {
        name: String,
        transport: SharedInstanceTransport,
        instance_port: u16,
        control_port: u16,
        rpc_key: Option<Vec<u8>>,
        forced_bitrate: Option<BitrateBps>,
    },
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedInstanceTransport {
    Tcp,
    Unix,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedInterface {
    pub name: String,
    pub policy: EffectiveInterfacePolicy,
    pub access: InterfaceAccessPlan,
    pub medium: PlannedMedium,
    pub discovery: InterfaceDiscoveryPlan,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressFamilyPreference {
    System,
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpTunnelMode {
    Direct,
    I2p,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectLimit {
    Unlimited,
    Attempts(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectTimeoutSeconds(u64);

impl ConnectTimeoutSeconds {
    pub const fn new(seconds: u64) -> Self {
        Self(seconds)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpDialPlan {
    pub host: String,
    pub port: u16,
    pub connect_timeout: ConnectTimeoutSeconds,
    pub reconnect_limit: ReconnectLimit,
    pub address_family: AddressFamilyPreference,
    pub tunnel: TcpTunnelMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TcpListenHost {
    Any,
    Address(String),
    Device(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpListenPlan {
    pub host: TcpListenHost,
    pub port: u16,
    pub address_family: AddressFamilyPreference,
    pub tunnel: TcpTunnelMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UdpEndpointHost {
    Address(String),
    DeviceBroadcast(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpEndpointPlan {
    pub host: UdpEndpointHost,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UdpFlowPlan {
    ReceiveOnly {
        listen: UdpEndpointPlan,
    },
    SendOnly {
        forward: UdpEndpointPlan,
    },
    Bidirectional {
        listen: UdpEndpointPlan,
        forward: UdpEndpointPlan,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialDataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialParity {
    None,
    Even,
    Odd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialStopBits {
    One,
    Two,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerialLinePlan {
    baud: u32,
    data_bits: SerialDataBits,
    parity: SerialParity,
    stop_bits: SerialStopBits,
}

impl SerialLinePlan {
    pub const fn baud(self) -> u32 {
        self.baud
    }

    pub const fn data_bits(self) -> SerialDataBits {
        self.data_bits
    }

    pub const fn parity(self) -> SerialParity {
        self.parity
    }

    pub const fn stop_bits(self) -> SerialStopBits {
        self.stop_bits
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyCommandFlowControl {
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationIdentificationPlan {
    callsign: String,
    interval_seconds: u64,
}

impl StationIdentificationPlan {
    pub fn callsign(&self) -> &str {
        &self.callsign
    }

    pub const fn interval_seconds(&self) -> u64 {
        self.interval_seconds
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AirtimeLimitCentiPercent(u16);

impl AirtimeLimitCentiPercent {
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipeRespawnDelay(std::time::Duration);

impl PipeRespawnDelay {
    pub const fn get(self) -> std::time::Duration {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeCommandPlan {
    source: String,
    argv: Vec<String>,
}

impl PipeCommandPlan {
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn argv(&self) -> &[String] {
        &self.argv
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct I2pPeerPlan(String);

impl I2pPeerPlan {
    fn new(value: String) -> Result<Self, PlanErrorKind> {
        validate_peer(&value).map_err(|_| PlanErrorKind::InvalidSetting {
            key: interface_key::PEERS,
        })?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pPeersPlan(Vec<I2pPeerPlan>);

impl I2pPeersPlan {
    fn new(peers: Vec<String>) -> Result<Self, PlanErrorKind> {
        validate_peers(peers.iter().map(String::as_str)).map_err(|_| {
            PlanErrorKind::InvalidSetting {
                key: interface_key::PEERS,
            }
        })?;
        peers
            .into_iter()
            .map(I2pPeerPlan::new)
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    pub fn iter(&self) -> impl Iterator<Item = &I2pPeerPlan> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2pReachabilityPlan {
    OutboundOnly,
    Connectable,
}

impl I2pReachabilityPlan {
    pub const fn is_connectable(self) -> bool {
        matches!(self, Self::Connectable)
    }
}

/// The wire a planned interface runs on. Only mediums a host can stand up appear here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedMedium {
    /// RNS `AutoInterface`: multicast LAN discovery plus unicast peers (our `AutoWifi`).
    AutoWifi {
        group: Option<String>,
    },
    /// RNS `TCPClientInterface`: dial one peer.
    TcpClient {
        connection: TcpDialPlan,
        framing: TcpWireFraming,
    },
    /// RNS `TCPServerInterface`: accept peers on the configured listener.
    TcpServer {
        listener: TcpListenPlan,
        framing: TcpWireFraming,
    },
    /// RNS `UDPInterface`: receive, send, or do both over configured datagram endpoints.
    Udp {
        flow: UdpFlowPlan,
    },
    /// RNS `SerialInterface`: a configured serial device.
    Serial {
        device: String,
        line: SerialLinePlan,
    },
    /// RNS `KISSInterface`: a KISS TNC on a configured serial line, with the CSMA/timing config
    /// written to the TNC at startup (the millisecond values as the operator gave them).
    Kiss {
        device: String,
        line: SerialLinePlan,
        preamble_ms: u32,
        txtail_ms: u32,
        persistence: u8,
        slottime_ms: u32,
        flow_control: ReadyCommandFlowControl,
        station_id: Option<StationIdentificationPlan>,
    },
    /// RNS `AX25KISSInterface`: a KISS TNC carrying AX.25 UI frames, sourced from `callsign`/`ssid`.
    /// The callsign/SSID are validated before the daemon plan is constructed.
    Ax25Kiss {
        device: String,
        line: SerialLinePlan,
        preamble_ms: u32,
        txtail_ms: u32,
        persistence: u8,
        slottime_ms: u32,
        flow_control: ReadyCommandFlowControl,
        callsign: String,
        ssid: u8,
    },
    /// RNS `PipeInterface`: a subprocess `command` whose stdout/stdin carries HDLC-framed packets,
    /// respawned after the configured delay when it exits.
    Pipe {
        command: PipeCommandPlan,
        respawn_delay: PipeRespawnDelay,
    },
    /// RNS `RNodeInterface`: a LoRa RNode driven over a USB-serial KISS link, configured to a radio
    /// channel at bring-up. The radio parameters are required; the airtime locks are the wire-scaled
    /// `int(percent * 100)` values, absent when unconfigured.
    Rnode {
        device: String,
        frequency_hz: u64,
        bandwidth_hz: u32,
        txpower_dbm: i16,
        spreading_factor: u8,
        coding_rate: u8,
        flow_control: ReadyCommandFlowControl,
        station_id: Option<StationIdentificationPlan>,
        airtime_limit_short: Option<AirtimeLimitCentiPercent>,
        airtime_limit_long: Option<AirtimeLimitCentiPercent>,
    },
    RnodeMulti {
        member: RNodeMultiMemberPlan,
    },
    /// RNS `BackboneInterface`: the listening end of a TCP backbone link.
    Backbone {
        listener: TcpListenPlan,
    },
    /// RNS `BackboneClientInterface`: dial one backbone peer. Wire-identical to
    /// [`TcpClient`](Self::TcpClient).
    BackboneClient {
        connection: TcpDialPlan,
    },
    I2p {
        peers: I2pPeersPlan,
        reachability: I2pReachabilityPlan,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanErrorKind {
    UnsupportedKind,
    MissingRequiredField { key: &'static str },
    InvalidSetting { key: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanError {
    interface_name: String,
    interface_type: String,
    subinterface_name: Option<String>,
    kind: PlanErrorKind,
}

pub fn parse_and_plan(input: &str) -> Result<ConfigReport<DaemonPlan>, ConfigErrors> {
    parse_and_plan_named("config", input)
}

pub fn parse_and_plan_named(
    source: impl Into<String>,
    input: &str,
) -> Result<ConfigReport<DaemonPlan>, ConfigErrors> {
    let report = crate::reference::parse_named(source, input)?;
    let ConfigReport {
        value,
        warnings,
        source,
        locations,
    } = report;
    match build_plan(&value) {
        Ok(value) => Ok(ConfigReport {
            value,
            warnings,
            source,
            locations,
        }),
        Err(errors) => {
            let mut diagnostics = errors
                .iter()
                .map(|error| planning_diagnostic(&source, &locations, error))
                .collect::<Vec<_>>();
            diagnostics.extend(warnings);
            Err(ConfigErrors::new(diagnostics))
        }
    }
}

fn build_plan(config: &ReferenceConfig) -> Result<DaemonPlan, Vec<PlanError>> {
    let mut interfaces = Vec::new();
    let mut errors = Vec::new();
    let transport = transport_plan(config);
    let common = global_common_policy(config);
    let announce_rate = global_announce_rate(config);
    for interface in &config.interfaces {
        if matches!(interface.params, ReferenceParams::RnodeMulti { .. }) {
            match rnode_multi::plan(
                interface,
                common,
                announce_rate,
                transport.routing_enabled(),
            ) {
                Ok(planned) => interfaces.extend(planned),
                Err(failure) => errors.push(PlanError {
                    interface_name: interface.name.clone(),
                    interface_type: interface.type_name.clone(),
                    subinterface_name: failure.subinterface_name,
                    kind: failure.kind,
                }),
            }
            continue;
        }
        match plan_interface(
            interface,
            common,
            announce_rate,
            transport.routing_enabled(),
        ) {
            Ok(planned) => interfaces.push(planned),
            Err(kind) => errors.push(PlanError {
                interface_name: interface.name.clone(),
                interface_type: interface.type_name.clone(),
                subinterface_name: None,
                kind,
            }),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(DaemonPlan {
        transport,
        shared_instance: shared_instance(config),
        protocol: ProtocolPlan {
            randomize_local_hop_count: global_bool(
                &config.globals,
                global_key::LOCAL_HOPS_DELTA,
                false,
            ),
            link_mtu_discovery: global_bool(&config.globals, global_key::LINK_MTU_DISCOVERY, true),
            use_implicit_proof: global_bool(&config.globals, global_key::USE_IMPLICIT_PROOF, true),
        },
        logging: logging_plan(config),
        panic_on_interface_error: global_bool(
            &config.globals,
            global_key::PANIC_ON_INTERFACE_ERROR,
            false,
        ),
        network_identity_path: config.network_identity_path.as_deref().map(PathBuf::from),
        discovery: discovery_policy(config),
        interfaces,
    })
}

fn planning_diagnostic(
    source: &str,
    locations: &SourceLocations,
    error: &PlanError,
) -> ConfigDiagnostic {
    let display_section = error.subinterface_name.as_ref().map_or_else(
        || format!("[interfaces] > [[{}]]", error.interface_name),
        |name| format!("[interfaces] > [[{}]] > [[[{name}]]]", error.interface_name),
    );
    let correction_section = error.subinterface_name.as_ref().map_or_else(
        || format!("[[{}]]", error.interface_name),
        |name| format!("[[[{name}]]]"),
    );
    let configured_subject = if error.subinterface_name.is_some() {
        "enabled RNodeMulti subinterface"
    } else {
        "enabled interface"
    };
    let (code, key, message, accepted, correction) = match error.kind {
        PlanErrorKind::UnsupportedKind => (
            ConfigDiagnosticCode::UnsupportedInterface,
            interface_key::TYPE,
            format!(
                "interface type {:?} is not available in this build",
                error.interface_type
            ),
            "an interface type supported by this build".to_string(),
            format!(
                "set `{}` = No for [[{}]]",
                interface_key::ENABLED,
                error.interface_name
            ),
        ),
        PlanErrorKind::MissingRequiredField { key } => (
            ConfigDiagnosticCode::MissingRequiredKey,
            key,
            format!("{configured_subject} is missing required setting {key:?}"),
            format!("a valid {key} value"),
            format!("add `{key} = value` under {correction_section}"),
        ),
        PlanErrorKind::InvalidSetting { key } => (
            ConfigDiagnosticCode::InvalidValue,
            key,
            format!("setting {key:?} cannot be represented by this build"),
            format!("a valid, representable {key} value"),
            format!("replace `{key}` under {correction_section}"),
        ),
    };
    let mut path = vec![section_key::INTERFACES, error.interface_name.as_str()];
    if let Some(subinterface) = &error.subinterface_name {
        path.push(subinterface);
    }
    let section_path = path.clone();
    path.push(key);
    let line = locations
        .line(path.iter().copied())
        .or_else(|| locations.line(section_path.iter().copied()));
    ConfigDiagnostic::new(
        code,
        source,
        line.unwrap_or(1),
        format!("{display_section} > {key}"),
        None,
        message,
        Some(accepted),
        correction,
    )
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
    if global_bool(&config.globals, global_key::SHARE_INSTANCE, true) {
        SharedInstance::Enabled {
            name: global_string(&config.globals, global_key::INSTANCE_NAME)
                .unwrap_or_else(|| "default".to_string()),
            transport: match global_string(&config.globals, global_key::SHARED_INSTANCE_TYPE)
                .map(|value| value.trim().to_ascii_lowercase())
                .as_deref()
            {
                Some("tcp") => SharedInstanceTransport::Tcp,
                Some("unix") | None => SharedInstanceTransport::Unix,
                Some(_) => SharedInstanceTransport::Unix,
            },
            instance_port: global_u16(&config.globals, global_key::SHARED_INSTANCE_PORT)
                .unwrap_or(37_428),
            control_port: global_u16(&config.globals, global_key::INSTANCE_CONTROL_PORT)
                .unwrap_or(37_429),
            rpc_key: global_string(&config.globals, global_key::RPC_KEY)
                .and_then(|value| decode_hex(&value)),
            forced_bitrate: global_u64(&config.globals, global_key::FORCE_SHARED_INSTANCE_BITRATE)
                .and_then(BitrateBps::new),
        }
    } else {
        SharedInstance::Disabled
    }
}

fn transport_plan(config: &ReferenceConfig) -> TransportPlan {
    let routing = global_bool(&config.globals, global_key::ENABLE_TRANSPORT, false);
    if routing {
        TransportPlan::Routing
    } else {
        TransportPlan::Leaf(
            if global_bool(
                &config.globals,
                global_key::STATIC_TRANSPORT_IDENTITY,
                false,
            ) {
                TransportIdentityPolicy::Persistent
            } else {
                TransportIdentityPolicy::Ephemeral
            },
        )
    }
}

fn logging_plan(config: &ReferenceConfig) -> LoggingPlan {
    let logging = config.other_sections.get(section_key::LOGGING);
    LoggingPlan {
        level: logging
            .and_then(|section| global_u64(section, logging_key::LEVEL))
            .and_then(|level| u8::try_from(level).ok())
            .and_then(LogLevel::new)
            .unwrap_or(LogLevel::DEFAULT),
        timestamps: logging
            .map(|section| global_bool(section, logging_key::TIMESTAMPS, true))
            .unwrap_or(true),
    }
}

fn plan_interface(
    interface: &ReferenceInterface,
    global_common: InterfaceCommonPolicy,
    global_announce_rate: AnnounceRateLimit,
    transport_enabled: bool,
) -> Result<PlannedInterface, PlanErrorKind> {
    let medium = plan_medium(interface)?;
    let access = plan_access(interface, &medium)?;
    let discovery = plan_interface_discovery(interface, &medium);
    let policy = effective_policy(
        interface,
        &medium,
        &discovery,
        global_common,
        global_announce_rate,
        transport_enabled,
        MemberEgressPolicy::Inherit,
    )?;
    Ok(PlannedInterface {
        name: interface.name.clone(),
        policy,
        access,
        medium,
        discovery,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberEgressPolicy {
    Inherit,
    Disabled,
}

impl MemberEgressPolicy {
    const fn from_outgoing(outgoing: Option<bool>) -> Self {
        if matches!(outgoing, Some(false)) {
            Self::Disabled
        } else {
            Self::Inherit
        }
    }
}

fn effective_policy(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
    discovery: &InterfaceDiscoveryPlan,
    global_common: InterfaceCommonPolicy,
    global_announce_rate: AnnounceRateLimit,
    transport_enabled: bool,
    member_egress: MemberEgressPolicy,
) -> Result<EffectiveInterfacePolicy, PlanErrorKind> {
    let bitrate = interface
        .bitrate
        .map(|bitrate| {
            BitrateBps::new(bitrate).ok_or(PlanErrorKind::InvalidSetting {
                key: interface_key::BITRATE,
            })
        })
        .transpose()?;
    let mtu = configured_mtu(interface)?;
    let defaults = interface_defaults(medium)?;
    let ingress = if matches!(
        medium,
        PlannedMedium::Udp {
            flow: UdpFlowPlan::SendOnly { .. }
        }
    ) {
        IngressCapability::Disabled
    } else {
        defaults.capabilities.ingress
    };
    let egress = if interface.outgoing == Some(false)
        || member_egress == MemberEgressPolicy::Disabled
        || matches!(
            medium,
            PlannedMedium::Udp {
                flow: UdpFlowPlan::ReceiveOnly { .. }
            }
        ) {
        EgressCapability::Disabled
    } else {
        defaults.capabilities.egress
    };
    let capabilities = (ingress != defaults.capabilities.ingress
        || egress != defaults.capabilities.egress)
        .then_some(prns_core::interfaces::InterfaceCapabilities { ingress, egress });
    let announce_bandwidth_cap = interface
        .announce_cap
        .map(announce_bandwidth_cap)
        .transpose()?;
    let announce_rate_limit =
        planned_announce_rate_limit(interface, global_announce_rate, transport_enabled)?;
    let common = interface_common_policy(interface, global_common)?;
    Ok(defaults.configured(ConfiguredInterfacePolicy {
        capabilities,
        mode: Some(planned_mode(interface, discovery)),
        bitrate,
        mtu,
        announce_rate_limit,
        announce_bandwidth_cap,
        common: Some(common),
        ..ConfiguredInterfacePolicy::default()
    }))
}

enum AnnounceRateSource {
    Interface { target_seconds: u64 },
    TransportDefault(AnnounceRateLimit),
}

fn planned_announce_rate_limit(
    interface: &ReferenceInterface,
    global: AnnounceRateLimit,
    transport_enabled: bool,
) -> Result<Option<AnnounceRateLimit>, PlanErrorKind> {
    let source = match (interface.announce_rate_target, transport_enabled) {
        (Some(target_seconds), _) => AnnounceRateSource::Interface { target_seconds },
        (None, true) => AnnounceRateSource::TransportDefault(global),
        (None, false) => return Ok(None),
    };
    let (target_ms, default_grace, default_penalty_ms) = match source {
        AnnounceRateSource::Interface { target_seconds } => (
            checked_milliseconds(target_seconds, interface_key::ANNOUNCE_RATE_TARGET)?,
            0,
            0,
        ),
        AnnounceRateSource::TransportDefault(defaults) => {
            (defaults.target_ms, defaults.grace, defaults.penalty_ms)
        }
    };
    let grace = interface
        .announce_rate_grace
        .map(u16::try_from)
        .transpose()
        .map_err(|_| PlanErrorKind::InvalidSetting {
            key: interface_key::ANNOUNCE_RATE_GRACE,
        })?
        .unwrap_or(default_grace);
    let penalty_ms = interface
        .announce_rate_penalty
        .map(|seconds| checked_milliseconds(seconds, interface_key::ANNOUNCE_RATE_PENALTY))
        .transpose()?
        .unwrap_or(default_penalty_ms);
    Ok(Some(AnnounceRateLimit {
        target_ms,
        grace,
        penalty_ms,
    }))
}

fn checked_milliseconds(seconds: u64, key: &'static str) -> Result<u64, PlanErrorKind> {
    seconds
        .checked_mul(1_000)
        .ok_or(PlanErrorKind::InvalidSetting { key })
}

fn interface_defaults(medium: &PlannedMedium) -> Result<InterfaceDefaults, PlanErrorKind> {
    match medium {
        PlannedMedium::AutoWifi { .. } => Ok(wifi_core::DEFAULTS),
        PlannedMedium::TcpClient { .. }
        | PlannedMedium::TcpServer { .. }
        | PlannedMedium::Backbone { .. }
        | PlannedMedium::BackboneClient { .. } => Ok(tcp_core::DEFAULTS),
        PlannedMedium::Udp { .. } => Ok(udp_core::DEFAULTS),
        PlannedMedium::I2p { .. } => Ok(i2p_core::DEFAULTS),
        PlannedMedium::Serial { line, .. } => {
            let bitrate =
                BitrateBps::new(u64::from(line.baud())).ok_or(PlanErrorKind::InvalidSetting {
                    key: interface_key::SPEED,
                })?;
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
        } => rnode_defaults(*spreading_factor, *coding_rate, *bandwidth_hz),
        PlannedMedium::RnodeMulti { member } => {
            let radio = member.radio();
            rnode_defaults(
                radio.spreading_factor(),
                radio.coding_rate(),
                radio.bandwidth_hz(),
            )
        }
    }
}

fn rnode_defaults(
    spreading_factor: u8,
    coding_rate: u8,
    bandwidth_hz: u32,
) -> Result<InterfaceDefaults, PlanErrorKind> {
    let raw = rnode_policy::nominal_bitrate_bps(spreading_factor, coding_rate, bandwidth_hz);
    let bitrate = BitrateBps::new(u64::from(raw)).ok_or(PlanErrorKind::InvalidSetting {
        key: interface_key::BANDWIDTH,
    })?;
    Ok(rnode_policy::defaults_for_bitrate(bitrate))
}

fn configured_mtu(interface: &ReferenceInterface) -> Result<Option<MtuPolicy>, PlanErrorKind> {
    let fixed_mtu = match &interface.params {
        ReferenceParams::TcpClient { fixed_mtu, .. }
        | ReferenceParams::TcpServer { fixed_mtu, .. } => *fixed_mtu,
        _ => None,
    };
    fixed_mtu
        .map(|fixed_mtu| {
            if fixed_mtu > MAX_LINK_MTU {
                return Err(PlanErrorKind::InvalidSetting {
                    key: interface_key::FIXED_MTU,
                });
            }
            MtuBytes::new(fixed_mtu)
                .map(MtuPolicy::Fixed)
                .ok_or(PlanErrorKind::InvalidSetting {
                    key: interface_key::FIXED_MTU,
                })
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
                key: interface_key::REACHABLE_ON,
            },
        )
    };
    let kiss = || {
        Ok(DiscoveryAdvertisementPlan::Kiss {
            frequency_hz: interface.discovery.frequency_hz.ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::DISCOVERY_FREQUENCY,
                },
            )?,
            bandwidth_hz: interface.discovery.bandwidth_hz.ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::DISCOVERY_BANDWIDTH,
                },
            )?,
            modulation: interface.discovery.modulation.clone().ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::DISCOVERY_MODULATION,
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
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::LISTEN_PORT,
                },
            )?,
        }),
        (
            PlannedMedium::TcpServer { .. },
            ReferenceParams::TcpServer {
                listen_port, port, ..
            },
        ) => Ok(DiscoveryAdvertisementPlan::TcpServer {
            reachable_on: reachable_on()?,
            port: port.or(*listen_port).ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::LISTEN_PORT,
                },
            )?,
        }),
        (
            PlannedMedium::Rnode {
                frequency_hz,
                bandwidth_hz,
                spreading_factor,
                coding_rate,
                ..
            },
            ReferenceParams::Rnode { .. },
        ) => Ok(rnode_discovery_advertisement(
            *frequency_hz,
            *bandwidth_hz,
            *spreading_factor,
            *coding_rate,
        )),
        (PlannedMedium::RnodeMulti { member }, ReferenceParams::RnodeMulti { .. }) => {
            let radio = member.radio();
            Ok(rnode_discovery_advertisement(
                u64::from(radio.frequency().hz()),
                radio.bandwidth_hz(),
                radio.spreading_factor(),
                radio.coding_rate(),
            ))
        }
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
                key: interface_key::KISS_FRAMING,
            })
        }
        _ => Err(DiscoveryPublicationProblem::UnsupportedInterfaceType),
    }
}

fn rnode_discovery_advertisement(
    frequency_hz: u64,
    bandwidth_hz: u32,
    spreading_factor: u8,
    coding_rate: u8,
) -> DiscoveryAdvertisementPlan {
    DiscoveryAdvertisementPlan::RNode {
        frequency_hz,
        bandwidth_hz,
        spreading_factor,
        coding_rate,
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
) -> Result<InterfaceAccessPlan, PlanErrorKind> {
    if interface.network_name.is_none() && interface.passphrase.is_none() {
        return Ok(InterfaceAccessPlan::Open);
    }
    let default_size = match medium {
        PlannedMedium::AutoWifi { .. }
        | PlannedMedium::TcpClient { .. }
        | PlannedMedium::TcpServer { .. }
        | PlannedMedium::Udp { .. }
        | PlannedMedium::Backbone { .. }
        | PlannedMedium::BackboneClient { .. }
        | PlannedMedium::I2p { .. } => IfacSize::WIDE,
        PlannedMedium::Serial { .. }
        | PlannedMedium::Kiss { .. }
        | PlannedMedium::Ax25Kiss { .. }
        | PlannedMedium::Pipe { .. }
        | PlannedMedium::Rnode { .. }
        | PlannedMedium::RnodeMulti { .. } => IfacSize::NARROW,
    };
    let size = match interface.ifac_size_bits {
        Some(bits) if bits >= 8 => {
            IfacSize::new((bits / 8) as usize).map_err(|_| PlanErrorKind::InvalidSetting {
                key: interface_key::IFAC_SIZE,
            })?
        }
        Some(_) | None => default_size,
    };
    Ok(InterfaceAccessPlan::Ifac {
        network_name: interface.network_name.clone(),
        passphrase: interface.passphrase.clone(),
        size,
    })
}

fn plan_medium(interface: &ReferenceInterface) -> Result<PlannedMedium, PlanErrorKind> {
    match &interface.params {
        ReferenceParams::Auto { group_id, .. } => Ok(PlannedMedium::AutoWifi {
            group: group_id.clone(),
        }),
        ReferenceParams::TcpClient {
            target_host,
            target_port,
            kiss_framing,
            i2p_tunneled,
            connect_timeout,
            max_reconnect_tries,
            fixed_mtu: _,
        } => {
            let host = target_host
                .clone()
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::TARGET_HOST,
                })?;
            let port = target_port.ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::TARGET_PORT,
            })?;
            Ok(PlannedMedium::TcpClient {
                connection: tcp_dial_plan(
                    host,
                    port,
                    *connect_timeout,
                    *max_reconnect_tries,
                    AddressFamilyPreference::System,
                    *i2p_tunneled,
                ),
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
            i2p_tunneled,
            kiss_framing,
            fixed_mtu: _,
        } => {
            let listen_port = port
                .or(*listen_port)
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::LISTEN_PORT,
                })?;
            Ok(PlannedMedium::TcpServer {
                listener: TcpListenPlan {
                    host: tcp_listen_host(listen_ip, device),
                    port: listen_port,
                    address_family: preferred_ip_family(*prefer_ipv6),
                    tunnel: tunnel_mode(*i2p_tunneled),
                },
                framing: if *kiss_framing == Some(true) {
                    TcpWireFraming::Kiss
                } else {
                    TcpWireFraming::Hdlc
                },
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
            let listen = udp_endpoint(
                listen_ip.as_deref(),
                port.or(*listen_port),
                device.as_deref(),
                interface_key::LISTEN_PORT,
            )?;
            let forward = udp_endpoint(
                forward_ip.as_deref(),
                port.or(*forward_port),
                device.as_deref(),
                interface_key::FORWARD_PORT,
            )?;
            let flow = match (listen, forward) {
                (Some(listen), Some(forward)) => UdpFlowPlan::Bidirectional { listen, forward },
                (Some(listen), None) => UdpFlowPlan::ReceiveOnly { listen },
                (None, Some(forward)) => UdpFlowPlan::SendOnly { forward },
                (None, None) => {
                    return Err(PlanErrorKind::MissingRequiredField {
                        key: interface_key::LISTEN_IP,
                    })
                }
            };
            Ok(PlannedMedium::Udp { flow })
        }
        ReferenceParams::Serial {
            port,
            speed,
            databits,
            parity,
            stopbits,
        } => {
            let device = port.clone().ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            Ok(PlannedMedium::Serial {
                device,
                line: serial_line(*speed, *databits, parity.as_deref(), *stopbits)?,
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
            let device = port.clone().ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            Ok(PlannedMedium::Kiss {
                device,
                line: serial_line(*speed, *databits, parity.as_deref(), *stopbits)?,
                preamble_ms: preamble.unwrap_or(RNS_KISS_DEFAULT_PREAMBLE_MS),
                txtail_ms: txtail.unwrap_or(RNS_KISS_DEFAULT_TXTAIL_MS),
                persistence: persistence
                    .map(|p| p.min(u8::MAX as u32) as u8)
                    .unwrap_or(RNS_KISS_DEFAULT_PERSISTENCE),
                slottime_ms: slottime.unwrap_or(RNS_KISS_DEFAULT_SLOTTIME_MS),
                flow_control: ready_command_flow_control(*flow_control),
                station_id: station_identification(id_callsign.as_deref(), *id_interval, None)?,
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
            let device = port.clone().ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            let callsign = callsign
                .clone()
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::CALLSIGN,
                })?;
            let ssid = ssid.ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::SSID,
            })?;
            Ok(PlannedMedium::Ax25Kiss {
                device,
                line: serial_line(*speed, *databits, parity.as_deref(), *stopbits)?,
                preamble_ms: preamble.unwrap_or(RNS_KISS_DEFAULT_PREAMBLE_MS),
                txtail_ms: txtail.unwrap_or(RNS_KISS_DEFAULT_TXTAIL_MS),
                persistence: persistence
                    .map(|p| p.min(u8::MAX as u32) as u8)
                    .unwrap_or(RNS_KISS_DEFAULT_PERSISTENCE),
                slottime_ms: slottime.unwrap_or(RNS_KISS_DEFAULT_SLOTTIME_MS),
                flow_control: ready_command_flow_control(*flow_control),
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
            let device = port.clone().ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::PORT,
            })?;
            let frequency_hz = radio.frequency.ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::FREQUENCY,
            })?;
            let bandwidth_hz = radio.bandwidth.ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::BANDWIDTH,
            })?;
            let spreading_factor =
                radio
                    .spreadingfactor
                    .ok_or(PlanErrorKind::MissingRequiredField {
                        key: interface_key::SPREADINGFACTOR,
                    })?;
            let coding_rate = radio
                .codingrate
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::CODINGRATE,
                })?;
            let txpower_dbm = radio.txpower.ok_or(PlanErrorKind::MissingRequiredField {
                key: interface_key::TXPOWER,
            })?;
            Ok(PlannedMedium::Rnode {
                device,
                frequency_hz,
                bandwidth_hz,
                txpower_dbm,
                spreading_factor,
                coding_rate,
                flow_control: ready_command_flow_control(*flow_control),
                station_id: station_identification(id_callsign.as_deref(), *id_interval, Some(32))?,
                airtime_limit_short: airtime_limit(
                    *airtime_limit_short,
                    interface_key::AIRTIME_LIMIT_SHORT,
                )?,
                airtime_limit_long: airtime_limit(
                    *airtime_limit_long,
                    interface_key::AIRTIME_LIMIT_LONG,
                )?,
            })
        }
        ReferenceParams::Pipe {
            command,
            respawn_delay,
        } => {
            let command = command
                .as_deref()
                .ok_or(PlanErrorKind::MissingRequiredField {
                    key: interface_key::COMMAND,
                })?;
            Ok(PlannedMedium::Pipe {
                command: pipe_command(command)?,
                respawn_delay: pipe_respawn_delay(*respawn_delay)?,
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
            if target_host.is_some() || interface.type_name == "BackboneClientInterface" {
                let host = target_host
                    .clone()
                    .ok_or(PlanErrorKind::MissingRequiredField {
                        key: interface_key::TARGET_HOST,
                    })?;
                let port = port
                    .or(*target_port)
                    .ok_or(PlanErrorKind::MissingRequiredField {
                        key: interface_key::TARGET_PORT,
                    })?;
                Ok(PlannedMedium::BackboneClient {
                    connection: tcp_dial_plan(
                        host,
                        port,
                        *connect_timeout,
                        *max_reconnect_tries,
                        preferred_ip_family(*prefer_ipv6),
                        *i2p_tunneled,
                    ),
                })
            } else {
                let bind_port =
                    (*port)
                        .or(*listen_port)
                        .ok_or(PlanErrorKind::MissingRequiredField {
                            key: interface_key::LISTEN_PORT,
                        })?;
                Ok(PlannedMedium::Backbone {
                    listener: TcpListenPlan {
                        host: tcp_listen_host(listen_ip, device),
                        port: bind_port,
                        address_family: preferred_ip_family(*prefer_ipv6),
                        tunnel: TcpTunnelMode::Direct,
                    },
                })
            }
        }
        ReferenceParams::I2p { peers, connectable } => Ok(PlannedMedium::I2p {
            peers: I2pPeersPlan::new(peers.clone().unwrap_or_default())?,
            reachability: if *connectable == Some(true) {
                I2pReachabilityPlan::Connectable
            } else {
                I2pReachabilityPlan::OutboundOnly
            },
        }),
        _ => Err(PlanErrorKind::UnsupportedKind),
    }
}

fn tcp_dial_plan(
    host: String,
    port: u16,
    connect_timeout_seconds: Option<u64>,
    max_reconnect_tries: Option<u32>,
    address_family: AddressFamilyPreference,
    i2p_tunneled: Option<bool>,
) -> TcpDialPlan {
    TcpDialPlan {
        host,
        port,
        connect_timeout: ConnectTimeoutSeconds::new(
            connect_timeout_seconds.unwrap_or(RNS_TCP_CONNECT_TIMEOUT_SECONDS),
        ),
        reconnect_limit: max_reconnect_tries
            .map(ReconnectLimit::Attempts)
            .unwrap_or(ReconnectLimit::Unlimited),
        address_family,
        tunnel: tunnel_mode(i2p_tunneled),
    }
}

fn tcp_listen_host(listen_ip: &Option<String>, device: &Option<String>) -> TcpListenHost {
    match (device, listen_ip) {
        (Some(device), _) => TcpListenHost::Device(device.clone()),
        (None, Some(address)) => TcpListenHost::Address(address.clone()),
        (None, None) => TcpListenHost::Any,
    }
}

const fn preferred_ip_family(prefer_ipv6: Option<bool>) -> AddressFamilyPreference {
    match prefer_ipv6 {
        Some(true) => AddressFamilyPreference::Ipv6,
        Some(false) | None => AddressFamilyPreference::Ipv4,
    }
}

const fn tunnel_mode(i2p_tunneled: Option<bool>) -> TcpTunnelMode {
    match i2p_tunneled {
        Some(true) => TcpTunnelMode::I2p,
        Some(false) | None => TcpTunnelMode::Direct,
    }
}

fn udp_endpoint(
    address: Option<&str>,
    port: Option<u16>,
    device: Option<&str>,
    port_key: &'static str,
) -> Result<Option<UdpEndpointPlan>, PlanErrorKind> {
    if address.is_none() && port.is_none() {
        return Ok(None);
    }
    let host = match (address, device) {
        (Some(address), _) => Some(UdpEndpointHost::Address(address.to_string())),
        (None, Some(device)) => Some(UdpEndpointHost::DeviceBroadcast(device.to_string())),
        (None, None) => None,
    };
    match (host, port) {
        (Some(host), Some(port)) => Ok(Some(UdpEndpointPlan { host, port })),
        (Some(_), None) => Err(PlanErrorKind::MissingRequiredField { key: port_key }),
        (None, _) => Ok(None),
    }
}

const RNS_DEFAULT_SERIAL_BAUD: u32 = 9_600;
const RNS_TCP_CONNECT_TIMEOUT_SECONDS: u64 = 5;

/// RNS `KISSInterface` TNC defaults, mirrored from `interfaces::kiss::core` (kept in this crate so
/// the config planner stays independent of the interface crate): 350 ms preamble, 20 ms TX-tail,
/// persistence 64, 20 ms slot time.
const RNS_KISS_DEFAULT_PREAMBLE_MS: u32 = 350;
const RNS_KISS_DEFAULT_TXTAIL_MS: u32 = 20;
const RNS_KISS_DEFAULT_PERSISTENCE: u8 = 64;
const RNS_KISS_DEFAULT_SLOTTIME_MS: u32 = 20;

const RNS_PIPE_DEFAULT_RESPAWN_SECONDS: u64 = 5;

fn serial_line(
    speed: Option<u32>,
    data_bits: Option<u8>,
    parity: Option<&str>,
    stop_bits: Option<u8>,
) -> Result<SerialLinePlan, PlanErrorKind> {
    let baud = speed.unwrap_or(RNS_DEFAULT_SERIAL_BAUD);
    if u64::from(baud) < BitrateBps::MINIMUM {
        return Err(PlanErrorKind::InvalidSetting {
            key: interface_key::SPEED,
        });
    }
    let data_bits = match data_bits.unwrap_or(8) {
        5 => SerialDataBits::Five,
        6 => SerialDataBits::Six,
        7 => SerialDataBits::Seven,
        8 => SerialDataBits::Eight,
        _ => {
            return Err(PlanErrorKind::InvalidSetting {
                key: interface_key::DATABITS,
            })
        }
    };
    let parity = match parity.unwrap_or("n").trim().to_ascii_lowercase().as_str() {
        "n" | "none" => SerialParity::None,
        "e" | "even" => SerialParity::Even,
        "o" | "odd" => SerialParity::Odd,
        _ => {
            return Err(PlanErrorKind::InvalidSetting {
                key: interface_key::PARITY,
            })
        }
    };
    let stop_bits = match stop_bits.unwrap_or(1) {
        1 => SerialStopBits::One,
        2 => SerialStopBits::Two,
        _ => {
            return Err(PlanErrorKind::InvalidSetting {
                key: interface_key::STOPBITS,
            })
        }
    };
    Ok(SerialLinePlan {
        baud,
        data_bits,
        parity,
        stop_bits,
    })
}

fn ready_command_flow_control(configured: Option<bool>) -> ReadyCommandFlowControl {
    match configured {
        Some(true) => ReadyCommandFlowControl::Enabled,
        Some(false) | None => ReadyCommandFlowControl::Disabled,
    }
}

fn station_identification(
    callsign: Option<&str>,
    interval_seconds: Option<u64>,
    maximum_callsign_bytes: Option<usize>,
) -> Result<Option<StationIdentificationPlan>, PlanErrorKind> {
    let (callsign, interval_seconds) = match (callsign, interval_seconds) {
        (None, None) => return Ok(None),
        (Some(_), None) => {
            return Err(PlanErrorKind::MissingRequiredField {
                key: interface_key::ID_INTERVAL,
            })
        }
        (None, Some(_)) => {
            return Err(PlanErrorKind::MissingRequiredField {
                key: interface_key::ID_CALLSIGN,
            })
        }
        (Some(callsign), Some(interval_seconds)) => (callsign, interval_seconds),
    };
    if callsign.is_empty() || maximum_callsign_bytes.is_some_and(|maximum| callsign.len() > maximum)
    {
        return Err(PlanErrorKind::InvalidSetting {
            key: interface_key::ID_CALLSIGN,
        });
    }
    Ok(Some(StationIdentificationPlan {
        callsign: callsign.to_string(),
        interval_seconds,
    }))
}

fn airtime_limit(
    percent: Option<f64>,
    key: &'static str,
) -> Result<Option<AirtimeLimitCentiPercent>, PlanErrorKind> {
    let Some(percent) = percent else {
        return Ok(None);
    };
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return Err(PlanErrorKind::InvalidSetting { key });
    }
    Ok(Some(AirtimeLimitCentiPercent((percent * 100.0) as u16)))
}

fn pipe_respawn_delay(seconds: Option<f64>) -> Result<PipeRespawnDelay, PlanErrorKind> {
    let duration = match seconds {
        Some(seconds) => {
            Duration::try_from_secs_f64(seconds).map_err(|_| PlanErrorKind::InvalidSetting {
                key: interface_key::RESPAWN_DELAY,
            })?
        }
        None => Duration::from_secs(RNS_PIPE_DEFAULT_RESPAWN_SECONDS),
    };
    Ok(PipeRespawnDelay(duration))
}

fn pipe_command(source: &str) -> Result<PipeCommandPlan, PlanErrorKind> {
    let argv = shlex::split(source).filter(|argv| !argv.is_empty()).ok_or(
        PlanErrorKind::InvalidSetting {
            key: interface_key::COMMAND,
        },
    )?;
    Ok(PipeCommandPlan {
        source: source.to_string(),
        argv,
    })
}

fn announce_bandwidth_cap(percent: f64) -> Result<AnnounceBandwidthCap, PlanErrorKind> {
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return Err(PlanErrorKind::InvalidSetting {
            key: interface_key::ANNOUNCE_CAP,
        });
    }
    let per_mille = (percent * 10.0).round();
    Ok(AnnounceBandwidthCap::Limited {
        cap_per_mille: per_mille as u16,
    })
}

fn interface_common_policy(
    interface: &ReferenceInterface,
    global: InterfaceCommonPolicy,
) -> Result<InterfaceCommonPolicy, PlanErrorKind> {
    let mut common = global;
    common.forwarding = InterfaceForwardingPolicy {
        recursive_path_requests: interface
            .recursive_prs
            .unwrap_or(common.forwarding.recursive_path_requests),
        announces_from_internal: interface
            .announces_from_internal
            .unwrap_or(common.forwarding.announces_from_internal),
    };
    common.ingress_control.enabled = interface
        .ingress_control
        .unwrap_or(common.ingress_control.enabled);
    common.path_request_egress.enabled = interface
        .egress_control
        .unwrap_or(common.path_request_egress.enabled);
    if let Some(value) = interface.ic_max_held_announces {
        common.ingress_control.max_held_announces =
            usize::try_from(value).map_err(|_| PlanErrorKind::InvalidSetting {
                key: common_key::IC_MAX_HELD_ANNOUNCES,
            })?;
    }
    apply_common_numbers(
        CommonNumberOverrides::from_interface(interface),
        &mut common,
    )?;
    Ok(common)
}

#[derive(Debug, Clone, Copy, Default)]
struct CommonNumberOverrides {
    new_time: Option<f64>,
    burst_hold: Option<f64>,
    burst_penalty: Option<f64>,
    held_release_interval: Option<f64>,
    burst_freq_new: Option<f64>,
    burst_freq: Option<f64>,
    pr_burst_freq_new: Option<f64>,
    pr_burst_freq: Option<f64>,
    pr_egress_freq: Option<f64>,
}

impl CommonNumberOverrides {
    fn from_interface(interface: &ReferenceInterface) -> Self {
        Self {
            new_time: interface.ic_new_time,
            burst_hold: interface.ic_burst_hold,
            burst_penalty: interface.ic_burst_penalty,
            held_release_interval: interface.ic_held_release_interval,
            burst_freq_new: interface.ic_burst_freq_new,
            burst_freq: interface.ic_burst_freq,
            pr_burst_freq_new: interface.ic_pr_burst_freq_new,
            pr_burst_freq: interface.ic_pr_burst_freq,
            pr_egress_freq: interface.ec_pr_freq,
        }
    }

    fn from_globals(globals: &BTreeMap<String, ReferenceValue>) -> Self {
        Self {
            new_time: global_f64(globals, common_key::IC_NEW_TIME),
            burst_hold: global_f64(globals, common_key::IC_BURST_HOLD),
            burst_penalty: global_f64(globals, common_key::IC_BURST_PENALTY),
            held_release_interval: global_f64(globals, common_key::IC_HELD_RELEASE_INTERVAL),
            burst_freq_new: global_f64(globals, common_key::IC_BURST_FREQ_NEW),
            burst_freq: global_f64(globals, common_key::IC_BURST_FREQ),
            pr_burst_freq_new: global_f64(globals, common_key::IC_PR_BURST_FREQ_NEW),
            pr_burst_freq: global_f64(globals, common_key::IC_PR_BURST_FREQ),
            pr_egress_freq: global_f64(globals, common_key::EC_PR_FREQ),
        }
    }
}

fn apply_common_numbers(
    configured: CommonNumberOverrides,
    common: &mut InterfaceCommonPolicy,
) -> Result<(), PlanErrorKind> {
    if let Some(value) = configured.new_time {
        common.ingress_control.new_interface_ms =
            seconds_to_millis(value, common_key::IC_NEW_TIME)?;
    }
    if let Some(value) = configured.burst_hold {
        common.ingress_control.burst_hold_ms = seconds_to_millis(value, common_key::IC_BURST_HOLD)?;
    }
    if let Some(value) = configured.burst_penalty {
        common.ingress_control.burst_penalty_ms =
            seconds_to_millis(value, common_key::IC_BURST_PENALTY)?;
    }
    if let Some(value) = configured.held_release_interval {
        common.ingress_control.held_release_interval_ms =
            seconds_to_millis(value, common_key::IC_HELD_RELEASE_INTERVAL)?;
    }
    if let Some(value) = configured.burst_freq_new {
        common.ingress_control.announce_burst_frequency_new =
            hertz_to_milli_hertz(value, common_key::IC_BURST_FREQ_NEW)?;
    }
    if let Some(value) = configured.burst_freq {
        common.ingress_control.announce_burst_frequency =
            hertz_to_milli_hertz(value, common_key::IC_BURST_FREQ)?;
    }
    if let Some(value) = configured.pr_burst_freq_new {
        common.ingress_control.path_request_burst_frequency_new =
            hertz_to_milli_hertz(value, common_key::IC_PR_BURST_FREQ_NEW)?;
    }
    if let Some(value) = configured.pr_burst_freq {
        common.ingress_control.path_request_burst_frequency =
            hertz_to_milli_hertz(value, common_key::IC_PR_BURST_FREQ)?;
    }
    if let Some(value) = configured.pr_egress_freq {
        common.path_request_egress.frequency = hertz_to_milli_hertz(value, common_key::EC_PR_FREQ)?;
    }
    Ok(())
}

fn seconds_to_millis(value: f64, key: &'static str) -> Result<u64, PlanErrorKind> {
    let millis = (value * 1_000.0).round();
    if !value.is_finite() || value < 0.0 || millis >= u64::MAX as f64 {
        return Err(PlanErrorKind::InvalidSetting { key });
    }
    Ok(millis as u64)
}

fn hertz_to_milli_hertz(
    value: f64,
    key: &'static str,
) -> Result<FrequencyMilliHertz, PlanErrorKind> {
    let milli_hertz = (value * 1_000.0).round();
    if !value.is_finite() || value < 0.0 || milli_hertz >= u64::MAX as f64 {
        return Err(PlanErrorKind::InvalidSetting { key });
    }
    Ok(FrequencyMilliHertz::new(milli_hertz as u64))
}

fn global_common_policy(config: &ReferenceConfig) -> InterfaceCommonPolicy {
    let mut common = InterfaceCommonPolicy::RNS_DEFAULT;
    common.path_request_egress.enabled =
        global_bool(&config.globals, common_key::EGRESS_CONTROL, false);
    if let Some(value) = global_i64(&config.globals, common_key::IC_MAX_HELD_ANNOUNCES) {
        common.ingress_control.max_held_announces = usize::try_from(value)
            .expect("validated ic_max_held_announces must fit the current platform");
    }
    apply_common_numbers(
        CommonNumberOverrides::from_globals(&config.globals),
        &mut common,
    )
    .expect("validated common interface controls must have representable values");
    common
}

fn global_announce_rate(config: &ReferenceConfig) -> AnnounceRateLimit {
    let seconds = |key, default| {
        global_i64(&config.globals, key)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(default)
    };
    let target_seconds = seconds(global_key::DEFAULT_AR_TARGET, 3_600);
    let penalty_seconds = seconds(global_key::DEFAULT_AR_PENALTY, 0);
    AnnounceRateLimit {
        target_ms: target_seconds
            .checked_mul(1_000)
            .expect("validated default_ar_target must fit milliseconds"),
        grace: seconds(global_key::DEFAULT_AR_GRACE, 5)
            .try_into()
            .expect("validated default_ar_grace must fit u16"),
        penalty_ms: penalty_seconds
            .checked_mul(1_000)
            .expect("validated default_ar_penalty must fit milliseconds"),
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
        ReferenceMode::Internal => InterfaceMode::Internal,
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
    global_number(globals, key)
}

fn global_u64(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<u64> {
    global_number(globals, key)
}

fn global_string(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<String> {
    globals
        .get(key)
        .and_then(ReferenceValue::as_scalar)
        .map(str::to_string)
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks_exact(2)
        .map(|pair| {
            let text = core::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}

fn global_i64(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<i64> {
    global_number(globals, key)
}

fn global_f64(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<f64> {
    global_number(globals, key)
}

fn global_number<T>(globals: &BTreeMap<String, ReferenceValue>, key: &str) -> Option<T>
where
    T: core::str::FromStr,
{
    globals
        .get(key)
        .and_then(ReferenceValue::as_scalar)
        .and_then(|text| crate::reference::cleaned_number(text.trim()))
        .and_then(|text| text.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::parse;

    fn plan_of(config: &str) -> DaemonPlan {
        parse_and_plan(config).expect("config plans").value
    }

    fn named<'a>(plan: &'a DaemonPlan, name: &str) -> &'a PlannedInterface {
        plan.interfaces
            .iter()
            .find(|interface| interface.name == name)
            .unwrap_or_else(|| panic!("interface '{name}' was planned"))
    }

    fn tcp_dial(host: &str, port: u16) -> TcpDialPlan {
        TcpDialPlan {
            host: host.to_string(),
            port,
            connect_timeout: ConnectTimeoutSeconds::new(5),
            reconnect_limit: ReconnectLimit::Unlimited,
            address_family: AddressFamilyPreference::System,
            tunnel: TcpTunnelMode::Direct,
        }
    }

    fn tcp_listener(host: TcpListenHost, port: u16) -> TcpListenPlan {
        TcpListenPlan {
            host,
            port,
            address_family: AddressFamilyPreference::Ipv4,
            tunnel: TcpTunnelMode::Direct,
        }
    }

    fn udp_address(host: &str, port: u16) -> UdpEndpointPlan {
        UdpEndpointPlan {
            host: UdpEndpointHost::Address(host.to_string()),
            port,
        }
    }

    fn serial_line_plan(baud: u32) -> SerialLinePlan {
        SerialLinePlan {
            baud,
            data_bits: SerialDataBits::Eight,
            parity: SerialParity::None,
            stop_bits: SerialStopBits::One,
        }
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
        assert!(plan.transport.routing_enabled());
        assert_eq!(
            plan.transport.identity_policy(),
            TransportIdentityPolicy::Persistent
        );
        assert_eq!(
            plan.shared_instance,
            SharedInstance::Enabled {
                name: "default".to_string(),
                transport: SharedInstanceTransport::Unix,
                instance_port: 37_428,
                control_port: 37_429,
                rpc_key: None,
                forced_bitrate: None,
            }
        );
        assert_eq!(
            named(&plan, "Default Interface").policy.announce_rate_limit,
            Some(AnnounceRateLimit {
                target_ms: 3_600_000,
                grace: 5,
                penalty_ms: 0,
            })
        );
    }

    #[test]
    fn transport_is_off_and_sharing_on_by_default() {
        let plan = plan_of("[interfaces]\n[[A]]\ntype = AutoInterface\nenabled = Yes\n");
        assert!(!plan.transport.routing_enabled());
        assert_eq!(
            plan.transport.identity_policy(),
            TransportIdentityPolicy::Ephemeral
        );
        assert_eq!(plan.discovery, InterfaceDiscoveryPolicy::Disabled);
        assert!(matches!(
            plan.shared_instance,
            SharedInstance::Enabled { .. }
        ));
        assert_eq!(named(&plan, "A").policy.announce_rate_limit, None);
    }

    #[test]
    fn log_levels_cannot_represent_values_outside_the_stock_range() {
        assert_eq!(LogLevel::new(7).map(LogLevel::get), Some(7));
        assert_eq!(LogLevel::new(8), None);
    }

    #[test]
    fn global_protocol_identity_logging_and_shared_instance_settings_are_typed() {
        let plan = plan_of(
            "[reticulum]\n\
             enable_transport = No\n\
             static_transport_identity = Yes\n\
             local_hops_delta = Yes\n\
             link_mtu_discovery = No\n\
             use_implicit_proof = No\n\
             panic_on_interface_error = Yes\n\
             instance_name = field\n\
             shared_instance_type = TCP\n\
             shared_instance_port = 41_000\n\
             instance_control_port = 41_001\n\
             rpc_key = 00112233\n\
             force_shared_instance_bitrate = 250_000_000\n\
             [logging]\n\
             loglevel = 7\n\
             logtimestamps = No\n",
        );
        assert_eq!(
            plan.transport,
            TransportPlan::Leaf(TransportIdentityPolicy::Persistent)
        );
        assert_eq!(
            plan.protocol,
            ProtocolPlan {
                randomize_local_hop_count: true,
                link_mtu_discovery: false,
                use_implicit_proof: false,
            }
        );
        assert_eq!(
            plan.logging,
            LoggingPlan {
                level: LogLevel::new(7).unwrap(),
                timestamps: false,
            }
        );
        assert!(plan.panic_on_interface_error);
        assert_eq!(
            plan.shared_instance,
            SharedInstance::Enabled {
                name: "field".to_string(),
                transport: SharedInstanceTransport::Tcp,
                instance_port: 41_000,
                control_port: 41_001,
                rpc_key: Some(vec![0x00, 0x11, 0x22, 0x33]),
                forced_bitrate: BitrateBps::new(250_000_000),
            }
        );
    }

    #[test]
    fn grouped_global_controls_reach_the_effective_interface_policy() {
        let plan = plan_of(
            "[reticulum]\n\
             enable_transport = Yes\n\
             ic_max_held_announces = 1_024\n\
             ic_burst_freq = 12_500.5\n\
             default_ar_target = 3_600\n\
             [interfaces]\n\
             [[Hub]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = hub\n\
             target_port = 4242\n",
        );
        let policy = named(&plan, "Hub").policy;
        assert_eq!(policy.common.ingress_control.max_held_announces, 1_024);
        assert_eq!(
            policy.common.ingress_control.announce_burst_frequency.get(),
            12_500_500
        );
        assert_eq!(policy.announce_rate_limit.unwrap().target_ms, 3_600_000);
    }

    #[test]
    fn internal_outgoing_and_common_controls_form_one_effective_policy() {
        let plan = plan_of(
            "[reticulum]\n\
             ic_burst_freq = 12.5\n\
             egress_control = Yes\n\
             [interfaces]\n\
             [[Inside]]\n\
             type = TCPClientInterface\n\
             enabled = Yes\n\
             target_host = inside\n\
             target_port = 4242\n\
             mode = internal\n\
             outgoing = No\n\
             recursive_prs = Yes\n\
             announces_from_internal = No\n\
             ingress_control = No\n\
             ec_pr_freq = 0\n\
             ic_max_held_announces = 0\n",
        );
        let policy = named(&plan, "Inside").policy;
        assert_eq!(policy.mode, InterfaceMode::Internal);
        assert_eq!(
            policy.capabilities.ingress,
            prns_core::interfaces::IngressCapability::Enabled
        );
        assert_eq!(policy.capabilities.egress, EgressCapability::Disabled);
        assert!(policy.common.forwarding.recursive_path_requests);
        assert!(!policy.common.forwarding.announces_from_internal);
        assert!(!policy.common.ingress_control.enabled);
        assert_eq!(policy.common.ingress_control.max_held_announces, 0);
        assert_eq!(
            policy.common.ingress_control.announce_burst_frequency.get(),
            12_500
        );
        assert!(policy.common.path_request_egress.enabled);
        assert_eq!(policy.common.path_request_egress.frequency.get(), 0);
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
                    key: interface_key::KISS_FRAMING,
                }
            )
        );
        assert_eq!(
            named(&plan, "Incomplete Server").discovery,
            InterfaceDiscoveryPlan::Unpublishable(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::REACHABLE_ON,
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
                name: "default".to_string(),
                transport: SharedInstanceTransport::Unix,
                instance_port: 40_000,
                control_port: 40_001,
                rpc_key: None,
                forced_bitrate: None,
            }
        );
    }

    #[test]
    fn every_host_constructible_medium_maps() {
        let plan = plan_of(STOCK);
        assert_eq!(plan.interfaces.len(), 5);
        assert_eq!(
            named(&plan, "Default Interface").medium,
            PlannedMedium::AutoWifi { group: None }
        );
        assert_eq!(
            named(&plan, "Hub").medium,
            PlannedMedium::TcpClient {
                connection: tcp_dial("hub.example.com", 4965),
                framing: TcpWireFraming::Hdlc,
            }
        );
        assert_eq!(
            named(&plan, "Listener").medium,
            PlannedMedium::TcpServer {
                listener: tcp_listener(TcpListenHost::Address("0.0.0.0".to_string()), 4242),
                framing: TcpWireFraming::Hdlc,
            }
        );
        assert_eq!(
            named(&plan, "Mesh").medium,
            PlannedMedium::Udp {
                flow: UdpFlowPlan::Bidirectional {
                    listen: udp_address("0.0.0.0", 4848),
                    forward: udp_address("255.255.255.255", 4848),
                },
            }
        );
        assert_eq!(
            named(&plan, "Modem").medium,
            PlannedMedium::Serial {
                device: "/dev/ttyUSB0".to_string(),
                line: serial_line_plan(115_200),
            }
        );
    }

    #[test]
    fn tcp_socket_settings_are_typed_into_the_plan() {
        let plan = plan_of(
            "[interfaces]\n\
             [[Client]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = peer\ntarget_port = 4242\n\
             i2p_tunneled = Yes\nconnect_timeout = 11\nmax_reconnect_tries = 3\n\
             [[Server]]\ntype = TCPServerInterface\nenabled = Yes\nport = 4243\nprefer_ipv6 = Yes\n\
             i2p_tunneled = Yes\nkiss_framing = Yes\n",
        );
        assert_eq!(
            named(&plan, "Client").medium,
            PlannedMedium::TcpClient {
                connection: TcpDialPlan {
                    host: "peer".to_string(),
                    port: 4242,
                    connect_timeout: ConnectTimeoutSeconds::new(11),
                    reconnect_limit: ReconnectLimit::Attempts(3),
                    address_family: AddressFamilyPreference::System,
                    tunnel: TcpTunnelMode::I2p,
                },
                framing: TcpWireFraming::Hdlc,
            }
        );
        assert_eq!(
            named(&plan, "Server").medium,
            PlannedMedium::TcpServer {
                listener: TcpListenPlan {
                    host: TcpListenHost::Any,
                    port: 4243,
                    address_family: AddressFamilyPreference::Ipv6,
                    tunnel: TcpTunnelMode::I2p,
                },
                framing: TcpWireFraming::Kiss,
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
                line: serial_line_plan(115_200),
                preamble_ms: 350,
                txtail_ms: 20,
                persistence: 64,
                slottime_ms: 20,
                flow_control: ReadyCommandFlowControl::Disabled,
                station_id: None,
            }
        );
    }

    #[test]
    fn a_kiss_tnc_carries_configured_timing_flow_control_and_station_id() {
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
                line: serial_line_plan(RNS_DEFAULT_SERIAL_BAUD),
                preamble_ms: 150,
                txtail_ms: 50,
                persistence: 200,
                slottime_ms: 30,
                flow_control: ReadyCommandFlowControl::Enabled,
                station_id: Some(StationIdentificationPlan {
                    callsign: "N0CALL".to_string(),
                    interval_seconds: 600,
                }),
            }
        );
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
                line: serial_line_plan(RNS_DEFAULT_SERIAL_BAUD),
                preamble_ms: 350,
                txtail_ms: 20,
                persistence: 64,
                slottime_ms: 20,
                flow_control: ReadyCommandFlowControl::Disabled,
                callsign: "N0CALL".to_string(),
                ssid: 2,
            }
        );
    }

    #[test]
    fn an_ax25_tnc_without_a_callsign_or_ssid_is_invalid() {
        let no_call = parse(
            "[interfaces]\n[[Packet]]\ntype = AX25KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\nssid = 0\n",
        );
        assert!(no_call.is_err());
        let no_ssid = parse(
            "[interfaces]\n[[Packet]]\ntype = AX25KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\ncallsign = N0CALL\n",
        );
        assert!(no_ssid.is_err());
    }

    #[test]
    fn a_pipe_plans_with_its_command_and_the_default_respawn_delay() {
        let plan = plan_of(
            "[interfaces]\n[[Subprocess]]\ntype = PipeInterface\nenabled = Yes\ncommand = nc -l 4242\n",
        );
        assert_eq!(
            named(&plan, "Subprocess").medium,
            PlannedMedium::Pipe {
                command: PipeCommandPlan {
                    source: "nc -l 4242".to_string(),
                    argv: vec!["nc".to_string(), "-l".to_string(), "4242".to_string()],
                },
                respawn_delay: PipeRespawnDelay(Duration::from_secs(5)),
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
                command: PipeCommandPlan {
                    source: "prog".to_string(),
                    argv: vec!["prog".to_string()],
                },
                respawn_delay: PipeRespawnDelay(Duration::from_millis(2_500)),
            }
        );
    }

    #[test]
    fn a_pipe_without_a_command_is_invalid() {
        assert!(
            parse("[interfaces]\n[[Subprocess]]\ntype = PipeInterface\nenabled = Yes\n").is_err()
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
                listener: tcp_listener(TcpListenHost::Address("0.0.0.0".to_string()), 4242),
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
                listener: tcp_listener(TcpListenHost::Any, 5959),
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
                connection: TcpDialPlan {
                    address_family: AddressFamilyPreference::Ipv4,
                    ..tcp_dial("spine.example.com", 4242)
                },
            }
        );
    }

    #[test]
    fn backbone_remote_alias_selects_the_client_role_on_the_listener_type() {
        let plan = plan_of(
            "[interfaces]\n[[Uplink]]\ntype = BackboneInterface\nenabled = Yes\n\
             remote = spine.example.com\nport = 4242\nprefer_ipv6 = Yes\n",
        );
        assert_eq!(
            named(&plan, "Uplink").medium,
            PlannedMedium::BackboneClient {
                connection: TcpDialPlan {
                    host: "spine.example.com".to_string(),
                    port: 4242,
                    connect_timeout: ConnectTimeoutSeconds::new(5),
                    reconnect_limit: ReconnectLimit::Unlimited,
                    address_family: AddressFamilyPreference::Ipv6,
                    tunnel: TcpTunnelMode::Direct,
                }
            }
        );
    }

    #[test]
    fn a_backbone_listener_without_a_port_is_invalid() {
        let invalid = parse(
            "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\nlisten_ip = 0.0.0.0\n",
        );
        assert!(invalid.is_err());
    }

    #[test]
    fn a_backbone_client_without_a_target_is_invalid() {
        let no_host = parse(
            "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\ntarget_port = 4242\n",
        );
        assert!(no_host.is_err());
        let no_port = parse(
            "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\ntarget_host = spine\n",
        );
        assert!(no_port.is_err());
    }

    #[test]
    fn backbone_host_options_are_fully_planned() {
        let listener = plan_of(
            "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\n\
             listen_port = 4242\ndevice = eth0\nprefer_ipv6 = Yes\n",
        );
        let spine = named(&listener, "Spine");
        assert_eq!(
            spine.medium,
            PlannedMedium::Backbone {
                listener: TcpListenPlan {
                    host: TcpListenHost::Device("eth0".to_string()),
                    port: 4242,
                    address_family: AddressFamilyPreference::Ipv6,
                    tunnel: TcpTunnelMode::Direct,
                }
            }
        );

        let client = plan_of(
            "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\n\
             target_host = spine\ntarget_port = 4242\ni2p_tunneled = Yes\nconnect_timeout = 10\n\
             max_reconnect_tries = 3\n",
        );
        let uplink = named(&client, "Uplink");
        assert_eq!(
            uplink.medium,
            PlannedMedium::BackboneClient {
                connection: TcpDialPlan {
                    host: "spine".to_string(),
                    port: 4242,
                    connect_timeout: ConnectTimeoutSeconds::new(10),
                    reconnect_limit: ReconnectLimit::Attempts(3),
                    address_family: AddressFamilyPreference::Ipv4,
                    tunnel: TcpTunnelMode::I2p,
                }
            }
        );
    }

    #[test]
    fn a_disabled_interface_is_skipped_before_planning() {
        let plan = plan_of(
            "[interfaces]\n[[Off]]\ntype = TCPClientInterface\ntarget_host = h\ntarget_port = 1\n",
        );
        assert!(plan.interfaces.is_empty());
    }

    #[test]
    fn a_missing_required_field_is_invalid_before_planning() {
        let invalid = parse(
            "[interfaces]\n[[Hub]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\n",
        );
        assert!(invalid.is_err());
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
                flow_control: ReadyCommandFlowControl::Disabled,
                station_id: None,
                airtime_limit_short: Some(AirtimeLimitCentiPercent(150)),
                airtime_limit_long: Some(AirtimeLimitCentiPercent(500)),
            }
        );
    }

    #[test]
    fn an_rnode_without_a_radio_field_is_invalid() {
        let no_freq = parse(
            "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             bandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n",
        );
        assert!(no_freq.is_err());
        let no_sf = parse(
            "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\ncodingrate = 5\n",
        );
        assert!(no_sf.is_err());
    }

    #[test]
    fn an_rnode_plans_flow_control_and_station_identification() {
        let plan = plan_of(
            "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
             codingrate = 5\nflow_control = Yes\nid_callsign = N0CALL\nid_interval = 600\n",
        );
        let radio = named(&plan, "Radio");
        let PlannedMedium::Rnode {
            flow_control,
            station_id,
            ..
        } = &radio.medium
        else {
            panic!("RNode medium expected")
        };
        assert_eq!(*flow_control, ReadyCommandFlowControl::Enabled);
        assert_eq!(
            station_id.as_ref(),
            Some(&StationIdentificationPlan {
                callsign: "N0CALL".to_string(),
                interval_seconds: 600,
            })
        );
    }

    #[test]
    fn a_listen_only_udp_disables_egress_and_remains_constructible() {
        let plan = plan_of(
            "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\n\
             listen_ip = 0.0.0.0\nlisten_port = 4848\n",
        );
        let interface = named(&plan, "Mesh");
        assert_eq!(
            interface.medium,
            PlannedMedium::Udp {
                flow: UdpFlowPlan::ReceiveOnly {
                    listen: udp_address("0.0.0.0", 4848),
                }
            }
        );
        assert_eq!(
            interface.policy.capabilities.egress,
            EgressCapability::Disabled
        );
    }

    #[test]
    fn send_only_udp_disables_ingress_and_explicit_outgoing_no_still_wins() {
        let enabled = plan_of(
            "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\n\
             forward_ip = 255.255.255.255\nforward_port = 4848\n",
        );
        let interface = named(&enabled, "Mesh");
        assert_eq!(
            interface.medium,
            PlannedMedium::Udp {
                flow: UdpFlowPlan::SendOnly {
                    forward: udp_address("255.255.255.255", 4848),
                }
            }
        );
        assert_eq!(
            interface.policy.capabilities.ingress,
            IngressCapability::Disabled
        );
        assert!(interface.policy.capabilities.allows_transmit());

        let disabled = plan_of(
            "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\noutgoing = No\n\
             forward_ip = 255.255.255.255\nforward_port = 4848\n",
        );
        assert_eq!(
            named(&disabled, "Mesh").policy.capabilities.egress,
            EgressCapability::Disabled
        );
    }

    #[test]
    fn udp_device_and_port_form_a_bidirectional_broadcast_flow() {
        let plan = plan_of(
            "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\ndevice = eth0\nport = 4848\n",
        );
        let endpoint = UdpEndpointPlan {
            host: UdpEndpointHost::DeviceBroadcast("eth0".to_string()),
            port: 4848,
        };
        assert_eq!(
            named(&plan, "Mesh").medium,
            PlannedMedium::Udp {
                flow: UdpFlowPlan::Bidirectional {
                    listen: endpoint.clone(),
                    forward: endpoint,
                }
            }
        );
    }

    #[test]
    fn udp_device_supplies_the_address_without_changing_a_partial_direction() {
        let receive = plan_of(
            "[interfaces]\n[[Receive]]\ntype = UDPInterface\nenabled = Yes\ndevice = eth0\n\
             listen_port = 4848\n",
        );
        let send = plan_of(
            "[interfaces]\n[[Send]]\ntype = UDPInterface\nenabled = Yes\ndevice = eth0\n\
             forward_port = 4849\n",
        );

        assert_eq!(
            named(&receive, "Receive").medium,
            PlannedMedium::Udp {
                flow: UdpFlowPlan::ReceiveOnly {
                    listen: UdpEndpointPlan {
                        host: UdpEndpointHost::DeviceBroadcast("eth0".to_string()),
                        port: 4848,
                    },
                }
            }
        );
        assert_eq!(
            named(&send, "Send").medium,
            PlannedMedium::Udp {
                flow: UdpFlowPlan::SendOnly {
                    forward: UdpEndpointPlan {
                        host: UdpEndpointHost::DeviceBroadcast("eth0".to_string()),
                        port: 4849,
                    },
                }
            }
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
                line: serial_line_plan(RNS_DEFAULT_SERIAL_BAUD),
            }
        );
        assert_eq!(named(&plan, "Modem").policy.bitrate.get(), 9_600);
    }

    #[test]
    fn serial_line_settings_are_typed_and_drive_the_bitrate() {
        let plan = plan_of(
            "[interfaces]\n[[Modem]]\ntype = SerialInterface\nenabled = Yes\nport = /dev/ttyUSB0\nspeed = 57600\ndatabits = 7\nparity = even\nstopbits = 2\n",
        );
        assert_eq!(
            named(&plan, "Modem").medium,
            PlannedMedium::Serial {
                device: "/dev/ttyUSB0".to_string(),
                line: SerialLinePlan {
                    baud: 57_600,
                    data_bits: SerialDataBits::Seven,
                    parity: SerialParity::Even,
                    stop_bits: SerialStopBits::Two,
                },
            }
        );
        assert_eq!(named(&plan, "Modem").policy.bitrate.get(), 57_600);
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
    fn common_and_medium_settings_are_applied() {
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
        assert_eq!(
            hub.policy.announce_bandwidth_cap,
            AnnounceBandwidthCap::Limited { cap_per_mille: 50 }
        );
        assert_eq!(
            hub.policy.announce_rate_limit,
            Some(AnnounceRateLimit {
                target_ms: 3_600_000,
                grace: 0,
                penalty_ms: 0,
            })
        );
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
    fn an_oversize_ifac_fails_before_a_plan_is_returned() {
        let protected = parse_and_plan(
            "[interfaces]\n[[TooWide]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 1\nnetwork_name = n\nifac_size = 520\n",
        )
        .expect_err("oversize IFAC is invalid");
        assert_eq!(
            protected.diagnostics()[0].code(),
            ConfigDiagnosticCode::InvalidValue
        );
        let open = parse_and_plan(
            "[interfaces]\n[[Open]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 1\nifac_size = 520\n",
        )
        .expect_err("unused invalid IFAC is still invalid config");
        assert_eq!(
            open.diagnostics()[0].code(),
            ConfigDiagnosticCode::InvalidValue
        );
    }

    #[test]
    fn i2p_settings_become_one_effective_typed_plan() {
        let plan = plan_of(
            "[interfaces]\n\
               [[Private I2P]]\n\
                 type = I2PInterface\n\
                 enabled = Yes\n\
                 peers = example.i2p, QUJDRA==\n\
                 connectable = Yes\n\
                 outgoing = No\n\
                 bitrate = 500000\n\
                 network_name = private-overlay\n",
        );
        let interface = named(&plan, "Private I2P");
        let PlannedMedium::I2p {
            peers,
            reachability,
        } = &interface.medium
        else {
            panic!("I2P medium expected")
        };

        assert_eq!(
            peers.iter().map(I2pPeerPlan::as_str).collect::<Vec<_>>(),
            vec!["example.i2p", "QUJDRA=="]
        );
        assert_eq!(*reachability, I2pReachabilityPlan::Connectable);
        assert_eq!(interface.policy.bitrate.get(), 500_000);
        assert_eq!(
            interface.policy.mtu.resolve(interface.policy.bitrate),
            Some(1_064)
        );
        assert_eq!(
            interface.policy.capabilities.egress,
            EgressCapability::Disabled
        );
        assert!(matches!(
            interface.access,
            InterfaceAccessPlan::Ifac {
                size: IfacSize::WIDE,
                ..
            }
        ));
    }

    #[test]
    fn i2p_omissions_are_outbound_only_with_stock_defaults() {
        let plan = plan_of("[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\n");
        let interface = named(&plan, "Private I2P");
        let PlannedMedium::I2p {
            peers,
            reachability,
        } = &interface.medium
        else {
            panic!("I2P medium expected")
        };

        assert!(peers.is_empty());
        assert_eq!(*reachability, I2pReachabilityPlan::OutboundOnly);
        assert_eq!(interface.policy.bitrate.get(), 256_000);
        assert_eq!(
            interface.policy.mtu.resolve(interface.policy.bitrate),
            Some(1_064)
        );
    }

    #[test]
    fn invalid_i2p_peers_have_source_keyed_corrections() {
        let errors = parse_and_plan_named(
            "/etc/reticulum/config",
            "[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\npeers = one.i2p, two.i2p, one.i2p\n",
        )
        .expect_err("duplicate I2P peers are invalid");
        let diagnostic = &errors.diagnostics()[0];

        assert_eq!(diagnostic.code(), ConfigDiagnosticCode::InvalidValue);
        assert_eq!(diagnostic.source(), "/etc/reticulum/config");
        assert_eq!(diagnostic.line(), 5);
        assert_eq!(diagnostic.path(), "[interfaces] > [[Private I2P]] > peers");
        assert!(diagnostic
            .message()
            .contains("I2P peer 3 duplicates peer 1"));
        assert!(diagnostic
            .accepted()
            .is_some_and(|accepted| accepted.contains(".i2p names")));
        assert_eq!(
            diagnostic.correction(),
            "set `peers = example.i2p, QUJDRA==`"
        );
    }

    #[test]
    fn disabled_i2p_stanzas_do_not_validate_unused_peer_settings() {
        let plan = plan_of(
            "[interfaces]\n[[Dormant I2P]]\ntype = I2PInterface\nenabled = No\npeers = not a destination\n",
        );

        assert!(plan.interfaces.is_empty());
    }
}
