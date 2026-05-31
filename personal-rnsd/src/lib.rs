#![cfg_attr(not(feature = "std"), no_std)]
#![doc = "Reticulum daemon body: concrete `EngineDriver` impls plus the entry point that drives the engine."]

mod hosts;

#[cfg(feature = "std")]
pub use hosts::std::{StdEngineDriver, StdEngineDriverError};

#[cfg(feature = "tokio-host")]
pub use hosts::tokio::run_multi_thread;
