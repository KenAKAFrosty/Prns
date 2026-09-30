package rs.reticulum.prns.app.expo

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.util.Base64
import expo.modules.kotlin.Promise
import rs.reticulum.prns.app.bindings.*
import rs.reticulum.prns.expo.PrnsForegroundRuntime
import rs.reticulum.prns.expo.PrnsForegroundService
import rs.reticulum.prns.expo.PrnsRuntimeDelegate
import rs.reticulum.prns.expo.PrnsRuntimeStart
import rs.reticulum.prns.expo.PrnsRuntimeStop

/** Product configuration and notification copy; lifetime mechanics live in the SDK. */
class PrnsRuntimeService : PrnsForegroundService() {
  override val runtime: PrnsForegroundRuntime get() = PrnsAndroidRuntime.owner
  override fun createDelegate(): PrnsRuntimeDelegate = ProductRuntimeDelegate(applicationContext)

  companion object {
    internal const val CHANNEL = "prns-connections"

    internal fun startRuntime(context: Context, input: ByteArray, promise: Promise) =
      PrnsForegroundService.startRuntime(context, PrnsRuntimeService::class.java, PrnsAndroidRuntime.owner, input, promise) {
        PrnsCodec.decode(it, FfiConverterTypeDevelopmentNodeStartInput)
      }

    internal fun stopRuntime(context: Context, reset: Boolean, promise: Promise?) =
      PrnsForegroundService.stopRuntime(context, PrnsAndroidRuntime.owner, ProductRuntimeDelegate(context.applicationContext), reset, promise)
  }
}

private class ProductRuntimeDelegate(private val context: Context) : PrnsRuntimeDelegate {
  override val notificationId = 3101
  private val preferences get() = context.getSharedPreferences("prns-service", Context.MODE_PRIVATE)

  override fun savedInput(): ByteArray? = preferences.getString("startInputUniFFI", null)?.let {
    runCatching { Base64.decode(it, Base64.NO_WRAP) }.getOrNull()
  }
  override fun saveInput(input: ByteArray) {
    preferences.edit().putString("startInputUniFFI", Base64.encodeToString(input, Base64.NO_WRAP)).apply()
  }
  override fun clearInput() { preferences.edit().remove("startInputUniFFI").apply() }

  override fun start(input: ByteArray): PrnsRuntimeStart {
    val outcome = nativeStart(PrnsAndroidRuntime.storagePath(context), PrnsCodec.decode(input, FfiConverterTypeDevelopmentNodeStartInput))
    return PrnsRuntimeStart(
      PrnsCodec.encode(outcome, FfiConverterTypeDevelopmentNodeStartOutcome),
      outcome is DevelopmentNodeStartOutcome.Started || outcome is DevelopmentNodeStartOutcome.AlreadyRunning,
      (outcome as? DevelopmentNodeStartOutcome.Failed)?.detail,
    )
  }

  override fun stop(reset: Boolean): PrnsRuntimeStop {
    val outcome = if (reset) nativeReset(PrnsAndroidRuntime.storagePath(context)) else nativeStop()
    // Reset failure can still leave a successfully stopped owner. Always read
    // the joined stop acknowledgement before releasing its Bluetooth generation.
    val final = nativeStop()
    return PrnsRuntimeStop(PrnsCodec.encode(outcome, FfiConverterTypeDevelopmentNodeStopOutcome),
      final is DevelopmentNodeStopOutcome.Stopped || final is DevelopmentNodeStopOutcome.AlreadyStopped)
  }

  override fun notification(service: Service, stop: PendingIntent): Notification {
    service.getSystemService(NotificationManager::class.java).createNotificationChannel(
      NotificationChannel(PrnsRuntimeService.CHANNEL, "Node connections", NotificationManager.IMPORTANCE_LOW),
    )
    val launch = service.packageManager.getLaunchIntentForPackage(service.packageName)
    val builder = Notification.Builder(service, PrnsRuntimeService.CHANNEL)
      .setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
      .setContentTitle("prns is running")
      .setContentText("Keeping your node connections available")
      .setOngoing(true)
      .addAction(Notification.Action.Builder(null, "Stop", stop).build())
    if (launch != null) builder.setContentIntent(PendingIntent.getActivity(service, 0, launch, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE))
    return builder.build()
  }
}
