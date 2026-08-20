use std::path::PathBuf;
use std::time::Duration;

use prns_core::identity::IdentityHash;
use prns_core::interface_discovery::{
    AutoConnectPolicy, AutoConnectRoutingPolicy, DiscoverySourcePolicy, InterfaceDiscoveryPolicy,
    DEFAULT_STAMP_COST,
};
use prns_core::interfaces::{BitrateBps, InterfaceGravity};
use prns_core::routing::links::resources::ResourceMemoryLimits;

use super::error::{GlobalPlanError, PlanError, PlanningError};
use super::interface::{
    global_announce_rate, global_common_policy, plan_interface, PlanErrorKind, PlannedInterface,
};
use super::reference_globals::{
    decode_hex, global_bool, global_i64, global_string, global_u16, global_u64,
};
use super::rnode_multi;
use crate::reference::keys::{
    global as global_key, interface as interface_key, logging as logging_key,
    section as section_key,
};
use crate::reference::{ReferenceConfig, ReferenceConfigParams, ReferenceRemoteManagement};
use crate::{ConfigDiagnostic, ConfigDiagnosticCode, ConfigErrors, ConfigReport, SourceLocations};

/// The complete description of a node to stand up, projected from an RNS-compatible
/// configuration including supported Prns extensions.
#[derive(Debug, Clone, PartialEq)]
pub struct DaemonPlan {
    pub transport: TransportPlan,
    pub shared_instance: SharedInstance,
    pub remote_management: RemoteManagementPlan,
    pub probe_responder: ProbeResponderPlan,
    pub blackhole_exchange: BlackholeExchangePlan,
    pub protocol: ProtocolPlan,
    pub logging: LoggingPlan,
    pub resource_memory_limits: ResourceMemoryLimits,
    pub panic_on_interface_error: bool,
    pub network_identity_path: Option<PathBuf>,
    pub discovery: InterfaceDiscoveryPolicy,
    pub interfaces: Vec<PlannedInterface>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackholeExchangePlan {
    publication: BlackholePublicationPlan,
    sources: BlackholeSources,
    update_interval: BlackholeUpdateInterval,
}

impl BlackholeExchangePlan {
    pub const fn publication(&self) -> BlackholePublicationPlan {
        self.publication
    }

    pub fn sources(&self) -> &[IdentityHash] {
        self.sources.as_slice()
    }

    pub const fn update_interval(&self) -> BlackholeUpdateInterval {
        self.update_interval
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackholePublicationPlan {
    Disabled,
    Enabled,
}

impl BlackholePublicationPlan {
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackholeSources(Vec<IdentityHash>);

impl BlackholeSources {
    fn from_identities(identities: &[IdentityHash]) -> Self {
        let mut sources = Vec::new();
        for identity in identities {
            if !sources.contains(identity) {
                sources.push(*identity);
            }
        }
        Self(sources)
    }

    pub fn as_slice(&self) -> &[IdentityHash] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlackholeUpdateInterval(Duration);

impl BlackholeUpdateInterval {
    pub const DEFAULT: Self = Self(Duration::from_secs(60 * 60));
    pub const MINIMUM: Self = Self(Duration::from_secs(2 * 60));

    fn from_configured_minutes(minutes: f64) -> Option<Self> {
        if !minutes.is_finite() {
            return None;
        }
        Duration::try_from_secs_f64(minutes.max(2.0) * 60.0)
            .ok()
            .map(Self)
    }

    pub const fn duration(self) -> Duration {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResponderPlan {
    Disabled,
    Enabled,
}

impl ProbeResponderPlan {
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteManagementPlan {
    Disabled,
    Enabled(RemoteManagementAccessControlList),
}

impl RemoteManagementPlan {
    pub fn allowed(&self) -> Option<&[IdentityHash]> {
        match self {
            Self::Disabled => None,
            Self::Enabled(acl) => Some(acl.as_slice()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteManagementAccessControlList(Vec<IdentityHash>);

impl RemoteManagementAccessControlList {
    pub fn from_identities(identities: Vec<IdentityHash>) -> Self {
        Self(identities)
    }

    pub fn as_slice(&self) -> &[IdentityHash] {
        &self.0
    }
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

pub fn plan_reference_config(config: &ReferenceConfig) -> Result<DaemonPlan, ConfigErrors> {
    build_plan(config).map_err(|errors| {
        let locations = SourceLocations::default();
        ConfigErrors::new(
            errors
                .iter()
                .map(|error| planning_diagnostic("typed config", &locations, error))
                .collect(),
        )
    })
}

pub(super) fn build_plan(config: &ReferenceConfig) -> Result<DaemonPlan, Vec<PlanningError>> {
    let mut interfaces = Vec::new();
    let mut errors = Vec::new();
    let transport = transport_plan(config);
    let blackhole_exchange =
        blackhole_exchange(config).map_err(|error| vec![PlanningError::Global(error)])?;
    let common =
        global_common_policy(config).map_err(|error| vec![PlanningError::Global(error)])?;
    let announce_rate =
        global_announce_rate(config).map_err(|error| vec![PlanningError::Global(error)])?;
    let default_gravity = InterfaceGravity::new(
        global_i64(&config.globals, global_key::DEFAULT_GRAVITY).unwrap_or(0),
    );
    for interface in &config.interfaces {
        if matches!(interface.params, ReferenceConfigParams::RnodeMulti { .. }) {
            match rnode_multi::plan(
                interface,
                common,
                announce_rate,
                default_gravity,
                transport.routing_enabled(),
            ) {
                Ok(planned) => interfaces.extend(planned),
                Err(failure) => errors.push(PlanningError::Interface(PlanError {
                    interface_name: interface.name.clone(),
                    interface_type: interface.type_name.clone(),
                    subinterface_name: failure.subinterface_name,
                    kind: failure.kind,
                })),
            }
            continue;
        }
        match plan_interface(
            interface,
            common,
            announce_rate,
            default_gravity,
            transport.routing_enabled(),
        ) {
            Ok(planned) => interfaces.push(planned),
            Err(kind) => errors.push(PlanningError::Interface(PlanError {
                interface_name: interface.name.clone(),
                interface_type: interface.type_name.clone(),
                subinterface_name: None,
                kind,
            })),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(DaemonPlan {
        transport,
        shared_instance: shared_instance(config),
        remote_management: remote_management(config),
        probe_responder: if global_bool(&config.globals, global_key::RESPOND_TO_PROBES, false) {
            ProbeResponderPlan::Enabled
        } else {
            ProbeResponderPlan::Disabled
        },
        blackhole_exchange,
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
        resource_memory_limits: ResourceMemoryLimits {
            incoming_bytes: config
                .prns
                .resource_mem_in
                .unwrap_or(ResourceMemoryLimits::DEFAULT_HOST.incoming_bytes),
            outgoing_bytes: config
                .prns
                .resource_mem_out
                .unwrap_or(ResourceMemoryLimits::DEFAULT_HOST.outgoing_bytes),
        },
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

fn blackhole_exchange(config: &ReferenceConfig) -> Result<BlackholeExchangePlan, GlobalPlanError> {
    let update_interval = match config.blackhole_exchange.update_interval_minutes {
        Some(minutes) => {
            BlackholeUpdateInterval::from_configured_minutes(minutes).ok_or(GlobalPlanError {
                key: global_key::BLACKHOLE_UPDATE_INTERVAL,
            })?
        }
        None => BlackholeUpdateInterval::DEFAULT,
    };
    Ok(BlackholeExchangePlan {
        publication: if config.blackhole_exchange.publish == Some(true) {
            BlackholePublicationPlan::Enabled
        } else {
            BlackholePublicationPlan::Disabled
        },
        sources: BlackholeSources::from_identities(&config.blackhole_exchange.sources),
        update_interval,
    })
}

fn remote_management(config: &ReferenceConfig) -> RemoteManagementPlan {
    match &config.remote_management {
        ReferenceRemoteManagement::Disabled => RemoteManagementPlan::Disabled,
        ReferenceRemoteManagement::Enabled { allowed } => RemoteManagementPlan::Enabled(
            RemoteManagementAccessControlList::from_identities(allowed.clone()),
        ),
    }
}

pub(super) fn planning_diagnostic(
    source: &str,
    locations: &SourceLocations,
    error: &PlanningError,
) -> ConfigDiagnostic {
    match error {
        PlanningError::Global(error) => global_planning_diagnostic(source, locations, *error),
        PlanningError::Interface(error) => interface_planning_diagnostic(source, locations, error),
    }
}

fn global_planning_diagnostic(
    source: &str,
    locations: &SourceLocations,
    error: GlobalPlanError,
) -> ConfigDiagnostic {
    ConfigDiagnostic::new(
        ConfigDiagnosticCode::InvalidValue,
        source,
        locations
            .line([section_key::RETICULUM, error.key])
            .unwrap_or(1),
        format!("[reticulum] > {}", error.key),
        None,
        format!(
            "global setting {:?} cannot be represented by this build",
            error.key
        ),
        Some("a non-negative value within the documented range".to_string()),
        format!(
            "replace `{}` under [reticulum] with a smaller value",
            error.key
        ),
    )
}

fn interface_planning_diagnostic(
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
        AutoConnectRoutingPolicy {
            gravity: InterfaceGravity::new(config.discovery.auto_connect_gravity.unwrap_or(0)),
            announces_to_internal: config
                .discovery
                .auto_connect_announces_to_internal
                .unwrap_or(false),
        },
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
