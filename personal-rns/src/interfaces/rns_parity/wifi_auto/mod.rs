//! The RNS 1.3.1 WiFi/LAN `AutoInterface`: a parent that discovers peers on a link via IPv6
//! link-local multicast, validates each with a peering-token ack, and (once confirmed) spawns a
//! unicast-UDP child interface per peer — so the engine sees a flat list of per-peer interfaces.
//! Being resurrected onto the reactor; for now this is just the platform-agnostic parity [`core`].

pub mod core;
