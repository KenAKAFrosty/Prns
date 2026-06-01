//! The RNS-reference-compatible AutoInterface: a WiFi/IP LAN interface that
//! discovers peers by link-local multicast beacon and exchanges data as unicast
//! to each peer. Wire-exact against Python RNS 1.3.1.
//!
//! - [`protocol`] is the platform-agnostic brain (token, peer table, discovery).
//! - The per-platform worker shells own the sockets and drive the brain

pub mod core;
pub use core::{
    classify_beacon, link_local_from_mac, peering_token, AutoInterfaceProtocol, BeaconVerdict,
    Peer, PeerObservation, PeerTable, DATA_PORT, DISCOVERY_GROUP, DISCOVERY_PORT, GROUP_ID, HW_MTU,
    PEERING_TIMEOUT_MS, UNICAST_DISCOVERY_PORT,
};

#[cfg(feature = "embassy-host")]
pub mod embassy;
