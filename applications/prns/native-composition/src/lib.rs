#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(feature = "android")]
mod android;
pub mod contract;
mod development_store;
mod directory;
pub mod ffi;
mod input;
mod ios_restoration_probe;
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
        announce_self, approve, create_generated_identity, describe, initiate, reject, snapshot,
        start_configured, stop,
    };

    /// Exercise the application operation against a controlled public runtime.
    pub async fn describe_with_handle(
        handle: &personal_rns::prelude::PrnsNodeHandle,
        input: crate::contract::DescribeRemoteControlTargetInput,
    ) -> crate::contract::RemoteControlDescribeOutcome {
        let snapshots = crate::snapshot::SnapshotStore::new();
        snapshots.set_runtime(crate::contract::DevelopmentNodeRuntime::Running);
        crate::remote_control::describe(handle, &snapshots, input).await
    }
}

#[cfg(feature = "uniffi-bindings")]
pub mod bindings;

#[cfg(feature = "uniffi-bindings")]
uniffi::setup_scaffolding!();
