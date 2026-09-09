package rs.reticulum.prns.app.expo

import android.Manifest
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import expo.modules.interfaces.permissions.PermissionsResponseListener
import expo.modules.interfaces.permissions.PermissionsStatus
import expo.modules.kotlin.Promise
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

class PrnsAppModule : Module() {
  private val context: Context get() = requireNotNull(appContext.reactContext) { "React context is unavailable" }.applicationContext
  private val statusListener: (String) -> Unit = { status -> sendEvent("onAndroidRuntimeStatus", mapOf("status" to status)) }

  override fun definition() = ModuleDefinition {
    Name("PrnsApp")
    Events("onAndroidRuntimeStatus")
    OnCreate {
      PrnsAndroidRuntime.listen(statusListener)
      PrnsAndroidRuntime.setAppForeground(context, appContext.currentActivity?.hasWindowFocus() == true)
    }
    OnDestroy { PrnsAndroidRuntime.unlisten(statusListener) }
    OnActivityEntersForeground { PrnsAndroidRuntime.setAppForeground(context, true) }
    OnActivityEntersBackground { PrnsAndroidRuntime.setAppForeground(context, false) }
    OnActivityDestroys { PrnsAndroidRuntime.setAppForeground(context, false) }

    AsyncFunction("contractFingerprint") { PrnsNative.nativeContractFingerprint() }
    AsyncFunction("hostContractFingerprint") { PrnsNative.nativeHostContractFingerprint() }
    AsyncFunction("androidRuntimeStatus") {
      PrnsAndroidRuntime.refresh(context)
      PrnsAndroidRuntime.status(context)
    }
    AsyncFunction("requestBluetoothPermissions") { promise: Promise ->
      requestPermissions(PrnsAndroidRuntime.bluetoothPermissions(), promise, trackedPermission = "bluetooth")
    }
    AsyncFunction("requestBackgroundBluetoothPermission") { promise: Promise ->
      when {
        Build.VERSION.SDK_INT >= 31 -> promise.resolve(PrnsAndroidRuntime.status(context))
        !PrnsAndroidRuntime.hasBluetoothPermission(context) ->
          promise.reject("ERR_PRNS_PERMISSION", "Allow Bluetooth discovery before enabling background discovery", null)
        Build.VERSION.SDK_INT == 29 ->
          requestPermissions(arrayOf(Manifest.permission.ACCESS_BACKGROUND_LOCATION), promise)
        else -> {
          // Android 11 requires the user to grant 'Allow all the time' in Settings.
          context.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
          promise.resolve(PrnsAndroidRuntime.status(context))
        }
      }
    }

    AsyncFunction("requestConnectionNotificationPermission") { promise: Promise ->
      if (Build.VERSION.SDK_INT < 33 || context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == android.content.pm.PackageManager.PERMISSION_GRANTED) {
        promise.resolve(PrnsAndroidRuntime.status(context))
      } else {
        requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), promise, trackedPermission = "notification")
      }
    }

    AsyncFunction("start") { input: String, promise: Promise ->
      require(input.toByteArray(Charsets.UTF_8).size <= 65_536) { "Start input is too large" }
      PrnsRuntimeService.startRuntime(context, input, promise)
    }
    AsyncFunction("stop") { promise: Promise -> PrnsRuntimeService.stopRuntime(context, false, promise) }
    AsyncFunction("reset") { promise: Promise -> PrnsRuntimeService.stopRuntime(context, true, promise) }

    for (operation in listOf("inspectIdentity", "createGeneratedIdentity", "snapshot", "listContacts", "listLxmfPeers", "announceLxmf")) {
      AsyncFunction(operation) { promise: Promise -> PrnsAndroidRuntime.call(context, operation, null, promise) }
    }
    for (operation in listOf("previewIdentityImport", "createImportedIdentity")) {
      AsyncFunction(operation) { input: List<Int>, promise: Promise ->
        require(input.size <= 65_536 && input.all { it in 0..255 }) { "Identity input must contain bytes" }
        PrnsAndroidRuntime.call(context, operation, ByteArray(input.size) { input[it].toByte() }, promise)
      }
    }
    for (operation in listOf(
      "initiatePairing", "approvePairing", "rejectPairing", "describeTarget", "announceTarget",
      "saveObservedDestination", "createManualContact", "setContactAlias", "setContactPinned", "deleteContact", "getContact",
      "listLxmfMessages", "retryLxmfMessage", "cancelLxmfMessage", "measureLxmfText", "sendDirectText",
    )) {
      AsyncFunction(operation) { input: String, promise: Promise ->
        val bytes = input.toByteArray(Charsets.UTF_8)
        require(bytes.size <= 65_536) { "Native input is too large" }
        PrnsAndroidRuntime.call(context, operation, bytes, promise)
      }
    }
  }

  private fun requestPermissions(permissions: Array<String>, promise: Promise, trackedPermission: String? = null) {
    val manager = appContext.permissions
    val activity = appContext.currentActivity
    if (manager == null || activity == null) {
      promise.reject("ERR_PRNS_PERMISSION", "Open the app to grant permissions", null)
      return
    }
    val application = context
    activity.runOnUiThread {
      try {
        manager.askForPermissions(PermissionsResponseListener { responses ->
          if (trackedPermission != null) {
            val blocked = responses.values.any { it.status != PermissionsStatus.GRANTED && !it.canAskAgain }
            application.getSharedPreferences("prns-platform", Context.MODE_PRIVATE).edit()
              .putBoolean("${trackedPermission}Asked", true).putBoolean("${trackedPermission}Blocked", blocked).apply()
          }
          PrnsAndroidRuntime.refresh(application)
          promise.resolve(PrnsAndroidRuntime.status(application))
        }, *permissions)
      } catch (error: Exception) {
        promise.reject("ERR_PRNS_PERMISSION", error.message, error)
      }
    }
  }
}
