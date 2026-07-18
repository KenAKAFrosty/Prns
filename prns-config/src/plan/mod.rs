//! The reference-to-ours mapping layer: a faithful [`crate::reference::ReferenceConfig`] becomes a [`DaemonPlan`], the host-agnostic description of the node a daemon should stand up.
//!
//! [`reference`](crate::reference) reads every interface type stock RNS knows about, exactly as RNS reads it. This layer narrows that to what Prns can actually construct today, and is honest about the rest: an interface Prns has no medium for, or one missing a field it needs, becomes a [`DeferredInterface`] carrying *why* rather than being silently dropped; a setting Prns parses but cannot yet route into construction (announce pacing and medium-specific options) is recorded as an [`UnappliedSetting`] on the interface that bears it. [`PlannedMedium`] holds only variants a host can stand up, so an unconstructable interface is unrepresentable as a plan member.
//!
//! [`plan`] is total: it never fails. A config that names nothing constructible yields a plan whose `interfaces` is empty and whose `deferred` explains each omission, leaving the daemon to decide whether an empty node is worth running.

mod interface;
mod node;
mod reference_globals;

pub use interface::{
    AddressFamilyPreference, ConnectTimeoutSeconds, DeferReason, DeferredInterface,
    DiscoveryAdvertisementPlan, DiscoveryAnnouncementPlan, DiscoveryEncryption,
    DiscoveryIfacPublication, DiscoveryLocationPlan, DiscoveryPublicationProblem,
    InterfaceAccessPlan, InterfaceDiscoveryPlan, PlannedInterface, PlannedMedium, ReconnectLimit,
    TcpDialPlan, TcpListenHost, TcpListenPlan, TcpTunnelMode, UdpEndpointHost, UdpEndpointPlan,
    UdpFlowPlan, UnappliedSetting,
};
pub use node::{
    plan, DaemonPlan, LogLevel, LoggingPlan, ProtocolPlan, SharedInstance, SharedInstanceTransport,
    TransportIdentityPolicy, TransportPlan,
};

#[cfg(test)]
mod tests;
