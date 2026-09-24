package rs.reticulum.prns.expo

/** Bluetooth platform ownership hooks; domain calls use generated UniFFI. */
object PrnsBluetoothLifecycle {
  init { System.loadLibrary(PrnsNativeImage.name) }

  @JvmStatic external fun nativePrepareBluetooth(): Boolean
  @JvmStatic external fun nativeReleaseBluetooth(): Boolean
}
