//! Bounded identity publication and lifecycle coordination.
//!
//! This crate owns no keys, runtime, network, clock or filesystem. Adapters execute
//! explicit, identity-scoped effects and acknowledge verified durable outcomes.
//! Storage errors fence the coordinator until an authoritative reload. Caller-
//! supplied working and staging catalogs keep snapshot placement explicit.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "uniffi-bindings")]
extern crate std;
#[cfg(feature = "uniffi-bindings")]
uniffi::setup_scaffolding!();
mod api_error;
#[cfg(feature = "uniffi-bindings")]
mod bindings;
pub use api_error::*;

mod coordinator;
mod model;

pub use coordinator::{validate_catalog, validate_create_request, Coordinator, Storage};
pub use model::*;
