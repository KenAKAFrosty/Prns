use jni::objects::{JByteBuffer, JClass, JString};
use jni::sys::{jboolean, jint, jlong, jlongArray, jstring};
use jni::JNIEnv;
use personal_hopspot_core::{
    BatteryPercent, BatteryState, MobileActionCode, MobileInputCode, MOBILE_RGBA_BYTES,
};

use crate::engine::{rpc_key_hex, runtime_health};
use crate::face::HopspotFace;

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

const HEALTH_FIELD_COUNT: i32 = 11;

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
    mut env: JNIEnv,
    _class: JClass,
    storage_dir: JString,
) -> jlong {
    init_logging();
    let storage_dir = match env.get_string(&storage_dir) {
        Ok(path) => std::path::PathBuf::from(path.to_string_lossy().into_owned()),
        Err(error) => {
            log::error!("invalid Android storage directory: {error}");
            return 0;
        }
    };
    if let Err(error) = crate::engine::configure_storage_dir(storage_dir) {
        log::error!("Android engine storage configuration failed: {error:?}");
        return 0;
    }
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
        return MobileActionCode::None.code();
    };
    let event = match MobileInputCode::decode(code) {
        Ok(event) => event,
        Err(error) => {
            log::warn!("rejected unknown mobile input code {}", error.code());
            return MobileActionCode::None.code();
        }
    };
    MobileActionCode::encode(face.post_input(event)).code()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeSetBattery(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    percent: jint,
    charging: jboolean,
) {
    // SAFETY: as in `nativePostInput`, a non-null `handle` is a live `HopspotFace` from
    // `nativeInit`; `as_mut` yields `None` for null rather than dereferencing it.
    let Some(face) = (unsafe { (handle as usize as *mut HopspotFace).as_mut() }) else {
        return;
    };
    let pct = percent.clamp(0, 100) as u8;
    let pct = BatteryPercent::saturating(pct);
    let state = if charging != 0 {
        BatteryState::Charging(pct)
    } else {
        BatteryState::Level(pct)
    };
    face.set_battery(state);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeAnnounce(
    _env: JNIEnv,
    _class: JClass,
) {
    crate::engine::announce();
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeRuntimeHealth(
    env: JNIEnv,
    _class: JClass,
) -> jlongArray {
    let health = runtime_health();
    let values = [
        health_long(health.uptime_millis),
        jlong::from(health.interface_count),
        jlong::from(health.online_interface_count),
        jlong::from(health.local_client_count),
        jlong::from(health.route_count),
        jlong::from(health.link_count),
        jlong::from(health.transported_link_count),
        health_long(health.rx_bytes),
        health_long(health.tx_bytes),
        health_long(health.rx_bps),
        health_long(health.tx_bps),
    ];
    let Ok(array) = env.new_long_array(HEALTH_FIELD_COUNT) else {
        return core::ptr::null_mut();
    };
    if env.set_long_array_region(&array, 0, &values).is_err() {
        return core::ptr::null_mut();
    }
    array.into_raw()
}

fn health_long(value: u64) -> jlong {
    value.min(jlong::MAX as u64) as jlong
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeRpcKeyHex(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    match env.new_string(rpc_key_hex()) {
        Ok(value) => value.into_raw(),
        Err(_) => core::ptr::null_mut(),
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
    if address.is_null() || capacity < MOBILE_RGBA_BYTES {
        return;
    }
    // SAFETY: `address`/`capacity` describe the JVM-owned direct buffer, pinned for
    // the duration of this call; we just checked it is non-null and at least
    // `MOBILE_RGBA_BYTES` long, and nothing else aliases it while we render into it.
    let out = unsafe { core::slice::from_raw_parts_mut(address, capacity) };
    face.render(out);
}
