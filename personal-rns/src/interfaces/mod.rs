//! The seam between the engine and the wire.
//!
//! The engine defines what every interface IS (the inert data shapes here)
//! and the [`Interface`] contract each autonomous interface fulfills:
//! TCP socket, UDP socket, LoRa radio, BLE GATT, ESP-NOW, sim, and so on.
//! Reticulum policy decisions stay in the engine; an interface impl is a
//! faithful tx/rx actor honoring the contract, never a source of routing
//! or fanout decisions.
//!
//! Layout:
//! - **Data shapes** (`capabilities`, `id`, `medium`, `mode`, `connection_state`):
//!   the inert types every interface presents, kept flat at the root.
//! - **`worker/`**: the [`Interface`] contract — how an autonomous interface starts
//!   and meets the runtime through its seam ([`InterfaceWorkerContext`] /
//!   [`InterfaceHandle`]).
//! - **`framing/`**: byte-stream framing codecs interfaces build on
//!   (`rns_serial_framing`) — building blocks, not interfaces themselves.
//! - **`impls/`**: the concrete implementations — the wire-exact `rns_parity/`
//!   ports of the interfaces stock Python RNS ships, plus the Personal-native
//!   `esp_now/` medium.
//!
//! Everything is re-exported flat here, so consumers reach for
//! `interfaces::InterfaceWorker` / `interfaces::rns_serial_framing` without
//! knowing which bucket owns it.

pub mod capabilities;
pub mod connection_state;
pub mod id;
pub mod mac;
pub mod medium;
pub mod mode;

mod descriptor;
mod framing;
pub mod impls;
mod worker;

pub use capabilities::Capabilities;
pub use connection_state::ConnectionState;
pub use id::InterfaceId;
pub use mac::MacAddress;
pub use medium::MediumKind;
pub use mode::InterfaceMode;

pub use descriptor::InterfaceDescriptor;
pub use worker::{
    ControlCommand, ControlEndpoint, ControlReport, DriverMode, InboundSink, Interface,
    InterfaceHandle, InterfaceWorkerContext, OutboundDrain, QueueFull, SelfDrivenInterface,
    StartedInterface, Substrate,
};

pub use framing::rns_serial_framing;
