mod discovery;
mod medium;
mod policy;

pub use discovery::{
    DiscoveryAdvertisementPlan, DiscoveryAnnouncementPlan, DiscoveryEncryption,
    DiscoveryIfacPublication, DiscoveryLocationPlan, DiscoveryPublicationProblem,
    InterfaceDiscoveryPlan,
};
pub use medium::{
    AddressFamilyPreference, ConnectTimeoutSeconds, PlannedMedium, ReconnectLimit, TcpDialPlan,
    TcpListenHost, TcpListenPlan, TcpTunnelMode, UdpEndpointHost, UdpEndpointPlan, UdpFlowPlan,
};
pub(super) use policy::{global_announce_rate, global_common_policy};

#[cfg(test)]
pub(super) use medium::RNS_DEFAULT_SERIAL_BAUD;

use prns_core::interfaces::ifac::IfacSize;
use prns_core::interfaces::{AnnounceRateLimit, EffectiveInterfacePolicy, InterfaceCommonPolicy};

use self::discovery::plan_interface_discovery;
use self::medium::plan_medium;
use self::policy::effective_policy;
use crate::reference::keys::interface as interface_key;
use crate::reference::ReferenceInterface;

/// One interface a host can construct, with one effective policy and a record of settings the current host backend does not yet honor.
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
pub enum InterfaceAccessPlan {
    Open,
    Ifac {
        network_name: Option<String>,
        passphrase: Option<String>,
        size: IfacSize,
    },
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
    Disabled,
    UnsupportedKind,
    MissingRequiredField { key: &'static str },
    InvalidSetting { key: &'static str },
}

/// A setting parsed from config that v1 construction does not yet route into the interface it belongs to. Surfaced so the daemon can report it rather than silently ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnappliedSetting {
    /// A medium-specific key parsed but not passed to the constructor (e.g. `kiss_framing`).
    MediumOption(&'static str),
}

pub(super) fn plan_interface(
    interface: &ReferenceInterface,
    global_common: InterfaceCommonPolicy,
    global_announce_rate: AnnounceRateLimit,
    transport_enabled: bool,
) -> Result<PlannedInterface, DeferReason> {
    if !interface.enabled.unwrap_or(false) {
        return Err(DeferReason::Disabled);
    }
    let mut unapplied = Vec::new();
    let medium = plan_medium(interface, &mut unapplied)?;
    let access = plan_access(interface, &medium)?;
    let discovery = plan_interface_discovery(interface, &medium);
    let policy = effective_policy(
        interface,
        &medium,
        &discovery,
        global_common,
        global_announce_rate,
        transport_enabled,
    )?;
    Ok(PlannedInterface {
        name: interface.name.clone(),
        policy,
        access,
        medium,
        discovery,
        unapplied,
    })
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
        Some(bits) if bits >= 8 => {
            IfacSize::new((bits / 8) as usize).map_err(|_| DeferReason::InvalidSetting {
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
