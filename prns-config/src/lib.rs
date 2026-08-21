#![forbid(unsafe_code)]

pub mod configobj;
pub mod diagnostic;
pub mod discovery;
pub mod editing;
pub mod plan;
pub mod reference;

pub use configobj::{ConfigDocument, ParsedConfigObj, SourceLocations};
pub use diagnostic::{
    ConfigDiagnostic, ConfigDiagnosticCode, ConfigErrors, ConfigFix, ConfigFixSafety, ConfigReport,
    ConfigSeverity, DisplayedConfigDiagnostic, SecretDisplay,
};
pub use discovery::{discover, DiscoveredConfig, DiscoveryError};
pub use plan::{
    parse_and_plan, parse_and_plan_named, plan_reference_config, AddressFamilyPreference,
    AirtimeLimitCentiPercent, AutoInterfaceDataPort, AutoInterfaceDevicePolicy,
    AutoInterfaceDiscoveryPort, AutoInterfaceDiscoveryScope, AutoInterfaceGroupId,
    AutoInterfaceMulticastAddressType, AutoInterfacePlan, BlackholeExchangePlan,
    BlackholePublicationPlan, BlackholeSources, BlackholeUpdateInterval,
    ConfiguredInterfaceLifecycle, ConnectTimeoutSeconds, DaemonPlan, DiscoveryAdvertisementPlan,
    DiscoveryAnnouncementPlan, DiscoveryEncryption, DiscoveryIfacPublication,
    DiscoveryLocationPlan, DiscoveryPublicationProblem, I2pPeerPlan, I2pPeersPlan,
    I2pReachabilityPlan, InterfaceAccessPlan, InterfaceDiscoveryPlan, LogLevel, LoggingPlan,
    PipeCommandPlan, PipeRespawnDelay, PlannedInterface, PlannedMedium, ProtocolPlan,
    RNodeBleAddress, RNodeBleName, RNodeBleTarget, RNodeMultiDevicePlan, RNodeMultiMemberPlan,
    RNodeSerialDevice, RNodeTcpHost, RNodeTcpTarget, RNodeTransportPlan, ReadyCommandFlowControl,
    ReconnectLimit, SerialDataBits, SerialLinePlan, SerialParity, SerialStopBits, SharedInstance,
    SharedInstanceTransport, StationIdentificationPlan, TcpDialPlan, TcpListenHost, TcpListenPlan,
    TcpTunnelMode, TransportIdentityPolicy, TransportPlan, UdpEndpointHost, UdpEndpointPlan,
    UdpFlowPlan, WebSocketTargetPlan, RNODE_TCP_PORT,
};
pub use reference::{
    InterfaceKind, RNodeRadio, RNodeSubinterface, ReferenceBlackholeExchange, ReferenceConfig,
    ReferenceConfigParams, ReferenceDiscoveryConfig, ReferenceInterface,
    ReferenceInterfaceDiscovery, ReferenceMode, ReferencePrnsConfig, ReferenceValue,
};
