//! Explicit selected-domain service construction and attachment.
//!
//! This crate returns ordinary domain clients and supplies their ownership and
//! identity gate. It does not introduce application commands or selected UI state.
#![forbid(unsafe_code)]

#[cfg(feature = "identities")]
pub mod local;
#[cfg(feature = "identities")]
pub mod stores;

#[cfg(feature = "groups")]
pub mod groups;

#[cfg(feature = "uniffi-bindings")]
uniffi::setup_scaffolding!();
