//! RNS 1.4.0 `Link.outgoing_resources` and `incoming_resources`.
//! - [`OutgoingResources`]: one advertised transfer per link at a time (`Link.ready_for_new_resource`).
//! - [`IncomingResources`]: one transfer per distinct hash on a link (`Link.has_incoming_resource`).
//!
//! Capacity is the store's own property: the engine never assumes a size, it asks, and refuses what doesn't fit, by name.

pub mod core;
mod impls;
pub use impls::*;

pub use self::core::*;
