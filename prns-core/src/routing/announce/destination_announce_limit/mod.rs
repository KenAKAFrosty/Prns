//! Per-destination announce rebroadcast rate limiting: RNS 1.3.5 `Transport.announce_rate_table`.
//! Violations past the grace count block a destination's rebroadcast for a penalty window (the path is still learned, only the propagation stops).
//!
//! NOTE The reference's `timestamps` ring is omitted. It feeds only the `rnpath` rate display, never the blocking decision.

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
