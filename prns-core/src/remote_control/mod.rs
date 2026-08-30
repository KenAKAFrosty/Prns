#![deny(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::float_arithmetic,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unwrap_used,
    clippy::wildcard_enum_match_arm,
    clippy::wildcard_in_or_patterns
)]

mod bootstrap;
mod core;
mod endpoint;
mod impls;
mod message;
mod pairing;
mod service;

pub use self::core::*;
pub use bootstrap::*;
pub use endpoint::*;
pub use impls::*;
pub use message::*;
pub use pairing::*;
pub use service::*;

#[cfg(test)]
mod tests;
