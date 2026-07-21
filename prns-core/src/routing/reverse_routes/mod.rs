//! The return path for proofs riding back over packets we forwarded (RNS 1.4.0 `Transport.reverse_table`).

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
