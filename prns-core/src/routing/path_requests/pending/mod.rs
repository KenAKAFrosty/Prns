//! RNS 1.3.5 fires path requests as fire-and-forget and lets the app poll `has_path`.
//! This table is the extension that turns the request into an awaitable outcome.

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
