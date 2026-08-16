mod policy;
mod protocol;

pub use policy::{
    configured_policy, descriptor, policy_for_bitrate, DEFAULTS, HARDWARE_MTU,
    WIFI_BITRATE_GUESS_BPS, WIFI_EMBEDDED_BITRATE_CEILING_BPS, WIFI_HW_MTU_CAP,
    WIFI_LAN_BITRATE_BPS,
};
#[cfg(feature = "alloc")]
pub use protocol::HeapAutoInterfaceProtocol;
pub use protocol::{
    classify_beacon, classify_beacon_for_group, discovery_group, link_local_from_mac,
    peering_token, peering_token_for_group, AutoInterfaceProtocol, BeaconVerdict, DiscoveryScope,
    FixedAutoInterfaceProtocol, MulticastAddressType, Peer, PeerObservation, PeerStore, PeerTable,
    PeeringToken, DEFAULT_DATA_PORT, DEFAULT_DISCOVERY_PORT, DISCOVERY_GROUP, GROUP_ID, GROUP_NAME,
    MDNS_SERVICE_PORT, MDNS_SERVICE_TYPE, PEERING_TIMEOUT_MS, TCP_RENDEZVOUS_PORT,
    UNICAST_DISCOVERY_PORT,
};
