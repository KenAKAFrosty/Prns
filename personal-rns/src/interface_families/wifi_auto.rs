#[cfg(feature = "tokio-host")]
pub use prns_interfaces_tokio::wifi_auto::{
    AutoWifi, AutoWifiDevicePolicy, AutoWifiPeer, AutoWifiSettings, AutoWifiSettingsError,
    AutoWifiStatus, DiscoveryParticipation, ServiceDiscovery, ServiceDiscoveryPublisher,
    SnapshotPublication,
};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::wifi_auto::{
    tcp_rendezvous, AutoWifi, AutoWifiSegment, AutoWifiShared, AutoWifiStatus, AutoWifiTopology,
    TcpRendezvousBuffers, TcpRendezvousClient, TcpRendezvousExitCause, TcpRendezvousServer,
    TcpRendezvousStorage, TcpRendezvousWireSlot, TcpRendezvousWriteFailure, WifiMemberStatus,
    TCP_RENDEZVOUS_FRAMED_LEN, TCP_RENDEZVOUS_FRAME_CAP, TCP_RENDEZVOUS_LIVENESS_TIMEOUT,
    TCP_RENDEZVOUS_READ_BUFFER_BYTES, TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES,
};
