mod ble;
mod bridge;
mod engine;
mod face;
mod framebuffer;

pub use face::HopspotFace;
pub use framebuffer::{ARGB_BYTES, PANEL_HEIGHT, PANEL_WIDTH};

use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use personal_hopspot_ui::{InputEvent, UiAction};

use crate::engine::{ble_bridge, usb_bridge};

#[cfg(all(target_os = "android", target_arch = "arm"))]
#[no_mangle]
pub extern "C" fn dl_iterate_phdr(
    _callback: *mut core::ffi::c_void,
    _data: *mut core::ffi::c_void,
) -> i32 {
    // Android before API 21 does not export this symbol. Rust's runtime may reference it for
    // backtrace/unwind metadata; Hopspot does not need that walk on the projector path.
    0
}

const INPUT_SHORT_PRESS: jint = 0;
const INPUT_LONG_PRESS: jint = 1;
const ACTION_NONE: jint = 0;
const ACTION_ANNOUNCE: jint = 1;

#[cfg(target_os = "android")]
fn init_logging() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("HopspotRust"),
        );
    });
}

#[cfg(not(target_os = "android"))]
fn init_logging() {}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeInit(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    init_logging();
    Box::into_raw(Box::new(HopspotFace::new())) as usize as jlong
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeFree(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    // SAFETY: `handle` was produced by `nativeInit` via `Box::into_raw` and is
    // reclaimed exactly once here; the non-zero guard above rejects a null/default
    // handle, and the JNI contract guarantees no other call aliases it afterward.
    drop(unsafe { Box::from_raw(handle as usize as *mut HopspotFace) });
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativePostInput(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    code: jint,
) -> jint {
    // SAFETY: a non-null `handle` is a live `HopspotFace` from `nativeInit` that
    // outlives this call (Kotlin frees only via `nativeFree`), and `as_mut`
    // yields `None` for a null pointer rather than dereferencing it.
    let Some(face) = (unsafe { (handle as usize as *mut HopspotFace).as_mut() }) else {
        return ACTION_NONE;
    };
    let event = match code {
        INPUT_LONG_PRESS => InputEvent::LongPress,
        INPUT_SHORT_PRESS => InputEvent::ShortPress,
        _ => InputEvent::ShortPress,
    };
    match face.post_input(event) {
        UiAction::Announce => ACTION_ANNOUNCE,
        UiAction::None => ACTION_NONE,
        UiAction::ToggleSelectedInterface => ACTION_NONE,
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeRender(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    buffer: JByteBuffer,
) {
    // SAFETY: a non-null `handle` is a live `HopspotFace` from `nativeInit` that
    // outlives this call (Kotlin frees only via `nativeFree`), and `as_mut`
    // yields `None` for a null pointer rather than dereferencing it.
    let Some(face) = (unsafe { (handle as usize as *mut HopspotFace).as_mut() }) else {
        return;
    };
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return;
    };
    if address.is_null() || capacity < ARGB_BYTES {
        return;
    }
    // SAFETY: `address`/`capacity` describe the JVM-owned direct buffer, pinned for
    // the duration of this call; we just checked it is non-null and at least
    // `ARGB_BYTES` long, and nothing else aliases it while we render into it.
    let out = unsafe { core::slice::from_raw_parts_mut(address, capacity) };
    face.render(out);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbConnected(
    _env: JNIEnv,
    _class: JClass,
    connected: jboolean,
) {
    usb_bridge().set_connected(connected != 0);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbRx(
    env: JNIEnv,
    _class: JClass,
    buffer: JByteBuffer,
    len: jint,
) {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return;
    };
    let n = (len.max(0) as usize).min(capacity);
    if address.is_null() || n == 0 {
        return;
    }
    // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call;
    // `n` is clamped to the buffer's reported capacity and we only read from it.
    let bytes = unsafe { core::slice::from_raw_parts(address, n) };
    usb_bridge().push_inbound(bytes);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbTx(
    env: JNIEnv,
    _class: JClass,
    buffer: JByteBuffer,
) -> jint {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return 0;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return 0;
    };
    if address.is_null() || capacity == 0 {
        return 0;
    }
    // SAFETY: `address`/`capacity` describe the JVM-owned direct buffer, pinned for
    // this call; nothing else aliases it while we drain outbound frames into it.
    let out = unsafe { core::slice::from_raw_parts_mut(address, capacity) };
    usb_bridge().pull_outbound(out) as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleSetPsm(
    _env: JNIEnv,
    _class: JClass,
    psm: jint,
) {
    if psm > 0 {
        ble_bridge().set_psm(psm as u16);
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleCentralReady(
    env: JNIEnv,
    _class: JClass,
    address: JByteBuffer,
) {
    let Ok(addr) = env.get_direct_buffer_address(&address) else {
        return;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&address) else {
        return;
    };
    if addr.is_null() || capacity < 6 {
        return;
    }
    // SAFETY: `addr` points at the JVM-owned direct buffer, pinned for this call; we just
    // checked its capacity holds at least the 6 address bytes we read, and we only read from it.
    let bytes = unsafe { core::slice::from_raw_parts(addr, 6) };
    let mut octets = [0u8; 6];
    octets.copy_from_slice(bytes);
    ble_bridge().central_ready(octets);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleControlIn(
    env: JNIEnv,
    _class: JClass,
    buffer: JByteBuffer,
    len: jint,
) {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return;
    };
    let n = (len.max(0) as usize).min(capacity);
    if address.is_null() || n == 0 {
        return;
    }
    // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call; `n` is
    // clamped to the buffer's reported capacity and we only read from it.
    let bytes = unsafe { core::slice::from_raw_parts(address, n) };
    ble_bridge().control_in(bytes);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleControlOut(
    env: JNIEnv,
    _class: JClass,
    buffer: JByteBuffer,
) -> jint {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return 0;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return 0;
    };
    if address.is_null() || capacity == 0 {
        return 0;
    }
    // SAFETY: `address`/`capacity` describe the JVM-owned direct buffer, pinned for this call;
    // nothing else aliases it while we drain the outgoing control PDU into it.
    let out = unsafe { core::slice::from_raw_parts_mut(address, capacity) };
    ble_bridge().control_out(out) as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleL2capIn(
    env: JNIEnv,
    _class: JClass,
    buffer: JByteBuffer,
    len: jint,
) {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return;
    };
    let n = (len.max(0) as usize).min(capacity);
    if address.is_null() || n == 0 {
        return;
    }
    // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call; `n` is
    // clamped to the buffer's reported capacity and we only read from it.
    let bytes = unsafe { core::slice::from_raw_parts(address, n) };
    ble_bridge().l2cap_in(bytes);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleL2capOut(
    env: JNIEnv,
    _class: JClass,
    buffer: JByteBuffer,
) -> jint {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return 0;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return 0;
    };
    if address.is_null() || capacity == 0 {
        return 0;
    }
    // SAFETY: `address`/`capacity` describe the JVM-owned direct buffer, pinned for this call;
    // nothing else aliases it while we drain outbound L2CAP bytes into it.
    let out = unsafe { core::slice::from_raw_parts_mut(address, capacity) };
    ble_bridge().l2cap_out(out) as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleL2capUp(
    _env: JNIEnv,
    _class: JClass,
) {
    ble_bridge().l2cap_up();
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleDisconnected(
    _env: JNIEnv,
    _class: JClass,
) {
    ble_bridge().disconnected();
}
