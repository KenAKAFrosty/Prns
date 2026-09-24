//! The single JNI adapter for the general Expo Android transport package.
//! The selected default or aggregate image calls `ensure_linked` once at initialization.
#![deny(unsafe_op_in_unsafe_fn)]

mod bluetooth_jni;
mod lifecycle_jni;
use prns_host_native::platform::android::ble_bridge;

/// Keeps name-bound JNI exports in either selected native image, even under LTO.
pub fn ensure_linked() {
    std::hint::black_box([
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleSetPsm as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleDesiredState
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBlePeerCapacity
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleWorkGeneration
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleWaitForWork
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleWakePumps
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleIdentity
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleSighting
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleDialFailed
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleLinkUp as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleColumbaLinkUp
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleControlIn
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleControlOut
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleCommitControlOut
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleL2capIn
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleL2capOut
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleDataIn as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleDataOut
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleCommitDataOut
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleL2capUp
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleDisconnected
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleNextClose
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleNextDial
            as *const (),
        bluetooth_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothNative_nativeBleNextL2capOpen
            as *const (),
        lifecycle_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothLifecycle_nativePrepareBluetooth
            as *const (),
        lifecycle_jni::Java_rs_reticulum_prns_expo_PrnsBluetoothLifecycle_nativeReleaseBluetooth
            as *const (),
    ]);
}
