use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jboolean, jint, jstring};
use jni::JNIEnv;
use personal_rns::interfaces::usb_auto::{
    ANDROID_ACCESSORY_DESCRIPTION, ANDROID_ACCESSORY_MANUFACTURER, ANDROID_ACCESSORY_MODEL,
    ANDROID_ACCESSORY_SERIAL, ANDROID_ACCESSORY_URI, ANDROID_ACCESSORY_VERSION, WEBUSB_PRODUCT_ID,
    WEBUSB_VENDOR_ID,
};

use crate::engine::usb_bridge;

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

pub(super) fn jni_string(env: JNIEnv, value: &str) -> jstring {
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
