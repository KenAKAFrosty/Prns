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
import rs.reticulum.prns.app.bindings.*

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

    AsyncFunction("prepareStorage") { promise: Promise ->
      val path = PrnsAndroidRuntime.storagePath(context)
      PrnsAndroidRuntime.platformCall(promise, FfiConverterTypeNativeStoragePreparationOutcome) { nativePrepareStorage(path) }
    }
    AsyncFunction("inspectIdentity") { promise: Promise ->
      val path = PrnsAndroidRuntime.storagePath(context)
      PrnsAndroidRuntime.platformCall(promise, FfiConverterTypePrimaryIdentityState) { nativeInspectIdentity(path) }
    }
    AsyncFunction("createGeneratedIdentity") { promise: Promise ->
      val path = PrnsAndroidRuntime.storagePath(context)
      PrnsAndroidRuntime.platformCall(promise, FfiConverterTypeIdentityCreationOutcome) { nativeCreateGeneratedIdentity(path) }
    }
    AsyncFunction("createImportedIdentity") { input: List<Int>, promise: Promise ->
      val identity = PrnsCodec.bytes(input)
      val path = PrnsAndroidRuntime.storagePath(context)
      PrnsAndroidRuntime.platformCall(promise, FfiConverterTypeIdentityCreationOutcome) { nativeCreateImportedIdentity(path, identity) }
    }
    AsyncFunction("start") { input: List<Int>, promise: Promise ->
      PrnsRuntimeService.startRuntime(context, PrnsCodec.bytes(input), promise)
    }
    AsyncFunction("stop") { promise: Promise -> PrnsRuntimeService.stopRuntime(context, false, promise) }
    AsyncFunction("reset") { promise: Promise -> PrnsRuntimeService.stopRuntime(context, true, promise) }
    AsyncFunction("prepareOutbound") { promise: Promise -> PrnsAndroidRuntime.prepareOutbound(promise) }
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
