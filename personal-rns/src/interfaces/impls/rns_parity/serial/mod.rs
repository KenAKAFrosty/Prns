//! The RNS-reference-compatible `SerialInterface`: a point-to-point byte-stream
//! interface (USB-CDC, RS-232, …) that frames Reticulum packets with the
//! canonical HDLC-like octet-stuffing in
//! [`rns_serial_framing`](crate::interfaces::rns_serial_framing), so it is
//! wire-exact against a stock RNS `SerialInterface`.
//!
//! - [`core`] is the platform-agnostic part: the interface descriptor and the
//!   serial MTU. The framing itself lives in
//!   [`rns_serial_framing`](crate::interfaces::rns_serial_framing) and is shared
//!   verbatim.
//! - The per-platform workers own the byte transport and run the
//!   read→deframe→stamp / drain→frame→write loop. Each is generic over the byte
//!   stream, so the implementation can serve USB-CDC, UART, or test pipes
//!   without naming a specific HAL.

pub mod core;
pub use core::{descriptor, SERIAL_MTU};

/// The embassy serial worker on the contract seam (gated on the lighter
/// `embassy-contract` feature — no radio stack).
#[cfg(feature = "embassy-contract")]
pub mod embassy_contract;

#[cfg(feature = "std-host")]
pub mod std_host;
