package rs.reticulum.prns.expo

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.pm.ServiceInfo
import expo.modules.kotlin.Promise
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

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

/** One native service owner survives React context destruction/replacement. */
class PrnsForegroundRuntime(
  private val onUpdate: (Context, String?, String?) -> Unit,
  val bluetoothReady: (Context) -> Boolean = PrnsBluetoothEnvironment::ready,
) {
  val lifecycle = Executors.newSingleThreadExecutor { runnable -> Thread(runnable, "prns-lifecycle") }
  val admission = PrnsRuntimeAdmission<PrnsForegroundService>()
  val service: PrnsForegroundService? get() = admission.currentOwner()
  internal val requests = ConcurrentHashMap<Long, Promise>()
  @Volatile var appForeground = false
    private set

  fun update(context: Context, state: String? = null, error: String? = null) = onUpdate(context, state, error)

  fun refresh(context: Context) {
    lifecycle.execute {
      service?.refreshBluetooth()
      update(context)
    }
  }

  fun setAppForeground(context: Context, foreground: Boolean) {
    appForeground = foreground
    refresh(context)
  }
}
