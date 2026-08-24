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

mod core;
mod impls;
mod message;

pub use self::core::*;
pub use impls::*;
pub use message::*;

#[cfg(test)]
mod tests;
