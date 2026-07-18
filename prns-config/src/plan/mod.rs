//! The reference-to-ours mapping layer: a faithful [`crate::reference::ReferenceConfig`] becomes a [`DaemonPlan`], the host-agnostic description of the node a daemon should stand up.

mod interface;
mod node;
mod reference_globals;
mod rnode_multi;

pub use interface::{
    AddressFamilyPreference, AirtimeLimitCentiPercent, ConnectTimeoutSeconds,
    DiscoveryAdvertisementPlan, DiscoveryAnnouncementPlan, DiscoveryEncryption,
    DiscoveryIfacPublication, DiscoveryLocationPlan, DiscoveryPublicationProblem, I2pPeerPlan,
    I2pPeersPlan, I2pReachabilityPlan, InterfaceAccessPlan, InterfaceDiscoveryPlan,
    PipeCommandPlan, PipeRespawnDelay, PlannedInterface, PlannedMedium, ReadyCommandFlowControl,
    ReconnectLimit, SerialDataBits, SerialLinePlan, SerialParity, SerialStopBits,
    StationIdentificationPlan, TcpDialPlan, TcpListenHost, TcpListenPlan, TcpTunnelMode,
    UdpEndpointHost, UdpEndpointPlan, UdpFlowPlan,
};
pub use node::{
    parse_and_plan, parse_and_plan_named, DaemonPlan, LogLevel, LoggingPlan, ProtocolPlan,
    SharedInstance, SharedInstanceTransport, TransportIdentityPolicy, TransportPlan,
};
pub use rnode_multi::{RNodeMultiDevicePlan, RNodeMultiMemberPlan};

#[cfg(test)]
mod tests;
