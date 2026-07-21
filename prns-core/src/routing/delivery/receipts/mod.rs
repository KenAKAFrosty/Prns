//! RNS 1.4.0 `Transport.receipts` + `PacketReceipt`. Removal IS the settlement, so every tracked send settles exactly once; the peer's signing key is copied in at send time, so proof validation never depends on the route surviving.

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
