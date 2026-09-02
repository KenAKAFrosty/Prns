use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::{ptr, slice, str};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::contract::{
    DescribeRemoteControlTargetInput, InitiateRemoteControlPairingInput,
    RemoteControlPairingDecisionInput, CONTRACT_FINGERPRINT,
};
use crate::lifecycle;

/// Maximum UTF-8 path length accepted by the application ABI.
pub const PRNS_APP_MAX_PATH_BYTES: usize = 4 * 1024;

/// Maximum JSON command length accepted by the application ABI.
pub const PRNS_APP_MAX_INPUT_BYTES: usize = 64 * 1024;

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

/// Start the development node using an application-private storage directory.
///
/// # Safety
///
/// When `path_len` is nonzero, `path_ptr` must point to `path_len` readable
/// bytes for the duration of this call. The bytes must be UTF-8 and must not be
/// mutated concurrently. The length must not exceed
/// [`PRNS_APP_MAX_PATH_BYTES`].
#[no_mangle]
pub unsafe extern "C" fn prns_app_start(path_ptr: *const u8, path_len: usize) -> PrnsAppBytes {
    // SAFETY: The caller contract for this exported function is forwarded to
    // `invoke_path`, which validates length and nullness before reading bytes.
    unsafe { invoke_path(path_ptr, path_len, lifecycle::start) }
}

/// Read the current authoritative development-node snapshot.
#[no_mangle]
pub extern "C" fn prns_app_snapshot() -> PrnsAppBytes {
    invoke(|| Ok(lifecycle::snapshot()))
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
