use prns_ffi::ble::android as ble;
use prns_ffi::wifi_direct::android as wifi_direct;
mod bridge;
mod engine;
mod face;
mod framebuffer;
mod mdns;

pub use face::HopspotFace;
pub use framebuffer::{ARGB_BYTES, PANEL_HEIGHT, PANEL_WIDTH};

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jboolean, jint, jlong, jlongArray, jstring};
use jni::JNIEnv;
use personal_hopspot_core::{BatteryState, InputEvent, UiAction};
use personal_rns::interfaces::usb_auto::core::{
    ANDROID_ACCESSORY_DESCRIPTION, ANDROID_ACCESSORY_MANUFACTURER, ANDROID_ACCESSORY_MODEL,
    ANDROID_ACCESSORY_SERIAL, ANDROID_ACCESSORY_URI, ANDROID_ACCESSORY_VERSION, WEBUSB_PRODUCT_ID,
    WEBUSB_VENDOR_ID,
};
use personal_rns::interfaces::wifi_auto::core as wifi_core;
use personal_rns::interfaces::wifi_direct::core as wifi_direct_core;

use crate::engine::{ble_bridge, mdns_bridge, rpc_key_hex, runtime_health, usb_bridge, wd_bridge};

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
        UiAction::None
        | UiAction::OledOff
        | UiAction::Sleep
        | UiAction::Wake
        | UiAction::ToggleSelectedInterface
        | UiAction::OpenDocs
        | UiAction::OpenLoRaEditor
        | UiAction::SetLoRaProfile(_)
        | UiAction::SwapRadioMode => ACTION_NONE,
    }
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
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbAutoVendorId(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    jint::from(WEBUSB_VENDOR_ID)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbAutoProductId(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    jint::from(WEBUSB_PRODUCT_ID)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbAccessoryManufacturer(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, ANDROID_ACCESSORY_MANUFACTURER)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbAccessoryModel(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, ANDROID_ACCESSORY_MODEL)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbAccessoryDescription(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, ANDROID_ACCESSORY_DESCRIPTION)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbAccessoryVersion(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, ANDROID_ACCESSORY_VERSION)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbAccessoryUri(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, ANDROID_ACCESSORY_URI)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeUsbAccessorySerial(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, ANDROID_ACCESSORY_SERIAL)
}

fn jni_string(env: JNIEnv, value: &str) -> jstring {
    match env.new_string(value) {
        Ok(value) => value.into_raw(),
        Err(_) => core::ptr::null_mut(),
    }
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
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeRendezvousPort(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    jint::from(wifi_core::TCP_RENDEZVOUS_PORT)
}

/// Build a peer's rendezvous endpoint from the raw address bytes NsdManager resolved (4 = IPv4,
/// 16 = IPv6) and its TCP port. An NsdManager-resolved IPv6 carries no interface scope, so a
/// link-local arrives unscoped (rarely dialable); Android resolution is almost always the LAN IPv4,
/// which is the case that matters here.
fn socket_addr(env: &JNIEnv, buffer: &JByteBuffer, port: jint) -> Option<SocketAddr> {
    let address = env.get_direct_buffer_address(buffer).ok()?;
    let capacity = env.get_direct_buffer_capacity(buffer).ok()?;
    let port = u16::try_from(port).ok()?;
    if address.is_null() || port == 0 {
        return None;
    }
    let ip = if capacity >= 16 {
        // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call; we read
        // exactly the 16 bytes whose presence the reported capacity just confirmed.
        let bytes = unsafe { core::slice::from_raw_parts(address, 16) };
        let mut octets = [0u8; 16];
        octets.copy_from_slice(bytes);
        IpAddr::V6(Ipv6Addr::from(octets))
    } else if capacity >= 4 {
        // SAFETY: as above, reading exactly the 4 IPv4 bytes the reported capacity confirmed present.
        let bytes = unsafe { core::slice::from_raw_parts(address, 4) };
        IpAddr::V4(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]))
    } else {
        return None;
    };
    Some(SocketAddr::new(ip, port))
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiSighting(
    env: JNIEnv,
    _class: JClass,
    address: JByteBuffer,
    port: jint,
) {
    if let Some(addr) = socket_addr(&env, &address, port) {
        mdns_bridge().sighting(addr);
    }
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
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleDesiredState(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    ble_bridge().radio_state() as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleIdentity(
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
    if address.is_null() || capacity < 16 {
        return 0;
    }
    // SAFETY: `address`/`capacity` describe the JVM-owned direct buffer, pinned for
    // this call; nothing else aliases it while we copy the local BLE identity into it.
    let out = unsafe { core::slice::from_raw_parts_mut(address, capacity) };
    ble_bridge().local_identity(out) as jint
}

fn ble_rssi(value: jint) -> Option<i8> {
    if value == 127 {
        None
    } else {
        i8::try_from(value).ok()
    }
}

fn ble_octets(env: &JNIEnv, buffer: &JByteBuffer) -> Option<[u8; 6]> {
    let address = env.get_direct_buffer_address(buffer).ok()?;
    let capacity = env.get_direct_buffer_capacity(buffer).ok()?;
    if address.is_null() || capacity < 6 {
        return None;
    }
    // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call; we read
    // exactly the 6 bytes whose presence the reported capacity just confirmed.
    let bytes = unsafe { core::slice::from_raw_parts(address, 6) };
    let mut octets = [0u8; 6];
    octets.copy_from_slice(bytes);
    Some(octets)
}

fn ble_identity_octets(env: &JNIEnv, buffer: &JByteBuffer) -> Option<[u8; 16]> {
    let address = env.get_direct_buffer_address(buffer).ok()?;
    let capacity = env.get_direct_buffer_capacity(buffer).ok()?;
    if address.is_null() || capacity < 16 {
        return None;
    }
    // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call; we read
    // exactly the 16 bytes whose presence the reported capacity just confirmed.
    let bytes = unsafe { core::slice::from_raw_parts(address, 16) };
    let mut octets = [0u8; 16];
    octets.copy_from_slice(bytes);
    Some(octets)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleSighting(
    env: JNIEnv,
    _class: JClass,
    address: JByteBuffer,
    rssi: jint,
) {
    if let Some(octets) = ble_octets(&env, &address) {
        ble_bridge().sighting(octets, ble_rssi(rssi));
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleDialFailed(
    env: JNIEnv,
    _class: JClass,
    address: JByteBuffer,
) {
    if let Some(octets) = ble_octets(&env, &address) {
        ble_bridge().dial_failed(octets);
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleLinkUp(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
    address: JByteBuffer,
    rssi: jint,
    dialed: jboolean,
) {
    if let Some(octets) = ble_octets(&env, &address) {
        ble_bridge().link_up(conn_id as u32, octets, ble_rssi(rssi), dialed != 0);
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleColumbaLinkUp(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
    address: JByteBuffer,
    rssi: jint,
    dialed: jboolean,
    peer_identity: JByteBuffer,
) {
    if let (Some(octets), Some(identity)) = (
        ble_octets(&env, &address),
        ble_identity_octets(&env, &peer_identity),
    ) {
        ble_bridge().columba_link_up(
            conn_id as u32,
            octets,
            ble_rssi(rssi),
            dialed != 0,
            identity,
        );
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleControlIn(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
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
    ble_bridge().control_in(conn_id as u32, bytes);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleControlOut(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
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
    ble_bridge().control_out(conn_id as u32, out) as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleL2capIn(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
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
    ble_bridge().l2cap_in(conn_id as u32, bytes);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleL2capOut(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
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
    ble_bridge().l2cap_out(conn_id as u32, out) as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleDataIn(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
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
    ble_bridge().data_in(conn_id as u32, bytes);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleDataOut(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
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
    // nothing else aliases it while we copy one outbound GATT-data fragment into it.
    let out = unsafe { core::slice::from_raw_parts_mut(address, capacity) };
    ble_bridge().data_out(conn_id as u32, out) as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleL2capUp(
    _env: JNIEnv,
    _class: JClass,
    conn_id: jint,
) {
    ble_bridge().l2cap_up(conn_id as u32);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleDisconnected(
    _env: JNIEnv,
    _class: JClass,
    conn_id: jint,
) {
    ble_bridge().disconnected(conn_id as u32);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleNextDial(
    env: JNIEnv,
    _class: JClass,
    buffer: JByteBuffer,
) -> jboolean {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return 0;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return 0;
    };
    if address.is_null() || capacity < 6 {
        return 0;
    }
    // SAFETY: `address`/`capacity` describe the JVM-owned direct buffer, pinned for this call;
    // nothing else aliases it while we write the 6 dial-target bytes into it.
    let out = unsafe { core::slice::from_raw_parts_mut(address, 6) };
    jboolean::from(ble_bridge().next_dial(out))
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleNextL2capOpen(
    env: JNIEnv,
    _class: JClass,
    buffer: JByteBuffer,
) -> jboolean {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return 0;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return 0;
    };
    if address.is_null() || capacity < 6 {
        return 0;
    }
    // SAFETY: `address`/`capacity` describe the JVM-owned direct buffer, pinned for this call;
    // nothing else aliases it while we write the 4-byte conn id and 2-byte PSM into it.
    let out = unsafe { core::slice::from_raw_parts_mut(address, 6) };
    jboolean::from(ble_bridge().next_l2cap_open(out))
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectServiceType(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, wifi_direct_core::SERVICE_TYPE)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectDeviceMarker(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, wifi_direct_core::DEVICE_NAME_MARKER)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectGroupSsidPrefix(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, wifi_direct_core::GROUP_SSID_PREFIX)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectGroupPassphrase(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, wifi_direct_core::GROUP_PASSPHRASE)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectRendezvousPort(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    i32::from(wifi_direct_core::WIFI_DIRECT_RENDEZVOUS_PORT)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectSighting(
    env: JNIEnv,
    _class: JClass,
    address: JByteBuffer,
    peer_is_supplicant: jboolean,
) {
    if let Some(octets) = ble_octets(&env, &address) {
        wd_bridge().sighting(octets, peer_is_supplicant != 0);
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectPeerGone(
    env: JNIEnv,
    _class: JClass,
    address: JByteBuffer,
) {
    if let Some(octets) = ble_octets(&env, &address) {
        wd_bridge().peer_gone(octets);
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectInvitation(
    env: JNIEnv,
    _class: JClass,
    address: JByteBuffer,
) {
    if let Some(octets) = ble_octets(&env, &address) {
        wd_bridge().invitation(octets);
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectGroupFormed(
    env: JNIEnv,
    _class: JClass,
    is_owner: jboolean,
    owner_address: JByteBuffer,
) {
    if let Some(owner) = ipv4_octets(&env, &owner_address) {
        wd_bridge().group_formed(is_owner != 0, Ipv4Addr::from(owner));
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectGroupLost(
    _env: JNIEnv,
    _class: JClass,
) {
    wd_bridge().group_lost();
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectAvailability(
    _env: JNIEnv,
    _class: JClass,
    code: jint,
) {
    wd_bridge().availability(code);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectDesiredDiscovery(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    jboolean::from(wd_bridge().desired_discovery())
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectTakeHostRequest(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    jboolean::from(wd_bridge().take_host_request())
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectTakeRemoveGroup(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    jboolean::from(wd_bridge().take_remove_group())
}

fn ipv4_octets(env: &JNIEnv, buffer: &JByteBuffer) -> Option<[u8; 4]> {
    let address = env.get_direct_buffer_address(buffer).ok()?;
    let capacity = env.get_direct_buffer_capacity(buffer).ok()?;
    if address.is_null() || capacity < 4 {
        return None;
    }
    // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call; we read
    // exactly the 4 bytes whose presence the reported capacity just confirmed.
    let bytes = unsafe { core::slice::from_raw_parts(address, 4) };
    let mut octets = [0u8; 4];
    octets.copy_from_slice(bytes);
    Some(octets)
}
