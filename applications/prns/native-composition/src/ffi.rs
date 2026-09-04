use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::{ptr, slice, str};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::contract::{
    AnnounceRemoteControlTargetInput, CancelLxmfMessageInput, ContactDestinationInput,
    CreateManualContactInput, DescribeRemoteControlTargetInput, DevelopmentNodeStartInput,
    InitiateRemoteControlPairingInput, ListLxmfMessagesInput, MeasureLxmfTextInput,
    RemoteControlPairingDecisionInput, RetryLxmfMessageInput, SendDirectTextInput,
    SetContactAliasInput, SetContactPinnedInput, CONTRACT_FINGERPRINT, HOST_CONTRACT_FINGERPRINT,
};
use crate::lifecycle;

/// Maximum UTF-8 path length accepted by the application ABI.
pub const PRNS_APP_MAX_PATH_BYTES: usize = 4 * 1024;

/// Maximum JSON command length accepted by the application ABI.
pub const PRNS_APP_MAX_INPUT_BYTES: usize = 64 * 1024;

/// Maximum UTF-8 length accepted for one CoreBluetooth restoration identifier.
pub const PRNS_APP_MAX_RESTORATION_IDENTIFIER_BYTES: usize = 1024;

const CONTRACT_FINGERPRINT_C: [u8; CONTRACT_FINGERPRINT.len() + 1] = {
    let source = CONTRACT_FINGERPRINT.as_bytes();
    let mut result = [0; CONTRACT_FINGERPRINT.len() + 1];
    let mut index = 0;
    while index < source.len() {
        result[index] = source[index];
        index += 1;
    }
    result
};

const HOST_CONTRACT_FINGERPRINT_C: [u8; HOST_CONTRACT_FINGERPRINT.len() + 1] = {
    let source = HOST_CONTRACT_FINGERPRINT.as_bytes();
    let mut result = [0; HOST_CONTRACT_FINGERPRINT.len() + 1];
    let mut index = 0;
    while index < source.len() {
        result[index] = source[index];
        index += 1;
    }
    result
};

const BRIDGE_ENCODING_FALLBACK: &[u8] = br#"{"type":"bridgeFailure","kind":"panic","detail":"native bridge could not encode its result"}"#;

/// An owned, length-delimited UTF-8 JSON result returned by the application ABI.
///
/// The caller must pass every value returned by a `prns_app_*` result function
/// to [`prns_app_bytes_free`] exactly once. The byte buffer is not NUL-terminated.
#[repr(C)]
#[derive(Debug)]
pub struct PrnsAppBytes {
    pub ptr: *mut u8,
    pub len: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum BridgeFailureKind {
    InvalidInput,
    Panic,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeFailure {
    #[serde(rename = "type")]
    failure_type: &'static str,
    kind: BridgeFailureKind,
    detail: &'static str,
}

impl BridgeFailure {
    const fn invalid_input(detail: &'static str) -> Self {
        Self {
            failure_type: "bridgeFailure",
            kind: BridgeFailureKind::InvalidInput,
            detail,
        }
    }

    const fn panic() -> Self {
        Self {
            failure_type: "bridgeFailure",
            kind: BridgeFailureKind::Panic,
            detail: "native bridge operation failed",
        }
    }
}

/// Return the immutable, NUL-terminated contract fingerprint for this library.
///
/// The returned pointer is borrowed static storage and must not be freed.
#[no_mangle]
pub extern "C" fn prns_app_contract_fingerprint() -> *const c_char {
    CONTRACT_FINGERPRINT_C.as_ptr().cast()
}

/// Return the immutable, NUL-terminated canonical Host-contract fingerprint.
///
/// The returned pointer is borrowed static storage and must not be freed.
#[no_mangle]
pub extern "C" fn prns_app_host_contract_fingerprint() -> *const c_char {
    HOST_CONTRACT_FINGERPRINT_C.as_ptr().cast()
}

/// Inspect the application-owned primary identity.
///
/// # Safety
///
/// The path buffer follows the same contract as [`prns_app_start`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_inspect_identity(
    path_ptr: *const u8,
    path_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contract is forwarded to the bounded path decoder.
    unsafe { invoke_path(path_ptr, path_len, lifecycle::inspect_identity) }
}

/// Preview the identity hash derived from a raw credential without storing it.
///
/// # Safety
///
/// A non-null buffer must remain readable for the call. Empty input is a valid
/// domain input and returns `invalidLength`.
#[no_mangle]
pub unsafe extern "C" fn prns_app_preview_identity_import(
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contract is forwarded to the bounded byte decoder.
    unsafe { invoke_bytes(input_ptr, input_len, lifecycle::preview_identity_import) }
}

/// Generate and store the application-owned primary identity.
///
/// # Safety
///
/// The path buffer follows the same contract as [`prns_app_start`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_create_generated_identity(
    path_ptr: *const u8,
    path_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contract is forwarded to the bounded path decoder.
    unsafe { invoke_path(path_ptr, path_len, lifecycle::create_generated_identity) }
}

/// Validate and store a raw application-owned primary identity.
///
/// # Safety
///
/// Both non-empty buffers must remain readable for the call. Empty credential
/// input is a valid domain input and returns `invalidLength`.
#[no_mangle]
pub unsafe extern "C" fn prns_app_create_imported_identity(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_bytes(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::create_imported_identity,
        )
    }
}

/// Start the development node using an application-private storage directory.
///
/// # Safety
///
/// When `path_len` is nonzero, `path_ptr` must point to `path_len` readable
/// bytes for the duration of this call. The bytes must be UTF-8 and must not be
/// mutated concurrently. The length must not exceed
/// [`PRNS_APP_MAX_PATH_BYTES`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_start(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_json::<DevelopmentNodeStartInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::start_configured,
        )
    }
}

/// Prepare the application-owned CoreBluetooth restoration managers before full node startup.
///
/// This call returns only after the managers and their delegates exist. The resulting owner stays
/// in the process supervisor until a matching restoring start consumes it, or stop/reset drops it.
///
/// # Safety
///
/// The path and restoration-identifier buffers follow
/// [`prns_app_start_with_apple_bluetooth_central_restoration`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_prepare_apple_bluetooth_central_restoration(
    path_ptr: *const u8,
    path_len: usize,
    central_identifier_ptr: *const u8,
    central_identifier_len: usize,
) -> PrnsAppBytes {
    crate::ios_restoration_probe::install();
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_string(
            path_ptr,
            path_len,
            central_identifier_ptr,
            central_identifier_len,
            lifecycle::prepare_apple_bluetooth_central_restoration,
        )
    }
}

/// Start the development node with an application-owned central restoration identifier.
///
/// # Safety
///
/// The path and JSON buffers follow [`prns_app_start`]. Each restoration-identifier pointer must
/// address its corresponding number of readable, immutable UTF-8 bytes for this call. The identifier
/// must be nonempty and no longer than
/// [`PRNS_APP_MAX_RESTORATION_IDENTIFIER_BYTES`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_start_with_apple_bluetooth_central_restoration(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
    central_identifier_ptr: *const u8,
    central_identifier_len: usize,
) -> PrnsAppBytes {
    crate::ios_restoration_probe::install();
    // SAFETY: The caller contracts are forwarded to the bounded independent decoders.
    unsafe {
        invoke_path_json_string::<DevelopmentNodeStartInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            central_identifier_ptr,
            central_identifier_len,
            lifecycle::start_configured_with_apple_bluetooth_central_restoration,
        )
    }
}

/// Read the current authoritative development-node snapshot.
#[no_mangle]
pub extern "C" fn prns_app_snapshot() -> PrnsAppBytes {
    invoke(|| Ok(lifecycle::snapshot()))
}

/// List compatible LXMF peers retained by the current native generation.
#[no_mangle]
pub extern "C" fn prns_app_list_lxmf_peers() -> PrnsAppBytes {
    invoke(|| Ok(lifecycle::list_lxmf_peers()))
}

/// Measure one UTF-8 title/content pair against the direct Link-packet bound.
///
/// # Safety
///
/// The input buffer follows the JSON contract documented by
/// [`prns_app_initiate_pairing`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_measure_lxmf_text(
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contract is forwarded to the bounded JSON decoder.
    unsafe {
        invoke_json::<MeasureLxmfTextInput, _>(input_ptr, input_len, lifecycle::measure_lxmf_text)
    }
}

/// Emit the registered current-form LXMF announce on all active interfaces.
#[no_mangle]
pub extern "C" fn prns_app_announce_lxmf() -> PrnsAppBytes {
    invoke(|| Ok(lifecycle::announce_lxmf()))
}

/// Start one proof-gated direct LXMF text attempt.
///
/// # Safety
///
/// The input buffer follows the JSON contract documented by
/// [`prns_app_initiate_pairing`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_send_direct_text(
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contract is forwarded to the bounded JSON decoder.
    unsafe {
        invoke_json::<SendDirectTextInput, _>(input_ptr, input_len, lifecycle::send_direct_text)
    }
}

/// List one bounded page of durable LXMF messages.
///
/// # Safety
///
/// The input buffer follows the JSON contract documented by
/// [`prns_app_initiate_pairing`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_list_lxmf_messages(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_json::<ListLxmfMessagesInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::list_lxmf_messages,
        )
    }
}

/// Requeue one failed durable LXMF record without recomposing its wire.
///
/// # Safety
///
/// The path and input buffers follow [`prns_app_start`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_retry_lxmf_message(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_json::<RetryLxmfMessageInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::retry_lxmf_message,
        )
    }
}

/// Cancel one queued durable LXMF record using a Rust-owned timestamp.
///
/// # Safety
///
/// The path and input buffers follow [`prns_app_start`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_cancel_lxmf_message(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_json::<CancelLxmfMessageInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::cancel_lxmf_message,
        )
    }
}

/// Initiate controller pairing from a generated contract JSON input.
///
/// # Safety
///
/// When `input_len` is nonzero, `input_ptr` must point to `input_len` readable
/// bytes for the duration of this call. The bytes must not be mutated
/// concurrently. The length must not exceed [`PRNS_APP_MAX_INPUT_BYTES`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_initiate_pairing(
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contract for this exported function is forwarded to
    // `invoke_json`, which validates length and nullness before reading bytes.
    unsafe {
        invoke_json::<InitiateRemoteControlPairingInput, _>(
            input_ptr,
            input_len,
            lifecycle::initiate,
        )
    }
}

/// Approve a controller-pairing attempt from a generated contract JSON input.
///
/// # Safety
///
/// When `input_len` is nonzero, `input_ptr` must point to `input_len` readable
/// bytes for the duration of this call. The bytes must not be mutated
/// concurrently. The length must not exceed [`PRNS_APP_MAX_INPUT_BYTES`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_approve_pairing(
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contract for this exported function is forwarded to
    // `invoke_json`, which validates length and nullness before reading bytes.
    unsafe {
        invoke_json::<RemoteControlPairingDecisionInput, _>(
            input_ptr,
            input_len,
            lifecycle::approve,
        )
    }
}

/// Reject a controller-pairing attempt from a generated contract JSON input.
///
/// # Safety
///
/// When `input_len` is nonzero, `input_ptr` must point to `input_len` readable
/// bytes for the duration of this call. The bytes must not be mutated
/// concurrently. The length must not exceed [`PRNS_APP_MAX_INPUT_BYTES`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_reject_pairing(
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contract for this exported function is forwarded to
    // `invoke_json`, which validates length and nullness before reading bytes.
    unsafe {
        invoke_json::<RemoteControlPairingDecisionInput, _>(input_ptr, input_len, lifecycle::reject)
    }
}

/// Describe a paired target from a generated contract JSON input.
///
/// # Safety
///
/// When `input_len` is nonzero, `input_ptr` must point to `input_len` readable
/// bytes for the duration of this call. The bytes must not be mutated
/// concurrently. The length must not exceed [`PRNS_APP_MAX_INPUT_BYTES`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_describe_target(
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contract for this exported function is forwarded to
    // `invoke_json`, which validates length and nullness before reading bytes.
    unsafe {
        invoke_json::<DescribeRemoteControlTargetInput, _>(
            input_ptr,
            input_len,
            lifecycle::describe,
        )
    }
}

/// Submit one authorized target announcement; settlement is retained in snapshot.
///
/// # Safety
/// `input_ptr` must reference `input_len` immutable readable bytes for this call,
/// unless the length is zero. Length must not exceed `PRNS_APP_MAX_INPUT_BYTES`.
#[no_mangle]
pub unsafe extern "C" fn prns_app_announce_target(
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The public pointer/length contract is forwarded to the checked reader.
    unsafe {
        invoke_json::<AnnounceRemoteControlTargetInput, _>(
            input_ptr,
            input_len,
            lifecycle::announce_self,
        )
    }
}

/// Save a destination whose authenticated identity is known by the running node.
///
/// # Safety
///
/// The path and JSON input buffers must satisfy the bounds and readable-buffer
/// contracts documented by [`prns_app_start`] and [`prns_app_initiate_pairing`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_save_observed_destination(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_json::<ContactDestinationInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::save_observed_destination,
        )
    }
}

/// Create one manually entered contact.
///
/// # Safety
///
/// The path and JSON input buffers must satisfy the bounds and readable-buffer
/// contracts documented by [`prns_app_start`] and [`prns_app_initiate_pairing`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_create_manual_contact(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_json::<CreateManualContactInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::create_manual_contact,
        )
    }
}

/// Set or clear a saved contact alias.
///
/// # Safety
///
/// The path and JSON input buffers must satisfy the bounds and readable-buffer
/// contracts documented by [`prns_app_start`] and [`prns_app_initiate_pairing`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_set_contact_alias(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_json::<SetContactAliasInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::set_contact_alias,
        )
    }
}

/// Set a saved contact's pin state.
///
/// # Safety
///
/// The path and JSON input buffers must satisfy the bounds and readable-buffer
/// contracts documented by [`prns_app_start`] and [`prns_app_initiate_pairing`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_set_contact_pinned(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_json::<SetContactPinnedInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::set_contact_pinned,
        )
    }
}

/// Delete one saved contact.
///
/// # Safety
///
/// The path and JSON input buffers must satisfy the bounds and readable-buffer
/// contracts documented by [`prns_app_start`] and [`prns_app_initiate_pairing`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_delete_contact(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_json::<ContactDestinationInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::delete_contact,
        )
    }
}

/// Read one saved contact.
///
/// # Safety
///
/// The path and JSON input buffers must satisfy the bounds and readable-buffer
/// contracts documented by [`prns_app_start`] and [`prns_app_initiate_pairing`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_get_contact(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contracts are forwarded to the bounded decoders.
    unsafe {
        invoke_path_json::<ContactDestinationInput, _, _>(
            path_ptr,
            path_len,
            input_ptr,
            input_len,
            lifecycle::get_contact,
        )
    }
}

/// List every saved contact in destination-byte order.
///
/// # Safety
///
/// The path buffer follows the same contract as [`prns_app_start`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_list_contacts(
    path_ptr: *const u8,
    path_len: usize,
) -> PrnsAppBytes {
    // SAFETY: The caller contract is forwarded to the bounded path decoder.
    unsafe { invoke_path(path_ptr, path_len, lifecycle::list_contacts) }
}

/// Stop the development node and return its generated stop outcome.
#[no_mangle]
pub extern "C" fn prns_app_stop() -> PrnsAppBytes {
    invoke(|| Ok(lifecycle::stop()))
}

/// Stop the development node and remove its application-private storage.
///
/// # Safety
///
/// When `path_len` is nonzero, `path_ptr` must point to `path_len` readable
/// bytes for the duration of this call. The bytes must be UTF-8 and must not be
/// mutated concurrently. The length must not exceed
/// [`PRNS_APP_MAX_PATH_BYTES`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_reset(path_ptr: *const u8, path_len: usize) -> PrnsAppBytes {
    // SAFETY: The caller contract for this exported function is forwarded to
    // `invoke_path`, which validates length and nullness before reading bytes.
    unsafe { invoke_path(path_ptr, path_len, lifecycle::reset) }
}

/// Release one owned result buffer returned by this ABI.
///
/// Passing `{ NULL, 0 }` is a no-op. Any non-null value must be an unchanged
/// `PrnsAppBytes` returned by this exact library and must be released exactly
/// once.
///
/// # Safety
///
/// For a non-null pointer, `bytes.ptr` and `bytes.len` must be the exact pair
/// returned by a `prns_app_*` result function from this library. The allocation
/// must not already have been released or otherwise transferred.
#[no_mangle]
pub unsafe extern "C" fn prns_app_bytes_free(bytes: PrnsAppBytes) {
    if bytes.ptr.is_null() {
        return;
    }

    let raw = ptr::slice_from_raw_parts_mut(bytes.ptr, bytes.len);
    // SAFETY: The caller promises the pointer/length pair came from
    // `owned_bytes`, which leaked exactly one boxed slice with these parts.
    unsafe { drop(Box::from_raw(raw)) };
}

fn invoke<T>(operation: impl FnOnce() -> Result<T, BridgeFailure>) -> PrnsAppBytes
where
    T: Serialize,
{
    match catch_unwind(AssertUnwindSafe(|| match operation() {
        Ok(value) => encode_owned(&value),
        Err(failure) => encode_bridge_failure(&failure),
    })) {
        Ok(bytes) => bytes,
        Err(_) => encode_bridge_failure(&BridgeFailure::panic()),
    }
}

unsafe fn invoke_path<T>(
    path_ptr: *const u8,
    path_len: usize,
    operation: impl FnOnce(&Path) -> T,
) -> PrnsAppBytes
where
    T: Serialize,
{
    invoke(|| {
        // SAFETY: `invoke_path` carries the caller's readable-buffer contract;
        // `required_bytes` checks bounds and nullness before forming the slice.
        let bytes = unsafe {
            required_bytes(
                path_ptr,
                path_len,
                PRNS_APP_MAX_PATH_BYTES,
                "storage path must not be empty",
                "storage path pointer is null",
                "storage path exceeds the ABI size limit",
            )
        }?;
        let path = str::from_utf8(bytes)
            .map_err(|_| BridgeFailure::invalid_input("storage path is not valid UTF-8"))?;
        Ok(operation(Path::new(path)))
    })
}

unsafe fn invoke_bytes<T>(
    input_ptr: *const u8,
    input_len: usize,
    operation: impl FnOnce(&[u8]) -> T,
) -> PrnsAppBytes
where
    T: Serialize,
{
    invoke(|| {
        // SAFETY: `invoke_bytes` carries the caller's readable-buffer contract.
        let bytes = unsafe { optional_bytes(input_ptr, input_len)? };
        Ok(operation(bytes))
    })
}

unsafe fn invoke_path_bytes<T>(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
    operation: impl FnOnce(&Path, &[u8]) -> T,
) -> PrnsAppBytes
where
    T: Serialize,
{
    invoke(|| {
        // SAFETY: The caller contracts are checked before either slice is formed.
        let path_bytes = unsafe {
            required_bytes(
                path_ptr,
                path_len,
                PRNS_APP_MAX_PATH_BYTES,
                "storage path must not be empty",
                "storage path pointer is null",
                "storage path exceeds the ABI size limit",
            )
        }?;
        let path = str::from_utf8(path_bytes)
            .map_err(|_| BridgeFailure::invalid_input("storage path is not valid UTF-8"))?;
        // SAFETY: `optional_bytes` checks the independent credential buffer.
        let input = unsafe { optional_bytes(input_ptr, input_len)? };
        Ok(operation(Path::new(path), input))
    })
}

unsafe fn invoke_path_json<Input, Output, Operation>(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
    operation: Operation,
) -> PrnsAppBytes
where
    Input: DeserializeOwned,
    Output: Serialize,
    Operation: FnOnce(&Path, Input) -> Output,
{
    invoke(|| {
        // SAFETY: The caller contracts are checked before either slice is formed.
        let path_bytes = unsafe {
            required_bytes(
                path_ptr,
                path_len,
                PRNS_APP_MAX_PATH_BYTES,
                "storage path must not be empty",
                "storage path pointer is null",
                "storage path exceeds the ABI size limit",
            )
        }?;
        let path = str::from_utf8(path_bytes)
            .map_err(|_| BridgeFailure::invalid_input("storage path is not valid UTF-8"))?;
        // SAFETY: The independent JSON buffer uses the same bounded input contract.
        let input_bytes = unsafe {
            required_bytes(
                input_ptr,
                input_len,
                PRNS_APP_MAX_INPUT_BYTES,
                "contract input must not be empty",
                "contract input pointer is null",
                "contract input exceeds the ABI size limit",
            )
        }?;
        let input = serde_json::from_slice(input_bytes)
            .map_err(|_| BridgeFailure::invalid_input("contract input is not valid JSON"))?;
        Ok(operation(Path::new(path), input))
    })
}

unsafe fn invoke_path_json_string<Input, Output, Operation>(
    path_ptr: *const u8,
    path_len: usize,
    input_ptr: *const u8,
    input_len: usize,
    identifier_ptr: *const u8,
    identifier_len: usize,
    operation: Operation,
) -> PrnsAppBytes
where
    Input: DeserializeOwned,
    Output: Serialize,
    Operation: FnOnce(&Path, Input, String) -> Output,
{
    invoke(|| {
        // SAFETY: Each buffer has an independent readable-buffer contract and is checked before a
        // slice is formed.
        let path_bytes = unsafe {
            required_bytes(
                path_ptr,
                path_len,
                PRNS_APP_MAX_PATH_BYTES,
                "storage path must not be empty",
                "storage path pointer is null",
                "storage path exceeds the ABI size limit",
            )
        }?;
        let path = str::from_utf8(path_bytes)
            .map_err(|_| BridgeFailure::invalid_input("storage path is not valid UTF-8"))?;
        // SAFETY: The independent JSON buffer uses the same bounded input contract.
        let input_bytes = unsafe {
            required_bytes(
                input_ptr,
                input_len,
                PRNS_APP_MAX_INPUT_BYTES,
                "contract input must not be empty",
                "contract input pointer is null",
                "contract input exceeds the ABI size limit",
            )
        }?;
        let input = serde_json::from_slice(input_bytes)
            .map_err(|_| BridgeFailure::invalid_input("contract input is not valid JSON"))?;
        // SAFETY: The identifier buffer has checked bounds and nullness.
        let identifier_bytes = unsafe {
            required_bytes(
                identifier_ptr,
                identifier_len,
                PRNS_APP_MAX_RESTORATION_IDENTIFIER_BYTES,
                "central restoration identifier must not be empty",
                "central restoration identifier pointer is null",
                "central restoration identifier exceeds the ABI size limit",
            )
        }?;
        let identifier = str::from_utf8(identifier_bytes).map_err(|_| {
            BridgeFailure::invalid_input("central restoration identifier is not valid UTF-8")
        })?;
        Ok(operation(Path::new(path), input, identifier.to_owned()))
    })
}

unsafe fn invoke_path_string<Output, Operation>(
    path_ptr: *const u8,
    path_len: usize,
    identifier_ptr: *const u8,
    identifier_len: usize,
    operation: Operation,
) -> PrnsAppBytes
where
    Output: Serialize,
    Operation: FnOnce(&Path, String) -> Output,
{
    invoke(|| {
        // SAFETY: Each buffer has an independent readable-buffer contract and is checked before a
        // slice is formed.
        let path_bytes = unsafe {
            required_bytes(
                path_ptr,
                path_len,
                PRNS_APP_MAX_PATH_BYTES,
                "storage path must not be empty",
                "storage path pointer is null",
                "storage path exceeds the ABI size limit",
            )
        }?;
        let path = str::from_utf8(path_bytes)
            .map_err(|_| BridgeFailure::invalid_input("storage path is not valid UTF-8"))?;
        // SAFETY: The identifier buffer has checked bounds and nullness.
        let identifier_bytes = unsafe {
            required_bytes(
                identifier_ptr,
                identifier_len,
                PRNS_APP_MAX_RESTORATION_IDENTIFIER_BYTES,
                "central restoration identifier must not be empty",
                "central restoration identifier pointer is null",
                "central restoration identifier exceeds the ABI size limit",
            )
        }?;
        let identifier = str::from_utf8(identifier_bytes).map_err(|_| {
            BridgeFailure::invalid_input("central restoration identifier is not valid UTF-8")
        })?;
        Ok(operation(Path::new(path), identifier.to_owned()))
    })
}

unsafe fn optional_bytes<'a>(
    input_ptr: *const u8,
    input_len: usize,
) -> Result<&'a [u8], BridgeFailure> {
    if input_len == 0 {
        return Ok(&[]);
    }
    if input_len > PRNS_APP_MAX_INPUT_BYTES {
        return Err(BridgeFailure::invalid_input(
            "identity input exceeds the ABI size limit",
        ));
    }
    if input_ptr.is_null() {
        return Err(BridgeFailure::invalid_input(
            "identity input pointer is null",
        ));
    }
    // SAFETY: The caller guarantees this checked non-null pointer is readable
    // for `input_len` bytes for the duration of the call.
    Ok(unsafe { slice::from_raw_parts(input_ptr, input_len) })
}

unsafe fn invoke_json<Input, Output>(
    input_ptr: *const u8,
    input_len: usize,
    operation: impl FnOnce(Input) -> Output,
) -> PrnsAppBytes
where
    Input: DeserializeOwned,
    Output: Serialize,
{
    invoke(|| {
        // SAFETY: `invoke_json` carries the caller's readable-buffer contract;
        // `required_bytes` checks bounds and nullness before forming the slice.
        let bytes = unsafe {
            required_bytes(
                input_ptr,
                input_len,
                PRNS_APP_MAX_INPUT_BYTES,
                "contract input must not be empty",
                "contract input pointer is null",
                "contract input exceeds the ABI size limit",
            )
        }?;
        let input = serde_json::from_slice(bytes)
            .map_err(|_| BridgeFailure::invalid_input("contract input is not valid JSON"))?;
        Ok(operation(input))
    })
}

unsafe fn required_bytes<'a>(
    input_ptr: *const u8,
    input_len: usize,
    maximum: usize,
    empty_detail: &'static str,
    null_detail: &'static str,
    oversized_detail: &'static str,
) -> Result<&'a [u8], BridgeFailure> {
    if input_len == 0 {
        return Err(BridgeFailure::invalid_input(empty_detail));
    }
    if input_len > maximum {
        return Err(BridgeFailure::invalid_input(oversized_detail));
    }
    if input_ptr.is_null() {
        return Err(BridgeFailure::invalid_input(null_detail));
    }

    // SAFETY: The caller guarantees that a non-null pointer is readable for
    // `input_len` bytes. The checks above reject zero and oversized lengths.
    Ok(unsafe { slice::from_raw_parts(input_ptr, input_len) })
}

fn encode_owned(value: &impl Serialize) -> PrnsAppBytes {
    match serde_json::to_vec(value) {
        Ok(bytes) => owned_bytes(bytes),
        Err(_) => encode_bridge_failure(&BridgeFailure::panic()),
    }
}

fn encode_bridge_failure(failure: &BridgeFailure) -> PrnsAppBytes {
    let bytes = serde_json::to_vec(failure).unwrap_or_else(|_| BRIDGE_ENCODING_FALLBACK.to_vec());
    owned_bytes(bytes)
}

fn owned_bytes(bytes: Vec<u8>) -> PrnsAppBytes {
    let bytes = bytes.into_boxed_slice();
    let len = bytes.len();
    let ptr = Box::into_raw(bytes).cast::<u8>();
    PrnsAppBytes { ptr, len }
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;
    use std::ptr::NonNull;

    use serde_json::{json, Value};

    use super::*;

    fn parse_and_free(bytes: PrnsAppBytes) -> Value {
        assert!(!bytes.ptr.is_null());
        // SAFETY: The result came directly from this module and remains owned
        // until it is released below.
        let encoded = unsafe { slice::from_raw_parts(bytes.ptr, bytes.len) };
        let value = serde_json::from_slice(encoded).unwrap_or(Value::Null);
        // SAFETY: This is the first and only release of the unchanged result.
        unsafe { prns_app_bytes_free(bytes) };
        value
    }

    #[test]
    fn fingerprint_is_borrowed_nul_terminated_contract_value() {
        let pointer = prns_app_contract_fingerprint();
        assert!(!pointer.is_null());
        // SAFETY: The function returns a borrowed static NUL-terminated array.
        let fingerprint = unsafe { CStr::from_ptr(pointer) };
        assert_eq!(fingerprint.to_bytes(), CONTRACT_FINGERPRINT.as_bytes());

        let host_pointer = prns_app_host_contract_fingerprint();
        assert!(!host_pointer.is_null());
        // SAFETY: The function returns a borrowed static NUL-terminated array.
        let host_fingerprint = unsafe { CStr::from_ptr(host_pointer) };
        assert_eq!(
            host_fingerprint.to_bytes(),
            HOST_CONTRACT_FINGERPRINT.as_bytes()
        );
    }

    #[test]
    fn empty_identity_bytes_are_a_domain_result_not_a_bridge_failure() {
        // SAFETY: Null plus zero length is the documented empty identity input.
        let output = unsafe { invoke_bytes(ptr::null(), 0, lifecycle::preview_identity_import) };
        assert_eq!(parse_and_free(output), json!({ "type": "invalidLength" }));
    }

    #[test]
    fn owned_results_are_utf8_json_and_can_be_released() {
        let output = invoke(|| Ok(json!({ "type": "busy" })));
        assert_eq!(parse_and_free(output), json!({ "type": "busy" }));

        // SAFETY: The documented sentinel has no allocation to release.
        unsafe {
            prns_app_bytes_free(PrnsAppBytes {
                ptr: ptr::null_mut(),
                len: 0,
            });
        }
    }

    #[test]
    fn null_and_malformed_inputs_return_bridge_failures() {
        // SAFETY: A null pointer is intentionally supplied to exercise the
        // pre-dereference validation path.
        let null = unsafe {
            invoke_json::<InitiateRemoteControlPairingInput, _>(
                ptr::null(),
                1,
                |_| json!({ "unreachable": true }),
            )
        };
        assert_eq!(
            parse_and_free(null),
            json!({
                "type": "bridgeFailure",
                "kind": "invalidInput",
                "detail": "contract input pointer is null"
            })
        );

        let malformed = b"{";
        // SAFETY: `malformed` remains readable and immutable for this call.
        let invalid_json = unsafe {
            invoke_json::<InitiateRemoteControlPairingInput, _>(
                malformed.as_ptr(),
                malformed.len(),
                |_| json!({ "unreachable": true }),
            )
        };
        assert_eq!(
            parse_and_free(invalid_json),
            json!({
                "type": "bridgeFailure",
                "kind": "invalidInput",
                "detail": "contract input is not valid JSON"
            })
        );
    }

    #[test]
    fn oversized_input_is_rejected_before_its_pointer_is_read() {
        let unreadable = NonNull::<u8>::dangling().as_ptr().cast_const();
        // SAFETY: The intentionally non-readable pointer is paired with an
        // oversized length, which must be rejected before a slice is formed.
        let output = unsafe {
            invoke_json::<InitiateRemoteControlPairingInput, _>(
                unreadable,
                PRNS_APP_MAX_INPUT_BYTES + 1,
                |_| json!({ "unreachable": true }),
            )
        };
        assert_eq!(
            parse_and_free(output),
            json!({
                "type": "bridgeFailure",
                "kind": "invalidInput",
                "detail": "contract input exceeds the ABI size limit"
            })
        );
    }

    #[test]
    fn storage_path_must_be_nonempty_bounded_utf8() {
        // SAFETY: A null pointer and zero length intentionally exercise the
        // empty-input validation path without forming a slice.
        let empty = unsafe { invoke_path(ptr::null(), 0, |_| json!({ "unreachable": true })) };
        assert_eq!(
            parse_and_free(empty),
            json!({
                "type": "bridgeFailure",
                "kind": "invalidInput",
                "detail": "storage path must not be empty"
            })
        );

        let invalid_utf8 = [0xff];
        // SAFETY: `invalid_utf8` remains readable and immutable for this call.
        let invalid = unsafe {
            invoke_path(
                invalid_utf8.as_ptr(),
                invalid_utf8.len(),
                |_| json!({ "unreachable": true }),
            )
        };
        assert_eq!(
            parse_and_free(invalid),
            json!({
                "type": "bridgeFailure",
                "kind": "invalidInput",
                "detail": "storage path is not valid UTF-8"
            })
        );
    }

    #[test]
    fn valid_generated_input_reaches_the_operation_without_reprojection() {
        let encoded = br#"{"candidateId":"candidate-1","invitationCode":"123456"}"#;
        // SAFETY: `encoded` remains readable and immutable for this call.
        let output = unsafe {
            invoke_json::<InitiateRemoteControlPairingInput, _>(
                encoded.as_ptr(),
                encoded.len(),
                |input| {
                    assert_eq!(input.candidate_id, "candidate-1");
                    assert_eq!(input.invitation_code, "123456");
                    json!({ "type": "busy" })
                },
            )
        };
        assert_eq!(parse_and_free(output), json!({ "type": "busy" }));
    }

    #[test]
    fn restoring_start_buffers_reach_the_operation_exactly() {
        let path = b"/tmp/prns/restoring";
        let input = br#"{"developmentTcpTarget":null}"#;
        let central = b"rs.reticulum.prns.dev.bluetooth-auto.central.v1";
        // SAFETY: All fixed test buffers remain readable and immutable for the call.
        let output = unsafe {
            invoke_path_json_string::<DevelopmentNodeStartInput, _, _>(
                path.as_ptr(),
                path.len(),
                input.as_ptr(),
                input.len(),
                central.as_ptr(),
                central.len(),
                |decoded_path, decoded_input, decoded_central| {
                    assert_eq!(decoded_path, Path::new("/tmp/prns/restoring"));
                    assert_eq!(decoded_input.development_tcp_target, None);
                    assert_eq!(decoded_central.as_bytes(), central);
                    json!({ "type": "accepted" })
                },
            )
        };
        assert_eq!(parse_and_free(output), json!({ "type": "accepted" }));

        // SAFETY: The other buffers remain valid; null plus zero is intentionally invalid for the
        // required central identifier.
        let empty_central = unsafe {
            invoke_path_json_string::<DevelopmentNodeStartInput, _, _>(
                path.as_ptr(),
                path.len(),
                input.as_ptr(),
                input.len(),
                ptr::null(),
                0,
                |_, _, _| json!({ "unreachable": true }),
            )
        };
        assert_eq!(
            parse_and_free(empty_central),
            json!({
                "type": "bridgeFailure",
                "kind": "invalidInput",
                "detail": "central restoration identifier must not be empty"
            })
        );
    }

    #[test]
    fn restoration_preparation_buffers_reach_the_operation_exactly() {
        let path = b"/tmp/prns/development";
        let central = b"rs.reticulum.prns.dev.bluetooth-auto.central.v1";
        // SAFETY: All fixed test buffers remain readable and immutable for the call.
        let output = unsafe {
            invoke_path_string(
                path.as_ptr(),
                path.len(),
                central.as_ptr(),
                central.len(),
                |decoded_path, decoded_central| {
                    assert_eq!(decoded_path, Path::new("/tmp/prns/development"));
                    assert_eq!(decoded_central.as_bytes(), central);
                    json!({ "type": "prepared" })
                },
            )
        };
        assert_eq!(parse_and_free(output), json!({ "type": "prepared" }));

        // SAFETY: Null plus zero is intentionally invalid for the required central identifier.
        let empty_central = unsafe {
            invoke_path_string(
                path.as_ptr(),
                path.len(),
                ptr::null(),
                0,
                |_, _| json!({ "unreachable": true }),
            )
        };
        assert_eq!(
            parse_and_free(empty_central),
            json!({
                "type": "bridgeFailure",
                "kind": "invalidInput",
                "detail": "central restoration identifier must not be empty"
            })
        );
    }

    #[test]
    fn manual_contact_requires_explicit_nullable_fields_on_the_wire() {
        let path = b"/tmp/prns/development";
        let missing_alias = br#"{"destination":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"identity":null}"#;
        // SAFETY: Both fixed test buffers remain readable for the call.
        let rejected = unsafe {
            invoke_path_json::<CreateManualContactInput, _, _>(
                path.as_ptr(),
                path.len(),
                missing_alias.as_ptr(),
                missing_alias.len(),
                |_, _| json!({ "unreachable": true }),
            )
        };
        assert_eq!(
            parse_and_free(rejected),
            json!({
                "type": "bridgeFailure",
                "kind": "invalidInput",
                "detail": "contract input is not valid JSON"
            })
        );

        let explicit_nulls =
            br#"{"destination":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"identity":null,"alias":null}"#;
        // SAFETY: Both fixed test buffers remain readable for the call.
        let accepted = unsafe {
            invoke_path_json::<CreateManualContactInput, _, _>(
                path.as_ptr(),
                path.len(),
                explicit_nulls.as_ptr(),
                explicit_nulls.len(),
                |decoded_path, input| {
                    assert_eq!(decoded_path, Path::new("/tmp/prns/development"));
                    assert_eq!(input.identity, None);
                    assert_eq!(input.alias, None);
                    json!({ "type": "accepted" })
                },
            )
        };
        assert_eq!(parse_and_free(accepted), json!({ "type": "accepted" }));
    }

    #[test]
    fn durable_mailbox_commands_forward_the_storage_path_and_exact_record_id() {
        let path = b"/tmp/prns/durable-mailbox";
        let encoded = br#"{"localRecordId":"18446744073709551615"}"#;
        // SAFETY: Both fixed test buffers remain readable and immutable for the call.
        let retried = unsafe {
            invoke_path_json::<RetryLxmfMessageInput, _, _>(
                path.as_ptr(),
                path.len(),
                encoded.as_ptr(),
                encoded.len(),
                |decoded_path, input| {
                    assert_eq!(decoded_path, Path::new("/tmp/prns/durable-mailbox"));
                    assert_eq!(input.local_record_id.0, u64::MAX.to_string());
                    json!({ "type": "accepted", "localRecordId": input.local_record_id.0 })
                },
            )
        };
        assert_eq!(
            parse_and_free(retried),
            json!({ "type": "accepted", "localRecordId": u64::MAX.to_string() })
        );

        // SAFETY: Both fixed test buffers remain readable and immutable for the call.
        let cancelled = unsafe {
            invoke_path_json::<CancelLxmfMessageInput, _, _>(
                path.as_ptr(),
                path.len(),
                encoded.as_ptr(),
                encoded.len(),
                |decoded_path, input| {
                    assert_eq!(decoded_path, Path::new("/tmp/prns/durable-mailbox"));
                    json!({ "type": "cancelled", "localRecordId": input.local_record_id.0 })
                },
            )
        };
        assert_eq!(
            parse_and_free(cancelled),
            json!({ "type": "cancelled", "localRecordId": u64::MAX.to_string() })
        );
    }

    #[test]
    fn panic_payloads_are_contained_and_redacted() {
        let output = invoke::<Value>(|| {
            std::panic::resume_unwind(Box::new("sensitive panic payload"));
        });
        assert_eq!(
            parse_and_free(output),
            json!({
                "type": "bridgeFailure",
                "kind": "panic",
                "detail": "native bridge operation failed"
            })
        );
    }
}
