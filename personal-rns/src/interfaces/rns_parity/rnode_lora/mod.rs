//! The RNS LoRa interface: carries Reticulum packets over a LoRa radio, on-air
//! compatible with RNode firmware — the Personal Hopspot *is* the modem, not a
//! host driving one over KISS-serial.
//!
//! [`core`] is the platform-agnostic part — routing descriptor, the modulation
//! profile both ends must agree on, and RNode's one-byte on-air link-header
//! codec. The embassy shell (the SX1262 driver over `lora-phy`) lands alongside
//! it for hosts that have the radio.

pub mod core;

#[cfg(feature = "embassy-host")]
pub mod embassy;
