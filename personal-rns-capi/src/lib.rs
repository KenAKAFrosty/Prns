//! C ABI bindings for `personal-rns`.
//!
//! This crate keeps the native ABI intentionally small and explicit. Consumers
//! own opaque runtime handles, fallible functions return status codes, and all
//! scalar results flow through out-parameters so C, C++, Go, Zig, and similar
//! hosts can bind the same surface without understanding Rust layouts.

use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;
use std::time::Instant;

use personal_rns::engine::{
    EngineCycleEntropy, EngineCycleEntropySeed, EngineState, InstantMillis,
    ENGINE_CYCLE_ENTROPY_LEN,
};
use personal_rns::routing::storage::FixedCapacity;

const PRNS_ABI_VERSION: u32 = 1;
type SdkEngineStorage = FixedCapacity<64, 64, 4096, 4, 512, 64>;

/// Successful C ABI call.
pub const PRNS_STATUS_OK: u32 = 0;
/// A required pointer argument was null.
pub const PRNS_STATUS_NULL_POINTER: u32 = 1;
/// The host OS did not provide cycle entropy.
pub const PRNS_STATUS_ENTROPY_UNAVAILABLE: u32 = 2;
/// A prior panic poisoned the runtime mutex.
pub const PRNS_STATUS_RUNTIME_POISONED: u32 = 3;
/// An unexpected Rust panic was caught before crossing the C ABI.
pub const PRNS_STATUS_PANIC: u32 = 4;

type PrnsStatus = u32;

struct SdkEngineSubstrate {
    base: Instant,
}

impl SdkEngineSubstrate {
    fn now_millis(&self) -> InstantMillis {
        InstantMillis(self.base.elapsed().as_millis() as u64)
    }

    fn cycle_entropy(&self) -> Result<EngineCycleEntropy, PrnsStatus> {
        let mut seed = [0u8; ENGINE_CYCLE_ENTROPY_LEN];
        getrandom::getrandom(&mut seed).map_err(|_| PRNS_STATUS_ENTROPY_UNAVAILABLE)?;
        Ok(EngineCycleEntropy::from_seed(EngineCycleEntropySeed::new(
            seed,
        )))
    }
}

struct RuntimeInner {
    state: EngineState<SdkEngineStorage>,
    substrate: SdkEngineSubstrate,
}

/// Opaque runtime handle for C ABI consumers.
#[repr(C)]
pub struct PrnsRuntime {
    inner: Mutex<RuntimeInner>,
}

impl PrnsRuntime {
    fn new() -> Self {
        Self {
            inner: Mutex::new(RuntimeInner {
                state: EngineState::<SdkEngineStorage>::default(),
                substrate: SdkEngineSubstrate {
                    base: Instant::now(),
                },
            }),
        }
    }

    fn tick(&self) -> Result<u64, PrnsStatus> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| PRNS_STATUS_RUNTIME_POISONED)?;
        let RuntimeInner { state, substrate } = &mut *inner;
        let now = substrate.now_millis();
        let entropy = substrate.cycle_entropy()?;
        let output = state.tick(now, entropy.jitter);
        Ok(output.egress_directive_count() as u64)
    }

    fn tick_count(&self) -> Result<u64, PrnsStatus> {
        self.inner
            .lock()
            .map_err(|_| PRNS_STATUS_RUNTIME_POISONED)
            .map(|inner| inner.state.tick_count())
    }
}

fn ffi_status(operation: impl FnOnce() -> Result<(), PrnsStatus>) -> PrnsStatus {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(())) => PRNS_STATUS_OK,
        Ok(Err(status)) => status,
        Err(_) => PRNS_STATUS_PANIC,
    }
}

fn write_out<T>(out: *mut T, value: T) -> Result<(), PrnsStatus> {
    if out.is_null() {
        return Err(PRNS_STATUS_NULL_POINTER);
    }

    // SAFETY: `out` was checked for null above. The C ABI contract requires
    // callers to pass a valid, writable pointer for out-parameters.
    unsafe {
        *out = value;
    }
    Ok(())
}

fn static_cstr(bytes: &'static [u8]) -> *const c_char {
    bytes.as_ptr().cast()
}

/// Return the C ABI version exposed by this library.
#[no_mangle]
pub extern "C" fn prns_abi_version() -> u32 {
    PRNS_ABI_VERSION
}

/// Return the personal-rns C API package version as a process-static C string.
#[no_mangle]
pub extern "C" fn prns_version() -> *const c_char {
    static_cstr(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes())
}

/// Return a process-static message for a C ABI status code.
#[no_mangle]
pub extern "C" fn prns_status_message(status: PrnsStatus) -> *const c_char {
    match status {
        PRNS_STATUS_OK => static_cstr(b"ok\0"),
        PRNS_STATUS_NULL_POINTER => static_cstr(b"null pointer\0"),
        PRNS_STATUS_ENTROPY_UNAVAILABLE => static_cstr(b"entropy unavailable\0"),
        PRNS_STATUS_RUNTIME_POISONED => static_cstr(b"runtime poisoned\0"),
        PRNS_STATUS_PANIC => static_cstr(b"panic\0"),
        _ => static_cstr(b"unknown status\0"),
    }
}

/// Allocate a new runtime handle.
///
/// # Safety
///
/// `out_runtime` must be null or valid to write one runtime pointer. Passing a
/// null pointer returns `PRNS_STATUS_NULL_POINTER`; passing any other invalid
/// pointer is undefined behavior.
#[no_mangle]
pub unsafe extern "C" fn prns_runtime_new(out_runtime: *mut *mut PrnsRuntime) -> PrnsStatus {
    ffi_status(|| {
        let runtime = Box::new(PrnsRuntime::new());
        write_out(out_runtime, Box::into_raw(runtime))
    })
}

/// Free a runtime handle allocated by `prns_runtime_new`.
///
/// # Safety
///
/// `runtime` must be null or a pointer returned by `prns_runtime_new` that has
/// not already been freed.
#[no_mangle]
pub unsafe extern "C" fn prns_runtime_free(runtime: *mut PrnsRuntime) {
    if runtime.is_null() {
        return;
    }

    // SAFETY: `runtime` was checked for null above. The C ABI contract requires
    // the pointer to come from `prns_runtime_new` and to be freed at most once.
    unsafe {
        drop(Box::from_raw(runtime));
    }
}

/// Drive one runtime tick and write the emitted directive count.
///
/// # Safety
///
/// `runtime` must be a valid pointer returned by `prns_runtime_new`, and
/// `out_emitted` must be null or valid to write one `u64`. Passing a null
/// pointer returns `PRNS_STATUS_NULL_POINTER`; passing any other invalid pointer
/// is undefined behavior.
#[no_mangle]
pub unsafe extern "C" fn prns_runtime_tick(
    runtime: *mut PrnsRuntime,
    out_emitted: *mut u64,
) -> PrnsStatus {
    ffi_status(|| {
        if runtime.is_null() {
            return Err(PRNS_STATUS_NULL_POINTER);
        }

        // SAFETY: `runtime` was checked for null above. The C ABI contract
        // requires the pointer to remain valid for the duration of this call.
        let runtime = unsafe { &*runtime };
        write_out(out_emitted, runtime.tick()?)
    })
}

/// Write the total ticks advanced since runtime construction.
///
/// # Safety
///
/// `runtime` must be a valid pointer returned by `prns_runtime_new`, and
/// `out_tick_count` must be null or valid to write one `u64`. Passing a null
/// pointer returns `PRNS_STATUS_NULL_POINTER`; passing any other invalid pointer
/// is undefined behavior.
#[no_mangle]
pub unsafe extern "C" fn prns_runtime_tick_count(
    runtime: *mut PrnsRuntime,
    out_tick_count: *mut u64,
) -> PrnsStatus {
    ffi_status(|| {
        if runtime.is_null() {
            return Err(PRNS_STATUS_NULL_POINTER);
        }

        // SAFETY: `runtime` was checked for null above. The C ABI contract
        // requires the pointer to remain valid for the duration of this call.
        let runtime = unsafe { &*runtime };
        write_out(out_tick_count, runtime.tick_count()?)
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;
    use std::ptr;

    use super::*;

    #[test]
    fn exposes_version_and_status_messages() {
        assert_eq!(prns_abi_version(), PRNS_ABI_VERSION);

        // SAFETY: `prns_version` returns a process-static, null-terminated
        // string by contract.
        let version = unsafe { CStr::from_ptr(prns_version()) };
        assert_eq!(version.to_str().unwrap(), env!("CARGO_PKG_VERSION"));

        // SAFETY: `prns_status_message` returns process-static,
        // null-terminated strings by contract.
        let message = unsafe { CStr::from_ptr(prns_status_message(PRNS_STATUS_NULL_POINTER)) };
        assert_eq!(message.to_str().unwrap(), "null pointer");
    }

    #[test]
    fn runtime_handle_ticks_and_frees() {
        let mut runtime: *mut PrnsRuntime = ptr::null_mut();

        // SAFETY: `runtime` is a valid writable out-parameter.
        let status = unsafe { prns_runtime_new(&mut runtime) };
        assert_eq!(status, PRNS_STATUS_OK);
        assert!(!runtime.is_null());

        let mut tick_count = u64::MAX;
        // SAFETY: `runtime` came from `prns_runtime_new`, and `tick_count` is a
        // valid writable out-parameter.
        let status = unsafe { prns_runtime_tick_count(runtime, &mut tick_count) };
        assert_eq!(status, PRNS_STATUS_OK);
        assert_eq!(tick_count, 0);

        let mut emitted = u64::MAX;
        // SAFETY: `runtime` came from `prns_runtime_new`, and `emitted` is a
        // valid writable out-parameter.
        let status = unsafe { prns_runtime_tick(runtime, &mut emitted) };
        assert_eq!(status, PRNS_STATUS_OK);
        assert_eq!(emitted, 0);

        // SAFETY: `runtime` came from `prns_runtime_new`, and `tick_count` is a
        // valid writable out-parameter.
        let status = unsafe { prns_runtime_tick_count(runtime, &mut tick_count) };
        assert_eq!(status, PRNS_STATUS_OK);
        assert_eq!(tick_count, 1);

        // SAFETY: `runtime` came from `prns_runtime_new` and has not been freed.
        unsafe { prns_runtime_free(runtime) };
    }

    #[test]
    fn null_pointers_report_status() {
        let mut emitted = 0;
        // SAFETY: Passing null is allowed and returns a status.
        let status = unsafe { prns_runtime_tick(ptr::null_mut(), &mut emitted) };
        assert_eq!(status, PRNS_STATUS_NULL_POINTER);

        let mut runtime: *mut PrnsRuntime = ptr::null_mut();
        // SAFETY: Passing null is allowed and returns a status.
        let status = unsafe { prns_runtime_new(ptr::null_mut()) };
        assert_eq!(status, PRNS_STATUS_NULL_POINTER);
        assert!(runtime.is_null());

        // SAFETY: `runtime` is a valid writable out-parameter.
        let status = unsafe { prns_runtime_new(&mut runtime) };
        assert_eq!(status, PRNS_STATUS_OK);
        // SAFETY: `runtime` came from `prns_runtime_new`; passing a null
        // out-parameter is allowed and returns a status.
        let status = unsafe { prns_runtime_tick(runtime, ptr::null_mut()) };
        assert_eq!(status, PRNS_STATUS_NULL_POINTER);
        // SAFETY: `runtime` came from `prns_runtime_new` and has not been freed.
        unsafe { prns_runtime_free(runtime) };
        // SAFETY: Freeing null is allowed as a no-op.
        unsafe { prns_runtime_free(ptr::null_mut()) };
    }
}
