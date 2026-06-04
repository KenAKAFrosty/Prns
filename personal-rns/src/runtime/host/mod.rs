mod core;
pub use core::{block_on, CycleStamp, Host};

#[cfg(any(
    feature = "embassy-contract",
    feature = "embassy-host",
    feature = "std-host"
))]
pub mod impls;
