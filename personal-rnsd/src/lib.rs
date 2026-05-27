#![cfg_attr(not(feature = "std"), no_std)]
#![doc = "Reticulum daemon body: a concrete `Host` plus the entry point that drives the core runtime loop."]

#[cfg(feature = "std")]
mod std_host;

#[cfg(feature = "std")]
pub use std_host::{StdHost, StdHostError};
