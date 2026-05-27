#![cfg_attr(not(feature = "std"), no_std)]
#![doc = "Reticulum daemon body: a concrete `HostAdapter` plus the entry point that drives the core runtime loop."]

mod hosts;

#[cfg(feature = "std")]
pub use hosts::std::{StdHost, StdHostError};

#[cfg(feature = "tokio-host")]
pub use hosts::tokio::run_multi_thread;
