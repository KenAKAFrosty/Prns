mod cards;
mod engine;
mod face;
mod persistence;
mod usbmux;

pub use face::HopspotFace;
pub use personal_hopspot_core::{
    MOBILE_PANEL_HEIGHT as PANEL_HEIGHT, MOBILE_PANEL_WIDTH as PANEL_WIDTH,
    MOBILE_RGBA_BYTES as RGBA_BYTES,
};

use core::ffi::c_char;
use personal_hopspot_core::{
    BatteryPercent, ExternalPowerState, MobileActionCode, MobileInputCode, PowerSnapshot,
    COALESCE_MS,
};

/// # Safety
/// `storage_directory_utf8` must be null or point to a NUL-terminated byte string that remains
/// readable for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn hopspot_start_engine(storage_directory_utf8: *const c_char) -> i32 {
    if storage_directory_utf8.is_null() {
        return engine::reject_storage_configuration().code();
    }
    // SAFETY: the caller contract requires a readable NUL-terminated string.
    let storage = unsafe { std::ffi::CStr::from_ptr(storage_directory_utf8) };
    let Ok(storage) = storage.to_str() else {
        return engine::reject_storage_configuration().code();
    };
    engine::start(std::path::Path::new(storage)).code()
}

#[no_mangle]
pub extern "C" fn hopspot_stop_engine() -> i32 {
    engine::stop().code()
}

#[no_mangle]
pub extern "C" fn hopspot_engine_state() -> i32 {
    engine::state().code()
}

#[no_mangle]
pub extern "C" fn hopspot_engine_last_failure() -> i32 {
    engine::last_failure().code()
}

#[no_mangle]
pub extern "C" fn hopspot_init() -> *mut HopspotFace {
    Box::into_raw(Box::new(HopspotFace::new()))
}

/// # Safety
/// `handle` must be a pointer returned by [`hopspot_init`] that has not already
/// been freed; it is dangling after this call.
#[no_mangle]
pub unsafe extern "C" fn hopspot_free(handle: *mut HopspotFace) {
    if handle.is_null() {
        return;
    }
    // SAFETY: the caller contract guarantees this is the unique live pointer returned by
    // `hopspot_init`; the null case was handled above and this consumes it exactly once.
    drop(unsafe { Box::from_raw(handle) });
}

/// # Safety
/// `handle` must be a live face from [`hopspot_init`] or null, and must not be
/// used concurrently with another call on the same handle.
#[no_mangle]
pub unsafe extern "C" fn hopspot_post_input(handle: *mut HopspotFace, code: i32) -> i32 {
    // SAFETY: the caller contract guarantees either null or unique access to a live HopspotFace for
    // this call; `as_mut` handles null without dereferencing it.
    let Some(face) = (unsafe { handle.as_mut() }) else {
        return MobileActionCode::None.code();
    };
    let event = match MobileInputCode::decode(code) {
        Ok(event) => event,
        Err(_) => return MobileActionCode::None.code(),
    };
    MobileActionCode::encode(face.post_input(event)).code()
}

#[no_mangle]
pub extern "C" fn hopspot_announce() {
    crate::engine::announce();
}

/// # Safety
/// `handle` must be a live face from [`hopspot_init`] or null, and must not be
/// used concurrently with another call on the same handle.
#[no_mangle]
pub unsafe extern "C" fn hopspot_set_battery(
    handle: *mut HopspotFace,
    percent: i32,
    externally_powered: bool,
) {
    // SAFETY: the caller contract guarantees either null or unique access to a live HopspotFace for
    // this call; `as_mut` handles null without dereferencing it.
    let Some(face) = (unsafe { handle.as_mut() }) else {
        return;
    };
    let pct = percent.clamp(0, 100) as u8;
    let pct = BatteryPercent::saturating(pct);
    let state = PowerSnapshot::new(
        Some(pct),
        ExternalPowerState::from_presence(externally_powered),
    );
    face.set_battery(state);
}

/// # Safety
/// `handle` must be a live face from [`hopspot_init`] or null; `ptr`/`len` must
/// describe one writable allocation that outlives the call and is not aliased.
#[no_mangle]
pub unsafe extern "C" fn hopspot_render(handle: *mut HopspotFace, ptr: *mut u8, len: usize) {
    // SAFETY: the caller contract guarantees either null or unique access to a live HopspotFace for
    // this call; `as_mut` handles null without dereferencing it.
    let Some(face) = (unsafe { handle.as_mut() }) else {
        return;
    };
    if ptr.is_null() || len < RGBA_BYTES {
        return;
    }
    // SAFETY: null and minimum size were checked above; the caller contract guarantees the
    // rendered prefix is writable, unaliased, and live for the duration of this call.
    let out = unsafe { core::slice::from_raw_parts_mut(ptr, RGBA_BYTES) };
    let Ok(out) = <&mut [u8; RGBA_BYTES]>::try_from(out) else {
        return;
    };
    face.render(out);
}

#[no_mangle]
pub extern "C" fn hopspot_panel_width() -> u32 {
    PANEL_WIDTH as u32
}

#[no_mangle]
pub extern "C" fn hopspot_panel_height() -> u32 {
    PANEL_HEIGHT as u32
}

#[no_mangle]
pub extern "C" fn hopspot_rgba_bytes() -> usize {
    RGBA_BYTES
}

#[no_mangle]
pub extern "C" fn hopspot_render_interval_millis() -> u32 {
    COALESCE_MS as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_creation_does_not_start_the_engine() {
        assert_eq!(hopspot_engine_state(), 0);
        let face = hopspot_init();
        assert!(!face.is_null());
        assert_eq!(hopspot_engine_state(), 0);
        // SAFETY: `face` is the unique live pointer returned immediately above.
        unsafe { hopspot_free(face) };
    }

    #[test]
    fn unknown_input_is_no_action_and_does_not_mutate_the_ui() {
        let face = hopspot_init();
        let mut before = vec![0; RGBA_BYTES];
        let mut after = vec![0; RGBA_BYTES];
        // SAFETY: face and both buffers are live and uniquely borrowed for each call.
        unsafe {
            hopspot_render(face, before.as_mut_ptr(), before.len());
            assert_eq!(hopspot_post_input(face, i32::MAX), 0);
            hopspot_render(face, after.as_mut_ptr(), after.len());
            hopspot_free(face);
        }
        assert_eq!(before, after);
    }
}
