//! Host contract for driving the runtime loop.
//!
//! `core` defines the substrate-neutral API. [`impls`] contains the
//! feature-gated platform adapters.

mod core;
pub use core::{block_on, CycleStamp, Host};

#[cfg(any(
    feature = "embassy-contract",
    feature = "embassy-host",
    feature = "std-host"
))]
pub mod impls;
