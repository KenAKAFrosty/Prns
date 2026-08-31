mod discovery;
mod medium;
mod policy;

pub(super) use super::error::PlanErrorKind;
pub(super) use discovery::plan_interface_discovery;
pub use discovery::{
    DiscoveryAdvertisementPlan, DiscoveryAnnouncementPlan, DiscoveryEncryption,
    DiscoveryIfacPublication, DiscoveryLocationPlan, DiscoveryPublicationProblem,
    InterfaceDiscoveryPlan,
};
pub(super) use medium::{airtime_limit, ready_command_flow_control, station_identification};
pub use medium::{
    AddressFamilyPreference, AirtimeLimitCentiPercent, AutoInterfaceDataPort,
    AutoInterfaceDevicePolicy, AutoInterfaceDiscoveryPort, AutoInterfaceDiscoveryScope,
    AutoInterfaceGroupId, AutoInterfaceMulticastAddressType, AutoInterfacePlan,
    ConnectTimeoutSeconds, I2pPeerPlan, I2pPeersPlan, I2pReachabilityPlan, PipeCommandPlan,
    PipeRespawnDelay, PlannedMedium, ReadyCommandFlowControl, ReconnectLimit, SerialDataBits,
    SerialLinePlan, SerialParity, SerialStopBits, StationIdentificationPlan, TcpDialPlan,
    TcpListenHost, TcpListenPlan, TcpTunnelMode, UdpEndpointHost, UdpEndpointPlan, UdpFlowPlan,
    WebSocketTargetPlan,
};
pub(super) use policy::{
    effective_policy, global_announce_rate, global_common_policy, InheritedInterfacePolicy,
    MemberEgressPolicy,
};

#[cfg(test)]
pub(super) use medium::RNS_DEFAULT_SERIAL_BAUD;

use prns_core::interfaces::IfacSize;
use prns_core::interfaces::{
    AnnounceRateLimit, EffectiveInterfacePolicy, InterfaceCommonPolicy, InterfaceGravity,
};

use self::discovery::plan_interface_discovery as discovery_plan;
use self::medium::plan_medium;
use self::policy::effective_policy as plan_effective_policy;
use crate::reference::keys::interface as interface_key;
use crate::reference::ReferenceInterface;

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedInterface {
    pub name: String,
    pub policy: EffectiveInterfacePolicy,
    pub access: InterfaceAccessPlan,
    pub medium: PlannedMedium,
    pub discovery: InterfaceDiscoveryPlan,
    pub lifecycle: ConfiguredInterfaceLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfiguredInterfaceLifecycle {
    Persistent,
    BootstrapOnly,
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

pub(super) fn plan_interface(
    interface: &ReferenceInterface,
    global_common: InterfaceCommonPolicy,
    global_announce_rate: Option<AnnounceRateLimit>,
    default_gravity: InterfaceGravity,
    transport_enabled: bool,
) -> Result<PlannedInterface, PlanErrorKind> {
    let medium = plan_medium(interface)?;
    let access = plan_access(interface, &medium)?;
    let discovery = discovery_plan(interface, &medium);
    let policy = plan_effective_policy(
        interface,
        &medium,
        &discovery,
        InheritedInterfacePolicy {
            common: global_common,
            announce_rate: global_announce_rate,
            gravity: default_gravity,
        },
        transport_enabled,
        MemberEgressPolicy::Inherit,
    )?;
    Ok(PlannedInterface {
        name: interface.name.clone(),
        policy,
        access,
        medium,
        discovery,
        lifecycle: if interface.bootstrap_only == Some(true) {
            ConfiguredInterfaceLifecycle::BootstrapOnly
        } else {
            ConfiguredInterfaceLifecycle::Persistent
        },
    })
}

pub(super) fn plan_access(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
) -> Result<InterfaceAccessPlan, PlanErrorKind> {
    if interface.network_name.is_none() && interface.passphrase.is_none() {
        return Ok(InterfaceAccessPlan::Open);
    }
    let default_size = match medium {
        PlannedMedium::AutoWifi(_)
        | PlannedMedium::TcpClient { .. }
        | PlannedMedium::TcpServer { .. }
        | PlannedMedium::Udp { .. }
        | PlannedMedium::Backbone { .. }
        | PlannedMedium::BackboneClient { .. }
        | PlannedMedium::I2p { .. }
        | PlannedMedium::Weave { .. }
        | PlannedMedium::PrnsUsbAuto
        | PlannedMedium::PrnsWebSocketClient { .. }
        | PlannedMedium::PrnsWebSocketServer { .. } => IfacSize::WIDE,
        PlannedMedium::Serial { .. }
        | PlannedMedium::Kiss { .. }
        | PlannedMedium::Ax25Kiss { .. }
        | PlannedMedium::Pipe { .. }
        | PlannedMedium::Rnode { .. }
        | PlannedMedium::RnodeMulti { .. }
        | PlannedMedium::PrnsBluetoothAuto { .. } => IfacSize::NARROW,
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
