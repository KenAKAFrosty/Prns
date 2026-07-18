//! The reference-to-ours mapping layer: a faithful [`crate::reference::ReferenceConfig`] becomes a [`DaemonPlan`], the host-agnostic description of the node a daemon should stand up.

mod error;
mod interface;
mod node;
mod reference_globals;
mod rnode;
mod rnode_multi;

pub use interface::{
    AddressFamilyPreference, AirtimeLimitCentiPercent, AutoInterfaceDataPort,
    AutoInterfaceDevicePolicy, AutoInterfaceDiscoveryPort, AutoInterfaceDiscoveryScope,
    AutoInterfaceGroupId, AutoInterfaceMulticastAddressType, AutoInterfacePlan,
    ConfiguredInterfaceLifecycle, ConnectTimeoutSeconds, DiscoveryAdvertisementPlan,
    DiscoveryAnnouncementPlan, DiscoveryEncryption, DiscoveryIfacPublication,
    DiscoveryLocationPlan, DiscoveryPublicationProblem, I2pPeerPlan, I2pPeersPlan,
    I2pReachabilityPlan, InterfaceAccessPlan, InterfaceDiscoveryPlan, PipeCommandPlan,
    PipeRespawnDelay, PlannedInterface, PlannedMedium, ReadyCommandFlowControl, ReconnectLimit,
    SerialDataBits, SerialLinePlan, SerialParity, SerialStopBits, StationIdentificationPlan,
    TcpDialPlan, TcpListenHost, TcpListenPlan, TcpTunnelMode, UdpEndpointHost, UdpEndpointPlan,
    UdpFlowPlan,
};
pub use node::{
    parse_and_plan, parse_and_plan_named, BlackholeExchangePlan, BlackholePublicationPlan,
    BlackholeSources, BlackholeUpdateInterval, DaemonPlan, LogLevel, LoggingPlan,
    ProbeResponderPlan, ProtocolPlan, RemoteManagementAccessControlList, RemoteManagementPlan,
    SharedInstance, SharedInstanceTransport, TransportIdentityPolicy, TransportPlan,
};
pub use rnode::{
    RNodeSerialDevice, RNodeTcpHost, RNodeTcpTarget, RNodeTransportPlan, RNODE_TCP_PORT,
};
pub use rnode_multi::{RNodeMultiDevicePlan, RNodeMultiMemberPlan};

#[cfg(test)]
mod tests;
