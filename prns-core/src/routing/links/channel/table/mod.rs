//! RNS 1.4.0 `Channel`'s ring state as a backend-swappable table: reorder buffer, outstanding sends, and window column per link.

pub mod core;
pub mod impls;

pub use self::core::*;
