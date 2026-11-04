package rs.reticulum.prns.expo

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.pm.ServiceInfo

/** The native result stays opaque to lifecycle mechanics; codecs belong to the caller. */
data class PrnsRuntimeStart(val response: Any?, val running: Boolean, val error: String? = null)
data class PrnsRuntimeStop(val response: Any?, val stopped: Boolean)

/**
 * Native startup configuration, independent of React and its lifetime.
 * A service subclass constructs this delegate on process restoration. Keep
 * credentials in native storage, never in an Intent or persisted input bytes.
 */
interface PrnsRuntimeDelegate {
  val bluetoothEnabled: Boolean get() = true
  val notificationId: Int
  val foregroundServiceType: Int get() = ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
  fun notification(service: Service, stop: PendingIntent): Notification
  fun savedInput(): ByteArray?
  fun saveInput(input: ByteArray)
  fun clearInput()
  fun start(input: ByteArray): PrnsRuntimeStart
  /** stopped=true acknowledges complete native drain, including on reset. */
  fun stop(reset: Boolean): PrnsRuntimeStop
}

/**
 * Opt in when readiness/drain is asynchronous. Entry points must return promptly;
 * Stop must fence an outstanding Start and repeated Stop must observe its drain.
 * The SDK delivers exactly one completion on its existing lifecycle executor.
 *
 * A running Start with an error retains the ready native owner but rejects the
 * request as a transport failure. The SDK saves reopen input and does not stop
 * that owner. Existing synchronous delegates keep their original result behavior.
 */
interface PrnsAsyncRuntimeDelegate : PrnsRuntimeDelegate {
  fun start(input: ByteArray, completion: (Result<PrnsRuntimeStart>) -> Unit)
  fun stop(reset: Boolean, completion: (Result<PrnsRuntimeStop>) -> Unit)

  override fun start(input: ByteArray): PrnsRuntimeStart =
    throw UnsupportedOperationException("Use the asynchronous lifecycle entrypoint")

  override fun stop(reset: Boolean): PrnsRuntimeStop =
    throw UnsupportedOperationException("Use the asynchronous lifecycle entrypoint")
}

/** Opt in to replace only the Bluetooth attachment, retaining every other host service. */
interface PrnsBluetoothRuntimeDelegate : PrnsRuntimeDelegate {
  fun detachBluetooth(completion: (Result<Unit>) -> Unit)
  fun attachBluetooth(completion: (Result<Unit>) -> Unit)
}
