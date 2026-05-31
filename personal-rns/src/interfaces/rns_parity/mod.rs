//! RNS reference-compatibility interfaces: faithful, wire-exact ports of the
//! interfaces Python Reticulum ships, so a Personal node interoperates with
//! stock RNS on the same media. Each is a named interface fulfilled per host by
//! a platform worker shell.
//!
//! Today: [`auto_interface`] (the WiFi/IP multicast LAN interface), [`serial`]
//! (the RNS `SerialInterface` over any byte stream — USB-CDC, RS-232), and
//! [`rnode_lora`] (RNode-compatible LoRa — we are the modem). Future parity ports —
//! TCP, UDP — land here as siblings.

pub mod auto_interface;
pub mod rnode_lora;
pub mod serial;
