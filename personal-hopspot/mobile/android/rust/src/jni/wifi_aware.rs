use super::usb::jni_string;
use crate::engine::wa_bridge;
use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;
use personal_rns::interfaces::wifi_aware as wifi_aware_contract;

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareServiceName(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, wifi_aware_contract::AWARE_SERVICE_NAME)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwarePassphrase(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, wifi_aware_contract::AWARE_PASSPHRASE)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareRendezvousPort(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    i32::from(wifi_aware_contract::AWARE_RENDEZVOUS_PORT)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareLocalToken(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    wa_bridge().local_token() as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwarePeerDiscovered(
    _env: JNIEnv,
    _class: JClass,
    peer: jint,
) {
    wa_bridge().peer_discovered(peer as u32);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareNdpRequested(
    _env: JNIEnv,
    _class: JClass,
    peer: jint,
) {
    wa_bridge().ndp_requested(peer as u32);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareDataPathUp(
    env: JNIEnv,
    _class: JClass,
    peer: jint,
    is_initiator: jboolean,
    address: JByteBuffer,
    scope: jint,
) {
    if let Some(octets) = ipv6_octets(&env, &address) {
        wa_bridge().data_path_up(peer as u32, is_initiator != 0, octets, scope as u32);
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareDataPathDown(
    _env: JNIEnv,
    _class: JClass,
    peer: jint,
    is_initiator: jboolean,
) {
    wa_bridge().data_path_down(peer as u32, is_initiator != 0);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareNdpFailed(
    _env: JNIEnv,
    _class: JClass,
    peer: jint,
    is_initiator: jboolean,
) {
    wa_bridge().ndp_failed(peer as u32, is_initiator != 0);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareAvailability(
    _env: JNIEnv,
    _class: JClass,
    code: jint,
) {
    wa_bridge().availability(code);
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareDesiredDiscovery(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    jboolean::from(wa_bridge().desired_discovery())
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareTakeRequest(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    encode_ndp(wa_bridge().take_request())
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiAwareTakeAbandon(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    encode_ndp(wa_bridge().take_abandon())
}

/// Pack an NDP request/abandon for the Kotlin poller: the peer token in the low 32 bits, the role in
/// bit 32 (1 = initiator, 0 = responder), and -1 for an empty queue — a token never occupies the sign.
fn encode_ndp(
    entry: Option<(
        wifi_aware_contract::RendezvousToken,
        wifi_aware_contract::NdpRole,
    )>,
) -> jlong {
    match entry {
        Some((peer, role)) => {
            let token = jlong::from(peer.value());
            let role_bit = if matches!(role, wifi_aware_contract::NdpRole::Initiator) {
                1i64 << 32
            } else {
                0
            };
            token | role_bit
        }
        None => -1,
    }
}

fn ipv6_octets(env: &JNIEnv, buffer: &JByteBuffer) -> Option<[u8; 16]> {
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

pub(super) fn ipv4_octets(env: &JNIEnv, buffer: &JByteBuffer) -> Option<[u8; 4]> {
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
