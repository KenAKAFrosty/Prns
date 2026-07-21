//! The shared symmetric keys held by GROUP destinations (RNS 1.4.0 `Destination` type `0x01`): one `Token.generate_key()` secret, encrypting and decrypting with it directly.

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
