mod cards;
mod engine;
mod face;
mod usbmux;

pub use face::HopspotFace;
pub use personal_hopspot_core::{
    MOBILE_PANEL_HEIGHT as PANEL_HEIGHT, MOBILE_PANEL_WIDTH as PANEL_WIDTH,
    MOBILE_RGBA_BYTES as RGBA_BYTES,
};

use personal_hopspot_core::{BatteryPercent, BatteryState, MobileActionCode, MobileInputCode};

#[no_mangle]
pub extern "C" fn hopspot_start_engine() {
    engine::start();
}

#[no_mangle]
pub extern "C" fn hopspot_init() -> *mut HopspotFace {
    engine::start();
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
    charging: bool,
) {
    // SAFETY: the caller contract guarantees either null or unique access to a live HopspotFace for
    // this call; `as_mut` handles null without dereferencing it.
    let Some(face) = (unsafe { handle.as_mut() }) else {
        return;
    };
    let pct = percent.clamp(0, 100) as u8;
    let pct = BatteryPercent::saturating(pct);
    let state = if charging {
        BatteryState::Charging(pct)
    } else {
        BatteryState::Level(pct)
    };
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
    // SAFETY: null and minimum size were checked above; the caller contract guarantees `ptr..len`
    // is one writable, unaliased allocation that remains live for the duration of this call.
    let out = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
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
