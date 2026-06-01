//! Concrete interface implementations the core ships — both inline
//! [`Interface`](crate::interfaces::Interface)s and autonomous
//! [`InterfaceWorker`](crate::interfaces::InterfaceWorker)s.
//!
//! Each is a faithful tx/rx actor honoring the contract, never a source of
//! routing or fanout decisions.
//!
//! - [`loopback`]: the in-process family (alloc / threaded / no_alloc variants).
//! - [`rns_parity`]: wire-exact ports of the interfaces stock Python RNS ships
//!   (AutoInterface, SerialInterface, RNode LoRa). Each is a platform-agnostic
//!   `core` plus per-platform worker shells generic over the HAL; only the
//!   concrete radio/peripheral build that names a specific board lives in a host.

pub mod loopback;
pub mod rns_parity;
