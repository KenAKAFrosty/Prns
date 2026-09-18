#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(feature = "android")]
mod android;
pub mod contract;
mod development_store;
mod directory;
mod input;
mod ios_restoration_probe;
mod lifecycle;
mod lxmf;
mod node;
mod pairing;
mod remote_control;
mod snapshot;

#[cfg(test)]
mod test_support;

#[cfg(feature = "host-test")]
#[doc(hidden)]
pub mod host_test {
    pub use crate::lifecycle::admission::{
        announce_target as announce_self, approve_pairing as approve, describe_target as describe,
        initiate_pairing as initiate, reject_pairing as reject,
    };
    pub use crate::lifecycle::snapshot_async as snapshot;
    pub use crate::lifecycle::{create_generated_identity, start_configured, stop};

    /// Exercise the application operation against a controlled public runtime.
    pub async fn describe_with_handle(
        handle: &personal_rns::prelude::PrnsNodeHandle,
        input: crate::contract::DescribeRemoteControlTargetInput,
    ) -> crate::contract::RemoteControlDescribeOutcome {
        let snapshots = crate::snapshot::SnapshotStore::new();
        snapshots.set_runtime(crate::contract::DevelopmentNodeRuntime::Running);
        let deadline = tokio::time::Instant::now() + crate::lifecycle::COMMAND_TIMEOUT;
        crate::remote_control::describe(handle, &snapshots, input, deadline).await
    }
}

#[cfg(feature = "uniffi-bindings")]
pub mod bindings;

#[cfg(feature = "uniffi-bindings")]
uniffi::setup_scaffolding!();
