mod policy;
mod protocol;

#[cfg(feature = "alloc")]
mod discovery;

#[cfg(feature = "alloc")]
pub use discovery::{
    AdvertisementInsertion, AdvertisementRemoval, CandidateInsertion, CandidateInsertionError,
    DiscoveryEndpoint, DiscoveryEndpointError, DiscoveryServiceName, DiscoveryServiceNameError,
    DiscoverySnapshot, DiscoveryTransport, DiscoveryVersion, DiscoveryVersionError,
    EphemeralDiscoveryInstanceName, ServiceAdvertisement, DEFAULT_DISCOVERY_SERVICE_CAPACITY,
    DISCOVERY_SERVICE_NAME_MAX_BYTES, DNS_SD_LOCAL_DOMAIN,
    EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES, SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY,
    TCP_DNS_SD_BASE_SERVICE_TYPE, TCP_DNS_SD_SERVICE_TYPE, TXT_VERSION_KEY, TXT_VERSION_VALUE,
    UDP_DNS_SD_BASE_SERVICE_TYPE, UDP_DNS_SD_SERVICE_TYPE,
};
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
    PEERING_TIMEOUT_MS, TCP_RENDEZVOUS_PORT, UNICAST_DISCOVERY_PORT,
};
