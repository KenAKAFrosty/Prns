//! Name-bound JNI hooks for platform Bluetooth ownership only.
use jni::objects::JClass;
use jni::sys::jboolean;
use jni::JNIEnv;

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsNative_nativePrepareBluetooth(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    super::owner().prepare().into()
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_app_expo_PrnsNative_nativeReleaseBluetooth(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    super::owner().release().into()
}
