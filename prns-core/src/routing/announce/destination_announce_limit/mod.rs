//! Per-destination announce rebroadcast rate limiting: RNS 1.4.0 `Transport.announce_rate_table`.
//! Violations past the grace count block a destination's rebroadcast for a penalty window (the path is still learned, only the propagation stops).
//!
pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
