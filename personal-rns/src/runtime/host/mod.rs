mod core;
pub use core::{block_on, CycleStamp, Host};

#[cfg(any(
    feature = "embassy-contract",
    feature = "embassy-host",
    feature = "std-sync-host"
))]
pub mod impls;
