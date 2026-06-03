//! The RNS-reference-compatible `SerialInterface`: a point-to-point byte-stream
//! interface (RS-232, a UART, …) that frames Reticulum packets with the
//! canonical HDLC-like octet-stuffing in
//! [`rns_serial_framing`](crate::interfaces::rns_serial_framing), so it is
//! wire-exact against a stock RNS `SerialInterface`.
//!
//! - [`core`] is the platform-agnostic part: the interface descriptor and the
//!   serial MTU. The framing itself lives in
//!   [`rns_serial_framing`](crate::interfaces::rns_serial_framing) and is shared
//!   verbatim.
//! - `impls/` holds the per-substrate workers, one file each (`std`, `embassy`).
//!   Each owns its platform's byte transport and runs the same
//!   read→deframe→stamp / drain→frame→write loop, generic over the byte stream so
//!   it can serve a UART, an RS-232 link, or a test pipe without naming a specific HAL.

pub mod core;
pub use core::{descriptor, SERIAL_MTU};

mod impls;
pub use impls::*;
