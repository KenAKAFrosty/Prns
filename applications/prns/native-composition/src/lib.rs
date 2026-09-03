#![deny(rustdoc::broken_intra_doc_links)]

pub mod contract;
mod development_store;
mod directory;
pub mod ffi;
mod lifecycle;
mod lxmf;
mod node;
mod pairing;
mod remote_control;
mod snapshot;

#[cfg(feature = "host-test")]
#[doc(hidden)]
pub mod host_test {
    pub use crate::lifecycle::{
        approve, create_generated_identity, describe, initiate, reject, snapshot, start_configured,
        stop,
    };
}
