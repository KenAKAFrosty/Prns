use std::net::Ipv4Addr;

use super::bluetooth::ble_octets;
use super::usb::jni_string;
use super::wifi_aware::ipv4_octets;
use crate::engine::wd_bridge;
use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jboolean, jint, jstring};
use jni::JNIEnv;
use personal_rns::interfaces::wifi_direct::core as wifi_direct_core;

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
    peer_name_hash: jint,
) {
    if let Some(octets) = ble_octets(&env, &address) {
        wd_bridge().sighting(octets, peer_is_supplicant != 0, peer_name_hash);
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDirectSetLocalNameHash(
    _env: JNIEnv,
    _class: JClass,
    hash: jint,
) {
    wd_bridge().set_local_name_hash(hash);
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
