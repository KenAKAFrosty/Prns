//! Concrete [`Interface`](crate::interfaces::Interface) implementations the core ships.
//!
//! Each is a faithful tx/rx actor honoring the contract, never a source of
//! routing or fanout decisions.
//!
//! - [`rns_parity`]: wire-exact ports of the interfaces stock Python RNS ships
//!   (AutoInterface, SerialInterface, RNode LoRa). Each is a platform-agnostic
//!   `core` plus per-platform workers generic over the HAL; only the
//!   concrete radio/peripheral build that names a specific board lives in a host.
//! - [`esp_now`]: a Personal-native broadcast medium (ESP-NOW) with no stock-RNS
//!   counterpart — Hopspot-to-Hopspot over the 2.4 GHz radio, coalescing several
//!   packets into one v2 frame.

pub mod esp_now;
pub mod rns_parity;
