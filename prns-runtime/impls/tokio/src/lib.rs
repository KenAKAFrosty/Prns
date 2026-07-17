#![forbid(unsafe_code)]
#![deny(rustdoc::broken_intra_doc_links)]

pub use prns_runtime::{
    crypto, engine, identity, interfaces, persistence, routes, routing, storage, units, wire,
};

#[cfg(feature = "interface-discovery")]
pub use prns_runtime::interface_discovery;
pub use runtime::node_introspection;

pub mod reactor;
pub mod runtime;
