//! The seam between the engine and the wire.
//!
//! The engine defines what every interface IS (the data shapes here and
//! the base [`Interface`] trait) and what each kind can DO (per-medium
//! sub-traits like [`PointToPointInterface`], [`SharedBroadcastInterface`],
//! and [`SharedMulticastInterface`]). Hosts fulfill the contracts with
//! concrete implementations: TCP socket, UDP socket, LoRa radio, BLE
//! GATT, in-process loopback, sim, and so on. Reticulum policy
//! decisions stay in the engine; an interface impl is a faithful actor
//! honoring the contract, never a source of routing or fanout
//! decisions.
//!
//! Layout:
//! - **Data shapes** (`capabilities`, `id`, `medium`, `mode`, `connection_state`):
//!   the inert types every interface presents, kept flat at the root.
//! - **`contract/`**: the [`Interface`] trait and the per-medium sub-traits
//!   each concrete impl signs.
//! - **`framing/`**: byte-stream framing codecs interfaces build on
//!   (`rns_serial_framing`) — building blocks, not interfaces themselves.
//! - **`impls/`**: the concrete implementations — in-process `loopback/` and the
//!   wire-exact `rns_parity/` ports of the interfaces stock Python RNS ships.
//!
//! Everything is re-exported flat here, so consumers reach for
//! `interfaces::Interface` / `interfaces::LoopbackInterface` /
//! `interfaces::rns_serial_framing` without knowing which bucket owns it.

pub mod capabilities;
pub mod connection_state;
pub mod id;
pub mod mac;
pub mod medium;
pub mod mode;

mod contract;
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

pub use contract::{
    Interface, PointToPointInterface, SharedBroadcastInterface, SharedMulticastInterface,
};

pub use descriptor::InterfaceDescriptor;
pub use worker::{InterfaceStats, InterfaceWorker, QueueFull, RuntimeDriven};

pub use framing::rns_serial_framing;

pub use impls::loopback::{NoAllocLoopback, NoAllocLoopbackError, NoAllocLoopbackQueue};

#[cfg(feature = "alloc")]
pub use impls::loopback::{LoopbackError, LoopbackInterface};

#[cfg(feature = "std")]
pub use impls::loopback::ThreadedLoopback;
