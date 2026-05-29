//! The seam between the engine and the wire.
//!
//! The engine defines what every interface IS (this module's data shapes,
//! soon a trait) and what each kind can DO (sub-traits arriving in later
//! slices). Hosts fulfill the contracts with concrete implementations:
//! TCP socket, UDP socket, LoRa radio, BLE GATT, in-process loopback,
//! sim, and so on. Reticulum policy decisions stay in the engine; an
//! interface impl is a faithful actor honoring the contract, never a
//! source of routing or fanout decisions.

pub mod capabilities;
pub mod id;
pub mod interface;
pub mod medium;
pub mod mode;
pub mod point_to_point;
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

#[cfg(feature = "alloc")]
pub mod loopback;
#[cfg(feature = "alloc")]
pub use loopback::{LoopbackError, LoopbackInterface};
