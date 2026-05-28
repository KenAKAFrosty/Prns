//! Concrete `HostAdapter` implementations for the daemon body.
//!
//! Each platform body the daemon runs on lives in its own module here, so they
//! stay grouped as more transports land rather than scattering across the crate
//! root.

#[cfg(feature = "std")]
pub mod std;

#[cfg(feature = "tokio-host")]
pub mod tokio;
