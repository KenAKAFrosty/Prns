package rs.reticulum.prns.app.expo

/** Name-bound JNI adapter to the existing application-owned Rust runtime. */
internal object PrnsNative {
  init { System.loadLibrary("prns_app") }

  @JvmStatic external fun nativeContractFingerprint(): String
  @JvmStatic external fun nativeHostContractFingerprint(): String
  @JvmStatic external fun nativeCall(operation: String, storagePath: String?, input: ByteArray?): String
  @JvmStatic external fun nativePrepareBluetooth(): Boolean
  @JvmStatic external fun nativeReleaseBluetooth(): Boolean
}
