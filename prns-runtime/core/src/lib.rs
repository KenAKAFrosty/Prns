#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![forbid(unsafe_code)]
#![doc = "Runtime-neutral Personal Reticulum node contracts and reactor kernel"]
#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "interface-discovery")]
pub use prns_core::interface_discovery;
pub use prns_core::{
    crypto, engine, identity, interfaces, persistence, routing, storage, units, wire,
};

#[cfg(feature = "std")]
pub mod node_introspection;
pub mod reactor;
pub mod runtime;
