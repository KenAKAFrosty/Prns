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
pub mod medium;
pub mod mode;

pub use capabilities::Capabilities;
pub use id::InterfaceId;
pub use medium::MediumKind;
pub use mode::InterfaceMode;
