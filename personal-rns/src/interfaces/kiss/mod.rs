//! The KISS TNC interface (RNS `KISSInterface`): Reticulum packets framed as KISS over a serial
//! TNC. The host-agnostic [`core`] holds the sizing, the startup TNC config, and the descriptor;
//! the per-host driver lives under [`impls`].

pub mod core;
pub mod impls;
