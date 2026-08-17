#[cfg(all(
    feature = "tokio-host",
    feature = "wifi-auto-apple",
    any(target_os = "macos", target_os = "ios")
))]
pub use prns_interfaces_tokio::wifi_auto::apple_service_discovery;
#[cfg(all(feature = "tokio-host", feature = "wifi-auto-mdns"))]
pub use prns_interfaces_tokio::wifi_auto::native_service_discovery;
#[cfg(feature = "tokio-host")]
pub use prns_interfaces_tokio::wifi_auto::{
    AutoWifi, AutoWifiDevicePolicy, AutoWifiPeer, AutoWifiSettings, AutoWifiSettingsError,
    AutoWifiStatus, DiscoveryLifecycleError, DiscoveryParticipation, ServiceDiscovery,
    ServiceDiscoveryPublisher, SnapshotPublication,
};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::wifi_auto::{
    tcp_rendezvous, AutoWifi, AutoWifiSegment, AutoWifiShared, AutoWifiStatus, AutoWifiTopology,
    TcpRendezvousBuffers, TcpRendezvousClient, TcpRendezvousExitCause, TcpRendezvousServer,
    TcpRendezvousStorage, TcpRendezvousWireSlot, TcpRendezvousWriteFailure,
    UdpServiceDiscoveryConstructionError, UdpServiceDiscoveryPublisher, WifiMemberStatus,
    EMBEDDED_DISCOVERY_PUBLISHER_CAPACITY, TCP_RENDEZVOUS_FRAMED_LEN, TCP_RENDEZVOUS_FRAME_CAP,
    TCP_RENDEZVOUS_LIVENESS_TIMEOUT, TCP_RENDEZVOUS_READ_BUFFER_BYTES,
    TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES, UDP_SERVICE_DISCOVERY_PACKET_BYTES,
    UDP_SERVICE_DISCOVERY_SOCKET_BYTES, UDP_SERVICE_DISCOVERY_SOCKET_COUNT,
    UDP_SERVICE_DISCOVERY_SOCKET_METADATA,
};
