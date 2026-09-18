package rs.reticulum.prns.app.expo

import android.Manifest
import android.app.NotificationManager
import android.bluetooth.BluetoothManager
import android.content.Context
import android.content.pm.PackageManager
import android.location.LocationManager
import android.os.Build
import expo.modules.kotlin.Promise
import java.io.File
import java.util.concurrent.CopyOnWriteArraySet
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicLong
import org.json.JSONObject

/** Process-owned platform state only. Protocol and durable state remain in Rust. */
internal object PrnsAndroidRuntime {
  val lifecycle = Executors.newSingleThreadExecutor { runnable -> Thread(runnable, "prns-lifecycle") }
  private val listeners = CopyOnWriteArraySet<(String) -> Unit>()
  private val revision = AtomicLong(0)
  val admission = PrnsRuntimeAdmission<PrnsRuntimeService>()
  val service: PrnsRuntimeService? get() = admission.currentOwner()
  @Volatile var appForeground = false
    private set
  @Volatile var phase = "stopped"
    private set
  @Volatile private var lastError: String? = null

  fun storagePath(context: Context): String = File(context.noBackupFilesDir, "prns/development").absolutePath

  fun bluetoothPermissions(): Array<String> = if (Build.VERSION.SDK_INT >= 31) {
    arrayOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.BLUETOOTH_ADVERTISE)
  } else {
    arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
  }

  fun hasBluetoothPermission(context: Context): Boolean = bluetoothPermissions().all {
    context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
  }

  fun bluetoothReady(context: Context): Boolean = hasBluetoothPermission(context) &&
    runCatching { context.getSystemService(BluetoothManager::class.java)?.adapter?.isEnabled == true }.getOrDefault(false) &&
    (Build.VERSION.SDK_INT >= 31 || context.getSystemService(LocationManager::class.java).isLocationEnabled)

  fun connectionNotificationStatus(context: Context): String {
    val preferences = context.getSharedPreferences("prns-platform", Context.MODE_PRIVATE)
    val manager = context.getSystemService(NotificationManager::class.java)
    val channel = manager.getNotificationChannel(PrnsRuntimeService.CHANNEL)
    val groupEnabled = channel?.group?.let { manager.getNotificationChannelGroup(it)?.isBlocked != true } ?: true
    return PrnsNotificationState.status(
      permissionRequired = Build.VERSION.SDK_INT >= 33,
      permissionGranted = Build.VERSION.SDK_INT < 33 || context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED,
      asked = preferences.getBoolean("notificationAsked", false),
      blocked = preferences.getBoolean("notificationBlocked", false),
      appEnabled = manager.areNotificationsEnabled(),
      channelEnabled = channel?.importance != NotificationManager.IMPORTANCE_NONE && groupEnabled,
    )
  }

  @Synchronized
  fun status(context: Context): String {
    val preferences = context.getSharedPreferences("prns-platform", Context.MODE_PRIVATE)
    val permission = if (hasBluetoothPermission(context)) "granted"
      else if (preferences.getBoolean("bluetoothBlocked", false)) "blocked"
      else if (preferences.getBoolean("bluetoothAsked", false)) "denied"
      else "notRequested"
    val radio = runCatching {
      val adapter = context.getSystemService(BluetoothManager::class.java)?.adapter
      when {
        adapter == null -> "unsupported"
        Build.VERSION.SDK_INT >= 31 && !hasBluetoothPermission(context) -> "unknown"
        adapter.isEnabled -> "on"
        else -> "off"
      }
    }.getOrDefault("unknown")
    return JSONObject()
      // Every capture gets its own revision, including fresh OS permission reads.
      .put("revision", revision.incrementAndGet())
      .put("bluetoothPermission", permission)
      .put("bluetoothRadio", radio)
      .put("locationServices", if (Build.VERSION.SDK_INT >= 31) "notRequired" else if (context.getSystemService(LocationManager::class.java).isLocationEnabled) "on" else "off")
      .put("backgroundDiscovery", if (Build.VERSION.SDK_INT >= 31) "notRequired" else if (context.checkSelfPermission(Manifest.permission.ACCESS_BACKGROUND_LOCATION) == PackageManager.PERMISSION_GRANTED) "granted" else "notGranted")
      .put("connectionNotification", connectionNotificationStatus(context))
      .put("service", phase)
      .put("lastError", lastError ?: JSONObject.NULL)
      .toString()
  }

  fun listen(listener: (String) -> Unit) { listeners.add(listener) }
  fun unlisten(listener: (String) -> Unit) { listeners.remove(listener) }

  fun update(context: Context, state: String? = null, error: String? = null) {
    val json = synchronized(this) {
      if (state != null) phase = state
      lastError = error
      status(context)
    }
    listeners.forEach { listener -> runCatching { listener(json) } }
  }

  fun refresh(context: Context) {
    lifecycle.execute {
      service?.refreshBluetooth()
      update(context, error = lastError)
    }
  }

  fun setAppForeground(context: Context, foreground: Boolean) {
    appForeground = foreground
    refresh(context)
  }

  fun <T> platformCall(promise: Promise, converter: rs.reticulum.prns.app.bindings.FfiConverter<T, *>, action: () -> T) {
    lifecycle.execute {
      try { promise.resolve(PrnsCodec.encode(action(), converter)) }
      catch (error: Exception) { promise.reject("ERR_PRNS_NATIVE", error.message, error) }
    }
  }

  fun prepareOutbound(promise: Promise) {
    prnsPrepareOutbound(
      lifecycle,
      refresh = { service?.refreshBluetooth() ?: true },
      prepared = { promise.resolve(null) },
      failed = { error -> promise.reject("ERR_PRNS_RECOVERY", error.message, error) },
    )
  }
}
