package rs.reticulum.prns.app.platform

import android.Manifest
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import kotlinx.coroutines.launch
import rs.reticulum.prns.chat.app.appPrepareNew
import rs.reticulum.prns.chat.app.appPrepareExisting
import rs.reticulum.prns.chat.app.appPrepareRetainedPhoneIdentityImport
import rs.reticulum.prns.chat.app.appCancelPhoneIdentityImport
import rs.reticulum.prns.chat.app.appOwner
import rs.reticulum.prns.chat.app.appCaptureResetRoot
import rs.reticulum.prns.chat.app.AppBootstrapException
import rs.reticulum.prns.host.bindings.InterfaceKind
import rs.reticulum.prns.host.bindings.validateBluetoothDiscoveryGroups
import expo.modules.kotlin.Promise
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import rs.reticulum.prns.expo.PrnsBluetoothEnvironment
import rs.reticulum.prns.expo.PrnsForegroundService

class PrnsAppPlatformModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("PrnsAppPlatform")
    OnActivityEntersForeground { context()?.let { PrnsAppProcess.runtime.setAppForeground(it, true) } }
    OnActivityEntersBackground { context()?.let { PrnsAppProcess.runtime.setAppForeground(it, false) } }
    AsyncFunction("status") { promise: Promise -> result(promise) { statusValue(requireContext()) } }
    AsyncFunction("requestBluetoothPermission") { promise: Promise ->
      requestBluetooth { result(promise) { statusValue(requireContext()) } }
    }
    AsyncFunction("requestNotificationPermission") { promise: Promise ->
      val context = requireContext()
      if (Build.VERSION.SDK_INT < 33 || context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) {
        result(promise) { statusValue(context) }
      } else {
        val permissions = requireNotNull(appContext.permissions) { "Permission requests are unavailable" }
        requireNotNull(appContext.currentActivity) { "Open the app before requesting notifications" }
        check(PrnsAppProcess.preferences(context).edit().putBoolean("notifications-requested", true).commit()) { "Could not save notification choice" }
        permissions.askForPermissions({ result(promise) { statusValue(context) } }, Manifest.permission.POST_NOTIFICATIONS)
      }
    }
    AsyncFunction("openSettings") { destination: String ->
      val context = requireContext()
      val intent = when (destination) {
        "application" -> Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))
        "bluetooth" -> Intent(Settings.ACTION_BLUETOOTH_SETTINGS)
        "location" -> Intent(Settings.ACTION_LOCATION_SOURCE_SETTINGS)
        "notifications" -> Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
        else -> error("Unknown settings destination")
      }
      context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }
    AsyncFunction("setBluetoothEnabled") { enabled: Boolean, promise: Promise ->
      val epoch = PrnsAppProcess.permissionEpoch.get()
      val apply = {
        changeBluetooth(promise, epoch, {
          PrnsAppProcess.selection(requireContext()).setBluetoothSelected(enabled)
          PrnsAppProcess.selectBluetooth(enabled)
        }) { null }
      }
      if (enabled) requestBluetooth(apply) else apply()
    }
    AsyncFunction("bluetoothDiscoveryGroups") { promise: Promise -> result(promise) { discoveryGroups() } }
    AsyncFunction("setBluetoothDiscoveryGroups") { groups: List<String>, promise: Promise ->
      val validated = validateBluetoothDiscoveryGroups(groups)
      changeBluetooth(promise, PrnsAppProcess.permissionEpoch.get(), {
        PrnsAppProcess.selection(requireContext()).saveDiscoveryGroups(validated)
      }) { discoveryGroups() }
    }
    AsyncFunction("installationPresent") {
      PrnsAppStorage.installationPresent(requireNotNull(context()) { "Application context is unavailable" })
    }
    AsyncFunction("bluetoothSelected") {
      val context = requireNotNull(context()) { "Application context is unavailable" }
      PrnsAppProcess.selection(context).bluetoothSelected()
    }
    AsyncFunction("bluetoothAuthorization") {
      val context = requireNotNull(context()) { "Application context is unavailable" }
      when {
        PrnsBluetoothEnvironment.hasPermission(context) -> "allowed"
        PrnsAppProcess.preferences(context).getBoolean("bluetooth-requested", false) -> "denied"
        else -> "notDetermined"
      }
    }
    AsyncFunction("start") { mode: String, bluetooth: Boolean, promise: Promise ->
      prepareAndStart(PrnsAppInput.from(mode, bluetooth), null, promise)
    }
    AsyncFunction("startImported") { reviewId: String, bluetooth: Boolean, promise: Promise ->
      require(reviewId.matches(Regex("[0-9a-f]{64}"))) { "Invalid identity review" }
      prepareAndStart(PrnsAppInput(create = true, bluetooth = bluetooth), reviewId, promise)
    }
    AsyncFunction("prepareReset") { promise: Promise ->
      val context = requireContext()
      PrnsAppProcess.runtime.lifecycle.execute {
        try { appCaptureResetRoot(PrnsAppStorage.root(context, false).toString()); promise.resolve(null) }
        catch (error: Exception) { promise.reject("ERR_PRNS_RESET_REVIEW", error.message, error) }
      }
    }
    AsyncFunction("resetPreferences") { promise: Promise ->
      val context = requireContext()
      PrnsAppProcess.permissionEpoch.incrementAndGet()
      PrnsAppProcess.runtime.lifecycle.execute {
        if (!PrnsAppProcess.bluetoothChange.compareAndSet(false, true)) {
          promise.reject("ERR_PRNS_BUSY", "A Bluetooth change is still finishing", null)
          return@execute
        }
        try {
          check(!PrnsAppProcess.preparingLocal.get()) { "Local installation preparation is still finishing" }
          check(PrnsAppProcess.runtime.admission.isIdle()) { "Stop networking before clearing platform preferences" }
          val reset = try { appOwner(); false } catch (_: AppBootstrapException.NotStarted) { true }
          check(reset) { "Complete the installation reset before clearing platform preferences" }
          PrnsAppProcess.selection(context).reset()
          PrnsAppProcess.released()
          promise.resolve(null)
        } catch (error: Exception) { promise.reject("ERR_PRNS_RESET_PREFERENCES", error.message, error) }
        finally { PrnsAppProcess.bluetoothChange.set(false) }
      }
    }
    AsyncFunction("stop") { promise: Promise ->
      val context = requireNotNull(context()) { "Application context is unavailable" }
      synchronized(PrnsAppProcess.startStopGate) {
        PrnsAppProcess.permissionEpoch.incrementAndGet()
        val delegate = PrnsAppDelegate(context)
        PrnsForegroundService.stopRuntime(context, PrnsAppProcess.runtime, delegate, false, promise)
      }
    }
  }
  /** Local data is prepared before optional permission/foreground-service admission.
   * Stop may run while readiness is pending; its epoch fences every later callback.
   * Import is consumed here and is never saved or replayed by service restoration. */
  private fun prepareAndStart(input: PrnsAppInput, reviewId: String?, promise: Promise) {
    val context = requireContext()
    val epoch = PrnsAppProcess.permissionEpoch.get()
    if (!PrnsAppProcess.preparingLocal.compareAndSet(false, true)) {
      promise.reject("ERR_PRNS_BUSY", "Installation preparation is still finishing", null)
      return
    }
    val handedOff = java.util.concurrent.atomic.AtomicBoolean(false)
    val fail: (Exception) -> Unit = { error ->
      if (handedOff.compareAndSet(false, true)) {
        PrnsAppProcess.preparingLocal.set(false)
        promise.reject("ERR_PRNS_START", error.message, error)
      }
    }
    PrnsAppProcess.runtime.lifecycle.execute {
      try {
        check(PrnsAppProcess.permissionEpoch.get() == epoch) { "Start was cancelled by Stop" }
        check(!PrnsAppProcess.bluetoothChange.get()) { "A Bluetooth change is still finishing" }
        val root = PrnsAppStorage.root(context, input.create).toString()
        val owner = when {
          reviewId != null -> appPrepareRetainedPhoneIdentityImport(root, reviewId)
          input.create -> appPrepareNew(root)
          else -> appPrepareExisting(root)
        }
        PrnsAppProcess.observers.launch {
          try {
            prnsAfterLocalReady(ready = {
              owner.ready()
              // A ready imported installation owns its key; retire only the review.
              if (reviewId != null) appCancelPhoneIdentityImport(reviewId)
            }, current = { PrnsAppProcess.permissionEpoch.get() == epoch }) {
              val start = {
                if (handedOff.compareAndSet(false, true)) {
                  try {
                    synchronized(PrnsAppProcess.startStopGate) {
                      check(PrnsAppProcess.permissionEpoch.get() == epoch) { "Start was cancelled by Stop" }
                      PrnsAppProcess.select(input.bluetooth)
                      // Epoch check and ticket admission share Stop's gate.
                      PrnsForegroundService.startRuntime(context, PrnsAppService::class.java,
                        PrnsAppProcess.runtime, input.forRestoration().encode(), promise) { PrnsAppInput.decode(it) }
                    }
                  } catch (error: Exception) { promise.reject("ERR_PRNS_START", error.message, error) }
                  finally { PrnsAppProcess.preparingLocal.set(false) }
                }
              }
              if (input.bluetooth && !PrnsBluetoothEnvironment.hasPermission(context)) {
                val activity = requireNotNull(appContext.currentActivity) { "Open the app before requesting Bluetooth" }
                activity.runOnUiThread {
                  try {
                    check(PrnsAppProcess.permissionEpoch.get() == epoch) { "Start was cancelled by Stop" }
                    val permissions = requireNotNull(appContext.permissions) { "Permission requests are unavailable" }
                    check(PrnsAppProcess.preferences(context).edit().putBoolean("bluetooth-requested", true).commit()) { "Could not save permission request" }
                    permissions.askForPermissions({ start() }, *PrnsBluetoothEnvironment.permissions())
                  } catch (error: Exception) { fail(error) }
                }
              } else { start() }
            }
          } catch (error: Exception) { fail(error) }
        }
      } catch (error: Exception) { fail(error) }
    }
  }

  private fun requireContext(): Context = requireNotNull(context()) { "Application context is unavailable" }

  private fun requestBluetooth(completion: () -> Unit) {
    val context = requireContext()
    if (PrnsBluetoothEnvironment.hasPermission(context)) { completion(); return }
    val permissions = requireNotNull(appContext.permissions) { "Permission requests are unavailable" }
    requireNotNull(appContext.currentActivity) { "Open the app before requesting Bluetooth" }
    check(PrnsAppProcess.preferences(context).edit().putBoolean("bluetooth-requested", true).commit()) { "Could not save permission choice" }
    permissions.askForPermissions({ completion() }, *PrnsBluetoothEnvironment.permissions())
  }

  private fun result(promise: Promise, operation: suspend () -> Any?) {
    PrnsAppProcess.observers.launch {
      try { promise.resolve(operation()) }
      catch (error: Exception) { promise.reject("ERR_PRNS_PLATFORM", error.message, error) }
    }
  }

  private suspend fun attached(): Boolean = runCatching {
    appOwner().host().snapshot().interfaces.any { it.kind == InterfaceKind.AUTOMATIC_BLUETOOTH_LE }
  }.getOrDefault(false)

  private suspend fun discoveryGroups(): Map<String, Any?> = mapOf(
    "saved" to PrnsAppProcess.selection(requireContext()).discoveryGroups(),
    "applied" to PrnsAppProcess.appliedGroups.takeIf { attached() },
  )

  private suspend fun statusValue(context: Context): Map<String, Any> {
    val preferences = PrnsAppProcess.preferences(context)
    val manager = context.getSystemService(NotificationManager::class.java)
    val required = Build.VERSION.SDK_INT >= 33
    val granted = !required || context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
    val asked = preferences.getBoolean("notifications-requested", false)
    val blocked = required && !granted && asked && appContext.currentActivity?.shouldShowRequestPermissionRationale(Manifest.permission.POST_NOTIFICATIONS) == false
    return mapOf(
      "bluetooth" to mapOf(
        "authorization" to when {
          PrnsBluetoothEnvironment.hasPermission(context) -> "allowed"
          preferences.getBoolean("bluetooth-requested", false) -> "denied"
          else -> "notDetermined"
        },
        "availability" to PrnsBluetoothEnvironment.availability(context),
        "selected" to PrnsAppProcess.selection(context).bluetoothSelected(),
        "attached" to attached(),
      ),
      "notifications" to PrnsNotificationState.status(required, granted, asked, blocked,
        manager.areNotificationsEnabled(), manager.getNotificationChannel("prns-connection")?.importance != NotificationManager.IMPORTANCE_NONE),
    )
  }

  private fun changeBluetooth(promise: Promise, epoch: Long, save: () -> Unit, value: suspend () -> Any?) {
    if (!PrnsAppProcess.bluetoothChange.compareAndSet(false, true)) {
      promise.reject("ERR_PRNS_BUSY", "A Bluetooth change is still finishing", null); return
    }
    try {
      check(PrnsAppProcess.permissionEpoch.get() == epoch) { "Bluetooth change was cancelled by Stop" }
      val persist = {
        check(!PrnsAppProcess.preparingLocal.get()) { "Installation preparation is still finishing" }
        check(PrnsAppProcess.permissionEpoch.get() == epoch) { "Bluetooth change was cancelled by Stop" }
        save()
      }
      val complete: (Result<Unit>) -> Unit = { outcome ->
        PrnsAppProcess.observers.launch {
          try {
            outcome.getOrThrow()
            check(PrnsAppProcess.permissionEpoch.get() == epoch) { "Bluetooth change was cancelled by Stop" }
            promise.resolve(value())
          } catch (error: Exception) { promise.reject("ERR_PRNS_BLUETOOTH", error.message, error) }
          finally { PrnsAppProcess.bluetoothChange.set(false) }
        }
      }
      val service = PrnsAppProcess.runtime.service
      if (service == null) { persist(); complete(Result.success(Unit)) }
      else service.reconfigureBluetooth(admitted = persist, completion = complete)
    } catch (error: Exception) {
      PrnsAppProcess.bluetoothChange.set(false)
      promise.reject("ERR_PRNS_BLUETOOTH", error.message, error)
    }
  }

  private fun context() = appContext.reactContext?.applicationContext
}
