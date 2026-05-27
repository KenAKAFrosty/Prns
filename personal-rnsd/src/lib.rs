#![cfg_attr(not(feature = "std"), no_std)]
#![doc = "Reticulum daemon body: a concrete `Host` plus the entry point that drives the core runtime loop."]

#[cfg(feature = "std")]
mod std_host;

#[cfg(feature = "std")]
pub use std_host::{StdHost, StdHostError};

#[cfg(feature = "tokio-host")]
mod tokio_host;

#[cfg(feature = "tokio-host")]
pub use tokio_host::run_multi_thread;
