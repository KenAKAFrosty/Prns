use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jint, jlong};
use jni::JNIEnv;
use personal_hopspot_core::{
    encode_mobile_discovery_groups, parse_mobile_discovery_groups, MobileDiscoveryGroupOutcome,
    MobileDiscoveryInterface, MOBILE_DISCOVERY_GROUPS_WIRE_MAX_LEN,
};

use crate::engine;

fn inventory_error(outcome: MobileDiscoveryGroupOutcome) -> jint {
    outcome.inventory_error_code() as jint
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeDiscoveryGroupsWireCapacity(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    MOBILE_DISCOVERY_GROUPS_WIRE_MAX_LEN as jlong
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeDiscoveryGroups(
    env: JNIEnv,
    _class: JClass,
    interface: jint,
    buffer: JByteBuffer,
) -> jint {
    let Ok(interface) = MobileDiscoveryInterface::decode(interface) else {
        return inventory_error(MobileDiscoveryGroupOutcome::InvalidInterface);
    };
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return inventory_error(MobileDiscoveryGroupOutcome::BufferTooShort);
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return inventory_error(MobileDiscoveryGroupOutcome::BufferTooShort);
    };
    if address.is_null() {
        return inventory_error(MobileDiscoveryGroupOutcome::BufferTooShort);
    }
    let groups = match engine::discovery_groups(interface) {
        Ok(groups) => groups,
        Err(outcome) => return inventory_error(outcome),
    };
    // SAFETY: JNI reports this direct buffer as writable for `capacity` bytes and pins it for the
    // duration of this call. The encoder never writes beyond that reported capacity.
    let out = unsafe { core::slice::from_raw_parts_mut(address, capacity) };
    match encode_mobile_discovery_groups(&groups, out) {
        Ok(len) => len as jint,
        Err(_) => inventory_error(MobileDiscoveryGroupOutcome::BufferTooShort),
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeReplaceDiscoveryGroups(
    env: JNIEnv,
    _class: JClass,
    interface: jint,
    buffer: JByteBuffer,
    len: jint,
) -> jint {
    let Ok(interface) = MobileDiscoveryInterface::decode(interface) else {
        return MobileDiscoveryGroupOutcome::InvalidInterface.code() as jint;
    };
    let Ok(address) = env.get_direct_buffer_address(&buffer) else {
        return MobileDiscoveryGroupOutcome::InvalidEncoding.code() as jint;
    };
    let Ok(capacity) = env.get_direct_buffer_capacity(&buffer) else {
        return MobileDiscoveryGroupOutcome::InvalidEncoding.code() as jint;
    };
    let Ok(len) = usize::try_from(len) else {
        return MobileDiscoveryGroupOutcome::InvalidEncoding.code() as jint;
    };
    if address.is_null() || len > capacity {
        return MobileDiscoveryGroupOutcome::InvalidEncoding.code() as jint;
    }
    // SAFETY: JNI reports this direct buffer as readable for `capacity` bytes and pins it for the
    // duration of this call. `len` was checked against that capacity.
    let encoded = unsafe { core::slice::from_raw_parts(address, len) };
    let Ok(groups) = parse_mobile_discovery_groups(encoded) else {
        return MobileDiscoveryGroupOutcome::InvalidEncoding.code() as jint;
    };
    engine::replace_discovery_groups(interface, groups).code() as jint
}
