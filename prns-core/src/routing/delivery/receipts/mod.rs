//! RNS 1.4.2 `Transport.receipts` + `PacketReceipt`. Removing a row concludes exactly one tracked
//! send; the peer's signing key is copied in at send time, so proof validation never depends on
//! the route surviving.

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
