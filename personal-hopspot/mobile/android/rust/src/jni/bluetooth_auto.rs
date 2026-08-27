use crate::bluetooth_auto::AndroidBleIngressAdmission;
use crate::engine::ble_bridge;
use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;

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
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBlePeerCapacity(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    crate::bluetooth_auto::AndroidBleBackend::MAX_PEERS as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleWorkGeneration(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    ble_bridge().work_generation() as jlong
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleWaitForWork(
    _env: JNIEnv,
    _class: JClass,
    observed: jlong,
    timeout_millis: jlong,
) -> jlong {
    ble_bridge().wait_for_work(observed as u64, timeout_millis.max(0) as u64) as jlong
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleWakePumps(
    _env: JNIEnv,
    _class: JClass,
) {
    ble_bridge().wake_work();
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

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleGroupTag(
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
    if address.is_null() || capacity < 4 {
        return 0;
    }
    // SAFETY: `address`/`capacity` describe the JVM-owned direct buffer, pinned for
    // this call; nothing else aliases it while we copy the local BLE group tag into it.
    let out = unsafe { core::slice::from_raw_parts_mut(address, capacity) };
    ble_bridge().local_group_tag(out) as jint
}

fn ble_rssi(value: jint) -> Option<i8> {
    if value == 127 {
        None
    } else {
        i8::try_from(value).ok()
    }
}

pub(super) fn ble_octets(env: &JNIEnv, buffer: &JByteBuffer) -> Option<[u8; 6]> {
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
) -> jboolean {
    if let Some(octets) = ble_octets(&env, &address) {
        return ble_bridge().dial_failed(octets).into();
    }
    0
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleLinkUp(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
    address: JByteBuffer,
    rssi: jint,
    dialed: jboolean,
) -> jboolean {
    if let Some(octets) = ble_octets(&env, &address) {
        return ble_bridge()
            .link_up(conn_id as u32, octets, ble_rssi(rssi), dialed != 0)
            .into();
    }
    0
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
) -> jboolean {
    if let (Some(octets), Some(identity)) = (
        ble_octets(&env, &address),
        ble_identity_octets(&env, &peer_identity),
    ) {
        return ble_bridge()
            .columba_link_up(
                conn_id as u32,
                octets,
                ble_rssi(rssi),
                dialed != 0,
                identity,
            )
            .into();
    }
    0
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleControlIn(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
    buffer: JByteBuffer,
    len: jint,
) -> jint {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return 2;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return 2;
    };
    let n = (len.max(0) as usize).min(capacity);
    if address.is_null() || n == 0 {
        return 2;
    }
    // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call; `n` is
    // clamped to the buffer's reported capacity and we only read from it.
    let bytes = unsafe { core::slice::from_raw_parts(address, n) };
    ingress_admission_code(ble_bridge().control_in(conn_id as u32, bytes))
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
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleCommitControlOut(
    _env: JNIEnv,
    _class: JClass,
    conn_id: jint,
) -> jboolean {
    ble_bridge().commit_control_out(conn_id as u32).into()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleL2capIn(
    env: JNIEnv,
    _class: JClass,
    conn_id: jint,
    buffer: JByteBuffer,
    len: jint,
) -> jboolean {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return 0;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return 0;
    };
    let n = (len.max(0) as usize).min(capacity);
    if address.is_null() || n == 0 {
        return 0;
    }
    // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call; `n` is
    // clamped to the buffer's reported capacity and we only read from it.
    let bytes = unsafe { core::slice::from_raw_parts(address, n) };
    ble_bridge().l2cap_in(conn_id as u32, bytes).into()
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
) -> jint {
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return 2;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return 2;
    };
    let n = (len.max(0) as usize).min(capacity);
    if address.is_null() || n == 0 {
        return 2;
    }
    // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call; `n` is
    // clamped to the buffer's reported capacity and we only read from it.
    let bytes = unsafe { core::slice::from_raw_parts(address, n) };
    ingress_admission_code(ble_bridge().data_in(conn_id as u32, bytes))
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
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleCommitDataOut(
    _env: JNIEnv,
    _class: JClass,
    conn_id: jint,
) -> jboolean {
    ble_bridge().commit_data_out(conn_id as u32).into()
}

fn ingress_admission_code(admission: AndroidBleIngressAdmission) -> jint {
    match admission {
        AndroidBleIngressAdmission::Accepted => 0,
        AndroidBleIngressAdmission::Full => 1,
        AndroidBleIngressAdmission::Closed => 2,
    }
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
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeBleNextClose(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    match ble_bridge().next_close() {
        Some(conn_id) => conn_id as jint,
        None => -1,
    }
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
