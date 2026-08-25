//! The reference-to-ours mapping layer: an RNS-compatible
//! [`crate::reference::ReferenceConfig`] becomes the complete [`DaemonPlan`] for a node.

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
    UdpFlowPlan, WebSocketTargetPlan,
};
pub use node::{
    parse_and_plan, parse_and_plan_named, plan_reference_config, BlackholeExchangePlan,
    BlackholePublicationPlan, BlackholeSources, BlackholeUpdateInterval, DaemonPlan, LogLevel,
    LoggingPlan, ProbeResponderPlan, ProtocolPlan, RemoteManagementAccessControlList,
    RemoteManagementPlan, SharedInstance, SharedInstanceTransport, TransportIdentityPolicy,
    TransportPlan,
};
pub use rnode::{
    RNodeBleAddress, RNodeBleName, RNodeBleTarget, RNodeSerialDevice, RNodeTcpHost, RNodeTcpTarget,
    RNodeTransportPlan, RNODE_TCP_PORT,
};
pub use rnode_multi::{RNodeMultiDevicePlan, RNodeMultiMemberPlan};

#[cfg(test)]
mod tests;
