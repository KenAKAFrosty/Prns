use jni::objects::{JByteBuffer, JClass, JString};
use jni::sys::{jboolean, jint, jlong, jlongArray, jstring};
use jni::JNIEnv;
use personal_hopspot_core::{
    BatteryPercent, ExternalPowerState, MobileActionCode, MobileEngineFailure, MobileEngineState,
    MobileInputCode, PowerSnapshot, COALESCE_MS, MOBILE_PANEL_HEIGHT, MOBILE_PANEL_WIDTH,
    MOBILE_RGBA_BYTES,
};

use crate::engine::{
    ble_identity_hex, delivery_destination_hex, engine_state, last_failure, node_identity_hash_hex,
    node_page_destination_hex, persistence_snapshot, rpc_key_hex, runtime_health,
    sideband_join_config, wifi_aware_failure_reason, wifi_direct_failure_reason,
};
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
const PERSISTENCE_FIELD_COUNT: i32 = 7;

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
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeStartEngine(
    mut env: JNIEnv,
    _class: JClass,
    storage_dir: JString,
) -> jint {
    init_logging();
    let storage_dir = match env.get_string(&storage_dir) {
        Ok(path) => std::path::PathBuf::from(path.to_string_lossy().into_owned()),
        Err(error) => {
            log::error!("invalid Android storage directory: {error}");
            return MobileEngineFailure::StorageConfiguration.code();
        }
    };
    crate::engine::start(storage_dir)
        .err()
        .map_or(MobileEngineFailure::None.code(), |error| {
            error.failure().code()
        })
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeStopEngine(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    crate::engine::stop()
        .err()
        .map_or(MobileEngineFailure::None.code(), |error| {
            error.failure().code()
        })
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeEngineState(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    engine_state().code()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeEngineLastFailure(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    last_failure().code()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeEngineLastFailureName(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    env.new_string(last_failure().wire_name())
        .map_or(core::ptr::null_mut(), JString::into_raw)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeInputShortPressCode(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MobileInputCode::ShortPress as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeInputLongPressCode(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MobileInputCode::LongPress as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeActionNoneCode(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MobileActionCode::None.code()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeActionAnnounceCode(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MobileActionCode::Announce.code()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeActionCopySharedInstanceConfigCode(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MobileActionCode::CopySharedInstanceConfig.code()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeEngineStoppedCode(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MobileEngineState::Stopped.code()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeEngineStartingCode(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MobileEngineState::Starting.code()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeEngineRunningCode(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MobileEngineState::Running.code()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeEngineFailedCode(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MobileEngineState::Failed.code()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativePanelWidth(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MOBILE_PANEL_WIDTH as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativePanelHeight(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MOBILE_PANEL_HEIGHT as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeRgbaBytes(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    MOBILE_RGBA_BYTES as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeRenderIntervalMillis(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    COALESCE_MS as jlong
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
    externally_powered: jboolean,
) {
    // SAFETY: as in `nativePostInput`, a non-null `handle` is a live `HopspotFace` from
    // `nativeInit`; `as_mut` yields `None` for null rather than dereferencing it.
    let Some(face) = (unsafe { (handle as usize as *mut HopspotFace).as_mut() }) else {
        return;
    };
    let pct = percent.clamp(0, 100) as u8;
    let pct = BatteryPercent::saturating(pct);
    let state = PowerSnapshot::new(
        Some(pct),
        ExternalPowerState::from_presence(externally_powered != 0),
    );
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
    let values = runtime_health().map_or([0; HEALTH_FIELD_COUNT as usize], |health| {
        [
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
        ]
    });
    let Ok(array) = env.new_long_array(HEALTH_FIELD_COUNT) else {
        return core::ptr::null_mut();
    };
    if env.set_long_array_region(&array, 0, &values).is_err() {
        return core::ptr::null_mut();
    }
    array.into_raw()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativePersistenceHealth(
    env: JNIEnv,
    _class: JClass,
) -> jlongArray {
    let Some(snapshot) = persistence_snapshot() else {
        return core::ptr::null_mut();
    };
    let values = [
        jlong::from(snapshot.restore.routes),
        jlong::from(snapshot.restore.destination_identities),
        jlong::from(snapshot.restore.tunnels),
        jlong::from(snapshot.restore.ratchets),
        jlong::from(snapshot.restore.refused),
        jlong::from(snapshot.restore.dropped),
        health_long(snapshot.successful_flushes),
    ];
    let Ok(array) = env.new_long_array(PERSISTENCE_FIELD_COUNT) else {
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
    rpc_key_hex()
        .and_then(|key| env.new_string(key).ok())
        .map_or(core::ptr::null_mut(), JString::into_raw)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeSidebandJoinConfig(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    sideband_join_config()
        .and_then(|config| env.new_string(config).ok())
        .map_or(core::ptr::null_mut(), JString::into_raw)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeNodeIdentityHashHex(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    node_identity_hash_hex()
        .and_then(|identity| env.new_string(identity).ok())
        .map_or(core::ptr::null_mut(), JString::into_raw)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleIdentityHex(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    ble_identity_hex()
        .and_then(|identity| env.new_string(identity).ok())
        .map_or(core::ptr::null_mut(), JString::into_raw)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeDeliveryDestinationHex(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    delivery_destination_hex()
        .and_then(|destination| env.new_string(destination).ok())
        .map_or(core::ptr::null_mut(), JString::into_raw)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeNodePageDestinationHex(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    node_page_destination_hex()
        .and_then(|destination| env.new_string(destination).ok())
        .map_or(core::ptr::null_mut(), JString::into_raw)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareFailureReason(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    wifi_aware_failure_reason()
        .and_then(|reason| env.new_string(reason).ok())
        .map_or(core::ptr::null_mut(), JString::into_raw)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectFailureReason(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    wifi_direct_failure_reason()
        .and_then(|reason| env.new_string(reason).ok())
        .map_or(core::ptr::null_mut(), JString::into_raw)
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
    // `MOBILE_RGBA_BYTES` long, and nothing else aliases the rendered prefix.
    let out = unsafe { core::slice::from_raw_parts_mut(address, MOBILE_RGBA_BYTES) };
    let Ok(out) = <&mut [u8; MOBILE_RGBA_BYTES]>::try_from(out) else {
        return;
    };
    face.render(out);
}
