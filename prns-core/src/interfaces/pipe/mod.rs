//! The pipe interface (RNS `PipeInterface`): Reticulum packets carried as HDLC framing over a
//! spawned subprocess's stdin/stdout. The host-agnostic [`core`] holds the sizing and descriptor;
//! the per-host driver — which serves over whatever byte stream its factory yields — lives in
//! `prns-interfaces-tokio`. The subprocess itself (spawning, the `kill`-on-drop child) is the
//! daemon's concern, supplied through the open factory, exactly as `tokio_serial` is for the serial
//! interface.

pub mod core;
