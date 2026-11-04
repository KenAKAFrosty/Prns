package rs.reticulum.prns.app.platform

import android.content.Context
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.sync.Mutex
import rs.reticulum.prns.expo.PrnsForegroundRuntime
import java.util.concurrent.atomic.AtomicLong

/** Platform selection and pending permission requests, never domain authority. */
internal object PrnsAppProcess {
  val runtime = PrnsForegroundRuntime(onUpdate = { _, _, _ -> })
  val observers = CoroutineScope(SupervisorJob() + Dispatchers.Default)
  val transportAttachment = Mutex()
  val startStopGate = Any()
  val permissionEpoch = AtomicLong(0)
  val preparingLocal = java.util.concurrent.atomic.AtomicBoolean(false)
  val bluetoothChange = java.util.concurrent.atomic.AtomicBoolean(false)
  @Volatile var appliedGroups: List<String>? = null
  private var selected: Boolean? = null

  @Synchronized fun select(bluetooth: Boolean) {
    require(selected == null || selected == bluetooth) { "Use Bluetooth controls to change the running transport" }
    selected = bluetooth
  }
  @Synchronized fun selectBluetooth(bluetooth: Boolean) { selected = bluetooth }
  @Synchronized fun bluetooth(context: Context): Boolean = selected ?: saved(context)?.bluetooth ?: false
  @Synchronized fun released() { selected = null; appliedGroups = null }
  fun preferences(context: Context) = context.getSharedPreferences("prns-app-startup", Context.MODE_PRIVATE)
  fun selection(context: Context) = PrnsAppPreferences(preferences(context))
  fun saved(context: Context): PrnsAppInput? = selection(context).savedInput()
}
