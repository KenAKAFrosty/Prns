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
mod control;
mod core;
mod endpoint;
mod factory_grant;
mod impls;
mod inventory;
mod message;
mod pagination;
mod pairing;
mod path_table;
mod service;

pub use self::core::*;
pub use bootstrap::*;
pub use control::*;
pub use endpoint::*;
pub use factory_grant::*;
pub use impls::*;
pub use inventory::*;
pub use message::*;
pub use pagination::*;
pub use pairing::*;
pub use path_table::*;
pub use service::*;

#[cfg(test)]
mod tests;
