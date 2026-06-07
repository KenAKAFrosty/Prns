//! RNS reference-compatibility interfaces: faithful, wire-exact ports of the
//! interfaces Python Reticulum ships, so a Personal node interoperates with
//! stock RNS on the same media. Each is a named interface fulfilled per host by
//! a platform worker.
//!
//! Today: [`auto_interface`] (the WiFi/IP multicast LAN interface), [`serial`]
//! (the RNS `SerialInterface` over any byte stream — USB-CDC, RS-232), [`pipe`]
//! (the RNS `PipeInterface` over a subprocess's stdio), and [`rnode_lora`]
//! (RNode-compatible LoRa).

pub mod auto_interface;
pub mod pipe;
pub mod rnode_lora;
pub mod serial;
