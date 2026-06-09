pub mod core;
pub use core::{
    classify_beacon, descriptor, link_local_from_mac, peering_token, AutoInterfaceProtocol,
    BeaconVerdict, Peer, PeerObservation, PeerTable, PeeringToken, DEFAULT_DATA_PORT,
    DEFAULT_DISCOVERY_PORT, DISCOVERY_GROUP, GROUP_ID, HARDWARE_MTU, PEERING_TIMEOUT_MS,
    UNICAST_DISCOVERY_PORT,
};

#[cfg(feature = "embassy-wifi")]
pub mod embassy;

#[cfg(feature = "wifi-lan-auto")]
pub mod impls;
#[cfg(feature = "wifi-lan-auto")]
pub use impls::wifi_lan_auto_interface;
