#![forbid(unsafe_code)]

//! Generated value transport and the thin UniFFI facade for the shared PRNS host.
uniffi::setup_scaffolding!();

#[path = "transport.generated.rs"]
pub mod transport;

#[path = "remote_control.generated.rs"]
pub mod remote_control;

mod inspection;
pub use inspection::*;

mod facade;
pub use facade::*;

mod platform;
pub use platform::*;

mod remote_operations;
pub use remote_operations::RemoteControlOperations;

#[cfg(feature = "connections")]
mod connections;
#[cfg(feature = "connections")]
#[path = "connections.generated.rs"]
pub mod connections_value;
#[cfg(feature = "connections")]
pub use connections::*;
