mod core;
pub use core::{block_on, Host, NextWake};

#[cfg(any(
    feature = "embassy-contract",
    feature = "std-sync-host",
    feature = "tokio-host"
))]
pub mod impls;
