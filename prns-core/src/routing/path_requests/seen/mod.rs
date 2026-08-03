//! Path requests already seen, keyed by `(destination, id)`: RNS 1.4.2 `Transport.discovery_pr_tags`. A bounded FIFO set; a duplicate (a loop or a re-arrival) is dropped, so a recursive forward never circulates forever.

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
