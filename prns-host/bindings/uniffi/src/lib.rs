//! Generated value transport and the thin UniFFI facade for the shared PRNS host.
uniffi::setup_scaffolding!();

#[path = "transport.generated.rs"]
pub mod transport;

#[path = "remote_control.generated.rs"]
pub mod remote_control;

mod facade;
pub use facade::*;
