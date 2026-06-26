mod cards;
mod engine;
mod face;
mod framebuffer;

pub use face::HopspotFace;
pub use framebuffer::{PANEL_HEIGHT, PANEL_WIDTH, RGBA_BYTES};

use personal_hopspot_ui::{BatteryState, InputEvent, UiAction};

const INPUT_SHORT_PRESS: i32 = 0;
const INPUT_LONG_PRESS: i32 = 1;
const ACTION_NONE: i32 = 0;
const ACTION_ANNOUNCE: i32 = 1;

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
    drop(Box::from_raw(handle));
}

/// # Safety
/// `handle` must be a live face from [`hopspot_init`] or null, and must not be
/// used concurrently with another call on the same handle.
#[no_mangle]
pub unsafe extern "C" fn hopspot_post_input(handle: *mut HopspotFace, code: i32) -> i32 {
    let Some(face) = handle.as_mut() else {
        return ACTION_NONE;
    };
    let event = match code {
        INPUT_LONG_PRESS => InputEvent::LongPress,
        INPUT_SHORT_PRESS => InputEvent::ShortPress,
        _ => InputEvent::ShortPress,
    };
    match face.post_input(event) {
        UiAction::Announce => ACTION_ANNOUNCE,
        UiAction::None
        | UiAction::ToggleSelectedInterface
        | UiAction::OpenLoRaEditor
        | UiAction::SetLoRaProfile(_)
        | UiAction::SwapRadioMode => ACTION_NONE,
    }
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
    let Some(face) = handle.as_mut() else {
        return;
    };
    let pct = percent.clamp(0, 100) as u8;
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
    let Some(face) = handle.as_mut() else {
        return;
    };
    if ptr.is_null() || len < RGBA_BYTES {
        return;
    }
    let out = core::slice::from_raw_parts_mut(ptr, len);
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
