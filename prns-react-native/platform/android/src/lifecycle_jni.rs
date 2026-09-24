//! Name-bound JNI hooks for platform Bluetooth ownership only.
use jni::objects::JClass;
use jni::sys::jboolean;
use jni::JNIEnv;

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsBluetoothLifecycle_nativePrepareBluetooth(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    prns_host_native::platform::android::prepare_bluetooth().into()
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsBluetoothLifecycle_nativeReleaseBluetooth(
    _env: JNIEnv,
    _class: JClass,
) -> jboolean {
    prns_host_native::platform::android::release_bluetooth().into()
}
