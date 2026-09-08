//! Thin JNI entry points over the existing bounded application C ABI.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jstring};
use jni::JNIEnv;

use crate::contract::{CONTRACT_FINGERPRINT, HOST_CONTRACT_FINGERPRINT};
use crate::ffi::{self, PrnsAppBytes};

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsNative_nativePrepareBluetooth(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    super::owner().prepare().into()
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsNative_nativeReleaseBluetooth(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    super::owner().release().into()
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsNative_nativeContractFingerprint(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    java_string(&mut env, CONTRACT_FINGERPRINT)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsNative_nativeHostContractFingerprint(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    java_string(&mut env, HOST_CONTRACT_FINGERPRINT)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsNative_nativeCall(
    mut env: JNIEnv,
    _class: JClass,
    operation: JString,
    storage_path: JString,
    input: JByteArray,
) -> jstring {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let operation = read_string(&mut env, &operation, 64)?;
        let path = read_string(&mut env, &storage_path, ffi::PRNS_APP_MAX_PATH_BYTES)?;
        let input = read_input(&env, &input)?;
        Ok::<_, &'static str>(consume(dispatch(&operation, path.as_bytes(), &input)))
    }));
    let output = match result {
        Ok(Ok(output)) => output,
        Ok(Err(detail)) => consume(ffi::invalid_bridge_input(detail)),
        Err(_) => "{\"type\":\"bridgeFailure\",\"kind\":\"panic\",\"detail\":\"native bridge operation failed\"}".to_owned(),
    };
    java_string(&mut env, &output)
}

fn read_string(env: &mut JNIEnv, value: &JString, maximum: usize) -> Result<String, &'static str> {
    if value.is_null() {
        return Ok(String::new());
    }
    let text = env
        .get_string(value)
        .map_err(|_| "could not read JNI string")?;
    if text.to_bytes().len() > maximum {
        return Err("JNI string exceeds the ABI size limit");
    }
    Ok(text.into())
}

fn read_input(env: &JNIEnv, input: &JByteArray) -> Result<Vec<u8>, &'static str> {
    if input.is_null() {
        return Ok(Vec::new());
    }
    let len = env
        .get_array_length(input)
        .map_err(|_| "could not read JNI input length")?;
    if usize::try_from(len).map_or(true, |len| len > ffi::PRNS_APP_MAX_INPUT_BYTES) {
        return Err("JNI input exceeds the ABI size limit");
    }
    env.convert_byte_array(input)
        .map_err(|_| "could not copy JNI input")
}

fn java_string(env: &mut JNIEnv, value: &str) -> jstring {
    match env.new_string(value) {
        Ok(value) => value.into_raw(),
        Err(_) => {
            if !env.exception_check().unwrap_or(true) {
                let _ = env.throw_new(
                    "java/lang/IllegalStateException",
                    "could not encode native result",
                );
            }
            ptr::null_mut()
        }
    }
}

struct OwnedResult(PrnsAppBytes);

impl Drop for OwnedResult {
    fn drop(&mut self) {
        let bytes = std::mem::replace(
            &mut self.0,
            PrnsAppBytes {
                ptr: ptr::null_mut(),
                len: 0,
            },
        );
        // SAFETY: Every result is returned unchanged by this library's C ABI and
        // released exactly once, including when conversion unwinds.
        unsafe { ffi::prns_app_bytes_free(bytes) };
    }
}

fn consume(bytes: PrnsAppBytes) -> String {
    let owned = OwnedResult(bytes);
    if owned.0.ptr.is_null() {
        return "{\"type\":\"bridgeFailure\",\"kind\":\"panic\",\"detail\":\"native result was empty\"}".to_owned();
    }
    // SAFETY: The C ABI returned this exact owned allocation, which remains
    // alive and immutable until `owned` is dropped after this copy.
    let bytes = unsafe { std::slice::from_raw_parts(owned.0.ptr, owned.0.len) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn dispatch(operation: &str, path: &[u8], input: &[u8]) -> PrnsAppBytes {
    // SAFETY: Every pointer below comes from a live immutable slice; the ABI
    // performs the same bounded path/JSON validation as for its Swift caller.
    unsafe {
        match operation {
            "inspectIdentity" => ffi::prns_app_inspect_identity(path.as_ptr(), path.len()),
            "previewIdentityImport" => {
                ffi::prns_app_preview_identity_import(input.as_ptr(), input.len())
            }
            "createGeneratedIdentity" => {
                ffi::prns_app_create_generated_identity(path.as_ptr(), path.len())
            }
            "createImportedIdentity" => ffi::prns_app_create_imported_identity(
                path.as_ptr(),
                path.len(),
                input.as_ptr(),
                input.len(),
            ),
            "start" => ffi::prns_app_start(path.as_ptr(), path.len(), input.as_ptr(), input.len()),
            "snapshot" => ffi::prns_app_snapshot(),
            "stop" => ffi::prns_app_stop(),
            "reset" => ffi::prns_app_reset(path.as_ptr(), path.len()),
            "initiatePairing" => ffi::prns_app_initiate_pairing(input.as_ptr(), input.len()),
            "approvePairing" => ffi::prns_app_approve_pairing(input.as_ptr(), input.len()),
            "rejectPairing" => ffi::prns_app_reject_pairing(input.as_ptr(), input.len()),
            "describeTarget" => ffi::prns_app_describe_target(input.as_ptr(), input.len()),
            "announceTarget" => ffi::prns_app_announce_target(input.as_ptr(), input.len()),
            "saveObservedDestination" => ffi::prns_app_save_observed_destination(
                path.as_ptr(),
                path.len(),
                input.as_ptr(),
                input.len(),
            ),
            "createManualContact" => ffi::prns_app_create_manual_contact(
                path.as_ptr(),
                path.len(),
                input.as_ptr(),
                input.len(),
            ),
            "setContactAlias" => ffi::prns_app_set_contact_alias(
                path.as_ptr(),
                path.len(),
                input.as_ptr(),
                input.len(),
            ),
            "setContactPinned" => ffi::prns_app_set_contact_pinned(
                path.as_ptr(),
                path.len(),
                input.as_ptr(),
                input.len(),
            ),
            "deleteContact" => {
                ffi::prns_app_delete_contact(path.as_ptr(), path.len(), input.as_ptr(), input.len())
            }
            "getContact" => {
                ffi::prns_app_get_contact(path.as_ptr(), path.len(), input.as_ptr(), input.len())
            }
            "listContacts" => ffi::prns_app_list_contacts(path.as_ptr(), path.len()),
            "listLxmfPeers" => ffi::prns_app_list_lxmf_peers(),
            "listLxmfMessages" => ffi::prns_app_list_lxmf_messages(
                path.as_ptr(),
                path.len(),
                input.as_ptr(),
                input.len(),
            ),
            "retryLxmfMessage" => ffi::prns_app_retry_lxmf_message(
                path.as_ptr(),
                path.len(),
                input.as_ptr(),
                input.len(),
            ),
            "cancelLxmfMessage" => ffi::prns_app_cancel_lxmf_message(
                path.as_ptr(),
                path.len(),
                input.as_ptr(),
                input.len(),
            ),
            "announceLxmf" => ffi::prns_app_announce_lxmf(),
            "measureLxmfText" => ffi::prns_app_measure_lxmf_text(input.as_ptr(), input.len()),
            "sendDirectText" => ffi::prns_app_send_direct_text(input.as_ptr(), input.len()),
            _ => ffi::invalid_bridge_input("unknown Android application operation"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn android_dispatch_uses_existing_bounded_abi_failures() {
        for (operation, path, input) in [
            ("notAnOperation", &b""[..], &b""[..]),
            ("inspectIdentity", &b""[..], &b""[..]),
            ("start", &b"/tmp/not-created-by-this-test"[..], &b"{"[..]),
            ("initiatePairing", &b""[..], &b"{}"[..]),
        ] {
            let output = consume(dispatch(operation, path, input));
            let json: serde_json::Value =
                serde_json::from_str(&output).expect("JSON bridge result");
            assert_eq!(json["type"], "bridgeFailure");
            assert_eq!(json["kind"], "invalidInput");
        }
    }

    #[test]
    fn android_identity_preview_preserves_the_existing_domain_result() {
        let output = consume(dispatch("previewIdentityImport", &[], &[1, 2, 3]));
        let json: serde_json::Value = serde_json::from_str(&output).expect("JSON result");
        assert_eq!(json["type"], "invalidLength");
    }
}
