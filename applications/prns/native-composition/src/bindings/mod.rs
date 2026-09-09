//! Generated foreign-language views over the application-owned native composition.
//!
//! This module creates neither a node nor a runtime. Platform lifecycle owners and
//! generated JavaScript callers must link/load the same `prns_app` library image.

mod host_generated;

use crate::contract::{DevelopmentNodeSnapshot, U64String};

type SnapshotBox = Box<DevelopmentNodeSnapshot>;
uniffi::custom_type!(SnapshotBox, DevelopmentNodeSnapshot, {
    lower: |value| *value,
    try_lift: |value| Ok(Box::new(value)),
});

type Bytes16 = [u8; 16];
type Bytes32 = [u8; 32];

uniffi::custom_type!(U64String, u64, {
    lower: |value| value.0,
    try_lift: |value| Ok(U64String(value)),
});
uniffi::custom_type!(Bytes16, Vec<u8>, {
    remote,
    lower: |value| value.to_vec(),
    try_lift: |value| value.try_into().map_err(|_| uniffi::deps::anyhow::anyhow!("expected 16 bytes")),
});
uniffi::custom_type!(Bytes32, Vec<u8>, {
    remote,
    lower: |value| value.to_vec(),
    try_lift: |value| value.try_into().map_err(|_| uniffi::deps::anyhow::anyhow!("expected 32 bytes")),
});

/// Refresh the full snapshot through the existing native actor.
///
/// Dropping the future cancels its waiter; an abandoned queued read does not
/// refresh the actor. The process-owned node survives that caller's lifetime.
#[uniffi::export]
pub async fn read_snapshot() -> DevelopmentNodeSnapshot {
    crate::lifecycle::snapshot_async().await
}

/// App and canonical contract identifiers, independent of UniFFI ABI checksums.
#[derive(uniffi::Record)]
pub struct BindingContract {
    pub app: String,
    pub host: String,
}

#[uniffi::export]
pub fn binding_contract() -> BindingContract {
    BindingContract {
        app: crate::contract::CONTRACT_FINGERPRINT.to_owned(),
        host: crate::contract::HOST_CONTRACT_FINGERPRINT.to_owned(),
    }
}
