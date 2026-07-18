use std::path::PathBuf;

use prns_core::interface_discovery::{
    AutoConnectPolicy, DiscoverySourcePolicy, InterfaceDiscoveryPolicy, DEFAULT_STAMP_COST,
};
use prns_core::interfaces::BitrateBps;

use super::interface::{
    global_announce_rate, global_common_policy, plan_interface, DeferredInterface, PlannedInterface,
};
use super::reference_globals::{global_bool, global_string, global_u16, global_u64};
use crate::reference::keys::{
    global as global_key, logging as logging_key, section as section_key,
};
use crate::reference::ReferenceConfig;

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
    /// The interfaces a host can construct from this config, in config order.
    pub interfaces: Vec<PlannedInterface>,
    /// The interfaces this config named that the node will not stand up, each with its reason.
    pub deferred: Vec<DeferredInterface>,
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

/// Project a faithful reference config onto the node a host should stand up.
#[must_use]
pub fn plan(config: &ReferenceConfig) -> DaemonPlan {
    let mut interfaces = Vec::new();
    let mut deferred = Vec::new();
    let transport = transport_plan(config);
    let common = global_common_policy(config);
    let announce_rate = global_announce_rate(config);
    for interface in &config.interfaces {
        match plan_interface(
            interface,
            common,
            announce_rate,
            transport.routing_enabled(),
        ) {
            Ok(planned) => interfaces.push(planned),
            Err(reason) => deferred.push(DeferredInterface {
                name: interface.name.clone(),
                type_name: interface.type_name.clone(),
                why: reason,
            }),
        }
    }
    DaemonPlan {
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
