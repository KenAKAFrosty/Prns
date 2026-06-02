//! The RNS-reference-compatible `SerialInterface`: a point-to-point byte-stream
//! interface (USB-CDC, RS-232, …) that frames Reticulum packets with the
//! canonical HDLC-like octet-stuffing in
//! [`rns_serial_framing`](crate::interfaces::rns_serial_framing), so it is
//! wire-exact against a stock RNS `SerialInterface` with zero adapter code.
//!
//! - [`core`] is the platform-agnostic part: the interface descriptor and the
//!   serial MTU. The framing itself lives in
//!   [`rns_serial_framing`](crate::interfaces::rns_serial_framing) and is shared
//!   verbatim.
//! - The per-platform worker shells own the byte transport and run the
//!   read→deframe→stamp / drain→frame→write loop. Each is generic over the byte
//!   stream (it never names a HAL), so the same shell serves USB-CDC, a UART, or
//!   a test pipe: `embassy` over `embedded_io_async`, a std shell later over
//!   `std::io`.

pub mod core;
pub use core::{descriptor, SERIAL_MTU};

#[cfg(feature = "embassy-host")]
pub mod embassy;

/// The going-forward embassy serial shell on the contract seam (gated on the lighter
/// `embassy-contract` feature, no radio stack). Supersedes [`embassy`], which retires
/// when heltec migrates off the legacy mailbox.
#[cfg(feature = "embassy-contract")]
pub mod embassy_contract;

#[cfg(feature = "std-host")]
pub mod std_host;
