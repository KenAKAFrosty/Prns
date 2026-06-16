//! The RNS 1.3.1 WiFi/LAN `AutoInterface`: an `InterfaceSupervisor` that discovers peers on a link
//! via IPv6 link-local multicast, validates each with a peering-token ack, and (once confirmed)
//! stands up a unicast-UDP member per peer through its `Fleet` handle, so the engine sees a flat
//! list of per-peer interfaces. The sizing brain and the parity protocol are the platform-agnostic
//! [`core`]; the tokio bodies (the per-peer leaf, and the supervisor that owns the shared sockets)
//! live under [`impls`].

pub mod core;
pub mod impls;

#[cfg(feature = "wifi-lan-auto")]
pub use impls::tokio::{AutoWifi, AutoWifiPeer, AutoWifiStatus};

#[cfg(feature = "embassy-wifi")]
pub use impls::embassy::{AutoWifi, AutoWifiShared, AutoWifiStatus, WifiMemberStatus};
