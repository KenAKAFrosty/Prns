//! Generated foreign-language views over the application-owned native composition.
//!
//! This module creates neither a node nor a runtime. Platform lifecycle owners and
//! generated JavaScript callers must link/load the same `prns_app` library image.

mod host_generated;

use crate::contract::*;
use std::path::Path;

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

/// Initialize the process-owned database. Platform background/lifecycle queue
/// only: this may perform blocking storage I/O and wait for native transitions.
#[uniffi::export]
pub fn native_prepare_storage(storage_root: String) -> NativeStoragePreparationOutcome {
    crate::lifecycle::generated::prepare_native_storage(Path::new(&storage_root))
}

/// Platform background/lifecycle queue only.
#[uniffi::export]
pub fn native_inspect_identity(storage_root: String) -> PrimaryIdentityState {
    crate::lifecycle::inspect_identity(Path::new(&storage_root))
}
/// Platform background/lifecycle queue only.
#[uniffi::export]
pub fn native_create_generated_identity(storage_root: String) -> IdentityCreationOutcome {
    crate::lifecycle::create_generated_identity(Path::new(&storage_root))
}
/// Platform background/lifecycle queue only.
#[uniffi::export]
pub fn native_create_imported_identity(
    storage_root: String,
    identity: Vec<u8>,
) -> IdentityCreationOutcome {
    crate::lifecycle::create_imported_identity(Path::new(&storage_root), &identity)
}
/// Bounded local validation; no storage or native lifecycle admission.
#[uniffi::export]
pub fn preview_identity_import(identity: Vec<u8>) -> IdentityImportPreviewOutcome {
    crate::lifecycle::preview_identity_import(&identity)
}
/// Platform lifecycle queue only, after platform permission/radio admission.
#[uniffi::export]
pub fn native_start(
    storage_root: String,
    input: DevelopmentNodeStartInput,
) -> DevelopmentNodeStartOutcome {
    crate::lifecycle::start_configured(Path::new(&storage_root), input)
}
/// Platform lifecycle queue only, preserving native-before-JavaScript restoration.
#[uniffi::export]
pub fn native_start_with_apple_bluetooth_central_restoration(
    storage_root: String,
    input: DevelopmentNodeStartInput,
    central_identifier: String,
) -> DevelopmentNodeStartOutcome {
    crate::lifecycle::start_configured_with_apple_bluetooth_central_restoration(
        Path::new(&storage_root),
        input,
        central_identifier,
    )
}
/// Platform lifecycle queue only; does not start a JavaScript-owned node.
#[uniffi::export]
pub fn native_prepare_apple_bluetooth_central_restoration(
    storage_root: String,
    central_identifier: String,
) -> AppleBluetoothRestorationPreparationOutcome {
    crate::lifecycle::prepare_apple_bluetooth_central_restoration(
        Path::new(&storage_root),
        central_identifier,
    )
}
/// Platform lifecycle queue only. The native owner retains failed-stop authority.
#[uniffi::export]
pub fn native_stop() -> DevelopmentNodeStopOutcome {
    crate::lifecycle::stop()
}
/// Platform lifecycle queue only. Drains the existing owners before deleting data.
#[uniffi::export]
pub fn native_reset(storage_root: String) -> DevelopmentNodeStopOutcome {
    crate::lifecycle::reset(Path::new(&storage_root))
}

#[uniffi::export]
pub async fn initiate_pairing(
    input: InitiateRemoteControlPairingInput,
) -> RemoteControlPairingCommandOutcome {
    crate::lifecycle::generated::initiate_pairing(input).await
}

#[uniffi::export]
pub async fn approve_pairing(
    input: RemoteControlPairingDecisionInput,
) -> RemoteControlPairingCommandOutcome {
    crate::lifecycle::generated::approve_pairing(input).await
}

#[uniffi::export]
pub async fn reject_pairing(
    input: RemoteControlPairingDecisionInput,
) -> RemoteControlPairingCommandOutcome {
    crate::lifecycle::generated::reject_pairing(input).await
}

#[uniffi::export]
pub async fn describe_target(
    input: DescribeRemoteControlTargetInput,
) -> RemoteControlDescribeOutcome {
    crate::lifecycle::generated::describe_target(input).await
}

#[uniffi::export]
pub async fn announce_target(
    input: AnnounceRemoteControlTargetInput,
) -> RemoteControlAnnounceOutcome {
    crate::lifecycle::generated::announce_target(input).await
}

#[uniffi::export]
pub async fn save_observed_destination(input: ContactDestinationInput) -> ContactMutationOutcome {
    crate::lifecycle::generated::save_observed_destination(input).await
}

#[uniffi::export]
pub async fn create_manual_contact(input: CreateManualContactInput) -> ContactMutationOutcome {
    crate::lifecycle::generated::create_manual_contact(input).await
}

#[uniffi::export]
pub async fn set_contact_alias(input: SetContactAliasInput) -> ContactMutationOutcome {
    crate::lifecycle::generated::set_contact_alias(input).await
}

#[uniffi::export]
pub async fn set_contact_pinned(input: SetContactPinnedInput) -> ContactMutationOutcome {
    crate::lifecycle::generated::set_contact_pinned(input).await
}

#[uniffi::export]
pub async fn delete_contact(input: ContactDestinationInput) -> ContactMutationOutcome {
    crate::lifecycle::generated::delete_contact(input).await
}

#[uniffi::export]
pub async fn get_contact(input: ContactDestinationInput) -> ContactLookupOutcome {
    crate::lifecycle::generated::get_contact(input).await
}

#[uniffi::export]
pub async fn list_contacts() -> ContactListOutcome {
    crate::lifecycle::generated::list_contacts().await
}

#[uniffi::export]
pub async fn list_lxmf_peers() -> LxmfPeerListOutcome {
    crate::lifecycle::generated::list_lxmf_peers().await
}

#[uniffi::export]
pub async fn list_lxmf_messages(input: ListLxmfMessagesInput) -> LxmfMessageListOutcome {
    crate::lifecycle::generated::list_lxmf_messages(input).await
}

#[uniffi::export]
pub async fn retry_lxmf_message(input: RetryLxmfMessageInput) -> RetryLxmfMessageOutcome {
    crate::lifecycle::generated::retry_lxmf_message(input).await
}

#[uniffi::export]
pub async fn cancel_lxmf_message(input: CancelLxmfMessageInput) -> CancelLxmfMessageOutcome {
    crate::lifecycle::generated::cancel_lxmf_message(input).await
}

#[uniffi::export]
pub async fn announce_lxmf() -> AnnounceLxmfOutcome {
    crate::lifecycle::generated::announce_lxmf().await
}

#[uniffi::export]
pub async fn measure_lxmf_text(input: MeasureLxmfTextInput) -> MeasureLxmfTextOutcome {
    crate::lifecycle::generated::measure_lxmf_text(input).await
}

#[uniffi::export]
pub async fn send_direct_text(input: SendDirectTextInput) -> SendDirectTextOutcome {
    crate::lifecycle::generated::send_direct_text(input).await
}
