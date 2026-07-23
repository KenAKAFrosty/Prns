#[cfg(feature = "tokio-host")]
pub use prns_interfaces_tokio::wifi_auto::{
    AutoWifi, AutoWifiDevicePolicy, AutoWifiPeer, AutoWifiSettings, AutoWifiSettingsError,
    AutoWifiStatus,
};

#[cfg(all(feature = "embassy-host", not(feature = "tokio-host")))]
pub use prns_interfaces_embassy::wifi_auto::{
    AutoWifi, AutoWifiSegment, AutoWifiShared, AutoWifiStatus, AutoWifiTopology, WifiMemberStatus,
};
