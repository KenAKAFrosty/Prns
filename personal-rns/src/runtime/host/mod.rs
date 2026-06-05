mod core;
pub use core::{block_on, CycleStamp, Host, NextWake};

#[cfg(any(
    feature = "embassy-contract",
    feature = "embassy-host",
    feature = "std-sync-host",
    feature = "tokio-host"
))]
pub mod impls;
