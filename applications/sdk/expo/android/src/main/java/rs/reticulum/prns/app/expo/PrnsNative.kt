package rs.reticulum.prns.app.expo

/** Bluetooth platform ownership hooks; domain calls use generated UniFFI. */
internal object PrnsNative {
  init { System.loadLibrary("prns_app") }

  @JvmStatic external fun nativePrepareBluetooth(): Boolean
  @JvmStatic external fun nativeReleaseBluetooth(): Boolean
}
