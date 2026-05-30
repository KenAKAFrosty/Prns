//! The seam between the engine and the wire.
//!
//! The engine defines what every interface IS (the data shapes here and
//! the base [`Interface`] trait) and what each kind can DO (per-medium
//! sub-traits like [`PointToPointInterface`] and
//! [`SharedBroadcastInterface`]). Hosts fulfill the contracts with
//! concrete implementations: TCP socket, UDP socket, LoRa radio, BLE
//! GATT, in-process loopback, sim, and so on. Reticulum policy
//! decisions stay in the engine; an interface impl is a faithful actor
//! honoring the contract, never a source of routing or fanout
//! decisions.
//!
//! Layout:
//! - **Data shapes** (`capabilities`, `id`, `medium`, `mode`, `state`):
//!   the inert types every interface presents.
//! - **Traits** (`interface`, `point_to_point`, `shared_broadcast`):
//!   the contract every concrete impl signs.
//! - **Concrete impls** (`loopback/`): in-process loopback variants
//!   live together since they share semantics and vary only by
//!   environment.
//! - **Framing helpers** (`rns_serial_framing`): RNS reference-compatible
//!   byte-stuffed framing used by serial-style transports (USB CDC,
//!   RS-232, etc.); not an `Interface` itself, but a building block for
//!   those that frame over byte streams.

pub mod capabilities;
pub mod id;
pub mod interface;
pub mod loopback;
pub mod medium;
pub mod mode;
pub mod point_to_point;
pub mod rns_serial_framing;
pub mod shared_broadcast;
pub mod state;

pub use capabilities::Capabilities;
pub use id::InterfaceId;
pub use interface::Interface;
pub use medium::MediumKind;
pub use mode::InterfaceMode;
pub use point_to_point::PointToPointInterface;
pub use shared_broadcast::SharedBroadcastInterface;
pub use state::InterfaceState;

pub use loopback::{NoAllocLoopback, NoAllocLoopbackError, NoAllocLoopbackQueue};

#[cfg(feature = "alloc")]
pub use loopback::{LoopbackError, LoopbackInterface};

#[cfg(feature = "std")]
pub use loopback::ThreadedLoopback;
