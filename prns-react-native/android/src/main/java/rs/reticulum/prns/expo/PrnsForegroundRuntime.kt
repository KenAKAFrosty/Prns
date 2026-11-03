package rs.reticulum.prns.expo

import android.content.Context
import expo.modules.kotlin.Promise
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

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
