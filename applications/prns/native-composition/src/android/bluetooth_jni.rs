// JNI adaptation of the upstream Android platform bridge. Protocol and queue ownership
// remain in prns-ffi; the app supplies only its reserved lifecycle session.
use super::ble_bridge;
use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use prns_ffi::bluetooth_auto::android::AndroidBleIngressAdmission;

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleSetPsm(
    _env: JNIEnv,
    _class: JClass,
    psm: jint,
) {
    if let Ok(psm) = u16::try_from(psm) {
        if psm != 0 {
            ble_bridge().set_psm(psm);
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleDesiredState(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    ble_bridge().radio_state() as jint
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBlePeerCapacity(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    prns_ffi::bluetooth_auto::android::AndroidBleBackend::MAX_PEERS as jint
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleWorkGeneration(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    ble_bridge().work_generation() as jlong
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleWaitForWork(
    _env: JNIEnv,
    _class: JClass,
    observed: jlong,
    timeout_millis: jlong,
) -> jlong {
    ble_bridge().wait_for_work(observed as u64, bounded_work_wait_millis(timeout_millis)) as jlong
}

fn bounded_work_wait_millis(requested: jlong) -> u64 {
    // Kotlin uses zero for upstream's indefinite idle wait. Keep the app's one-second
    // shutdown fallback without converting idle pumps into one-millisecond polling.
    // The shared generation signal still wakes all pumps immediately for work or stop.
    if requested == 0 {
        1_000
    } else {
        requested.clamp(1, 1_000) as u64
    }
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleWakePumps(
    _env: JNIEnv,
    _class: JClass,
) {
    ble_bridge().wake_work();
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleIdentity(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleSighting(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleDialFailed(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleLinkUp(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleColumbaLinkUp(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleControlIn(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleControlOut(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleCommitControlOut(
    _env: JNIEnv,
    _class: JClass,
    conn_id: jint,
) -> jboolean {
    ble_bridge().commit_control_out(conn_id as u32).into()
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleL2capIn(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleL2capOut(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleDataIn(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleDataOut(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleCommitDataOut(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleL2capUp(
    _env: JNIEnv,
    _class: JClass,
    conn_id: jint,
) {
    ble_bridge().l2cap_up(conn_id as u32);
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleDisconnected(
    _env: JNIEnv,
    _class: JClass,
    conn_id: jint,
) {
    ble_bridge().disconnected(conn_id as u32);
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleNextClose(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    match ble_bridge().next_close() {
        Some(conn_id) => conn_id as jint,
        None => -1,
    }
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleNextDial(
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
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsBluetoothNative_nativeBleNextL2capOpen(
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

#[cfg(test)]
mod tests {
    use super::bounded_work_wait_millis;
    use prns_ffi::bluetooth_auto::android::AndroidBleBridge;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn idle_wait_keeps_the_bounded_fallback_without_millisecond_polling() {
        for (requested, expected) in [
            (-1, 1),
            (0, 1_000),
            (1, 1),
            (4, 4),
            (1_000, 1_000),
            (1_001, 1_000),
            (i64::MAX, 1_000),
        ] {
            assert_eq!(bounded_work_wait_millis(requested), expected);
        }
    }

    #[test]
    fn idle_wait_sleeps_until_work_or_shutdown_wakes_the_shared_generation() {
        let bridge = AndroidBleBridge::new();
        let worker_bridge = bridge.clone();
        let generation = bridge.work_generation();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            entered_tx.send(()).unwrap();
            let next = worker_bridge.wait_for_work(generation, bounded_work_wait_millis(0));
            done_tx.send(next).unwrap();
        });

        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            done_rx.recv_timeout(Duration::from_millis(25)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "an idle pump must not return every millisecond",
        );
        // Kotlin stop calls this same wake before joining its workers; it must not wait
        // for the one-second fallback. A wake before wait also cannot be lost.
        bridge.wake_work();
        let next = done_rx.recv_timeout(Duration::from_millis(250)).unwrap();
        assert_ne!(next, generation);
        worker.join().unwrap();
        assert_eq!(
            bridge.wait_for_work(generation, bounded_work_wait_millis(0)),
            next,
        );
    }
}
