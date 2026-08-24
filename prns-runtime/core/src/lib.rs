#![cfg_attr(not(any(feature = "std", test)), no_std)]
#![deny(unsafe_code)]
#![doc = "Runtime-neutral Personal Reticulum node contracts and manifold kernel"]
#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "interface-discovery")]
pub use prns_core::interface_discovery;
#[cfg(feature = "rnx")]
pub use prns_core::rnx;
pub use prns_core::{
    crypto, engine, identity, interfaces, persistence, remote_control, rncp, routing, storage,
    units, wire,
};

pub use runtime::node_introspection;
pub use runtime::{RemoteControlEndpoint, RemoteControlEndpointState, REMOTE_CONTROL_ENDPOINT_ID};
pub mod manifold;
#[cfg(feature = "resource-bzip2")]
pub mod resource_compression;
pub mod runtime;
