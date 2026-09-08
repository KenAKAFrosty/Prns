package rs.reticulum.prns.app.expo

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.bluetooth.BluetoothAdapter
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.location.LocationManager
import android.os.Build
import android.os.IBinder
import expo.modules.kotlin.Promise
import java.util.concurrent.ConcurrentHashMap
import org.json.JSONObject

/** A foreground service owns native lifetime; destroying a React screen does not. */
class PrnsRuntimeService : Service() {
  private var bluetooth: PrnsBluetoothLink? = null
  @Volatile private var bridgePrepared = false
  @Volatile private var engineRunning = false
  @Volatile private var finishing = false
  @Volatile private var ownsRuntime = false
  @Volatile private var bluetoothGeneration = 0L
  private var publishedPsm: Int? = null
  private val radioReceiver = object : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) { PrnsAndroidRuntime.refresh(context) }
  }

  override fun onCreate() {
    super.onCreate()
    ownsRuntime = PrnsAndroidRuntime.admission.reserveOwner(this)
    if (!ownsRuntime || finishing) {
      stopSelf()
      return
    }
    val filter = IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED).apply {
      addAction(LocationManager.MODE_CHANGED_ACTION)
    }
    if (Build.VERSION.SDK_INT >= 33) registerReceiver(radioReceiver, filter, RECEIVER_EXPORTED)
    else registerReceiver(radioReceiver, filter)
    // The receiver only schedules a fresh permission/radio read; intent data is not trusted.
  }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val requestId = intent?.getLongExtra(EXTRA_REQUEST, 0) ?: PrnsAndroidRuntime.admission.enqueueStart()
    if (!ownsRuntime || finishing) {
      PrnsAndroidRuntime.admission.completeStart(requestId)
      requests.remove(requestId)?.reject("ERR_PRNS_SERVICE", "The previous node is still stopping", null)
      stopSelf(startId)
      return START_NOT_STICKY
    }
    try {
      startForeground(NOTIFICATION_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
    } catch (error: Exception) {
      requests.remove(requestId)?.reject("ERR_PRNS_SERVICE", error.message, error)
      PrnsAndroidRuntime.admission.completeStart(requestId)
      PrnsAndroidRuntime.update(this, "failed", "Could not start the background connection service")
      stopSelf()
      return START_NOT_STICKY
    }
    if (intent?.action == ACTION_STOP) {
      stopRuntime(this, false, null)
      return START_NOT_STICKY
    }
    if (!PrnsAndroidRuntime.admission.isStartCurrent(requestId)) {
      if (!engineRunning && !bridgePrepared) stopSelf(startId)
      return START_NOT_STICKY
    }
    val preferences = getSharedPreferences(PREFERENCES, MODE_PRIVATE)
    val input = intent?.getStringExtra(EXTRA_INPUT) ?: preferences.getString("startInput", null)
    if (input == null) {
      PrnsAndroidRuntime.admission.completeStart(requestId)
      stopSelf()
      return START_NOT_STICKY
    }
    PrnsAndroidRuntime.lifecycle.execute {
      if (!PrnsAndroidRuntime.admission.isStartCurrent(requestId)) return@execute
      try {
        PrnsAndroidRuntime.update(this, "starting")
        if (!engineRunning && !bridgePrepared) {
          check(PrnsNative.nativePrepareBluetooth()) { "Previous Bluetooth session is still stopping" }
          bridgePrepared = true
        }
        refreshBluetooth()
        val outcome = PrnsAndroidRuntime.consume(PrnsNative.nativeCall("start", PrnsAndroidRuntime.storagePath(this), input.toByteArray(Charsets.UTF_8)))
        val type = JSONObject(outcome).optString("type")
        if (type == "started" || type == "alreadyRunning") {
          engineRunning = true
          preferences.edit().putString("startInput", input).apply()
          PrnsAndroidRuntime.update(this, "running")
        } else {
          cleanupFailedStart()
          PrnsAndroidRuntime.update(this, "failed", JSONObject(outcome).optString("detail", "Could not start the node"))
        }
        if (PrnsAndroidRuntime.admission.completeStart(requestId)) requests.remove(requestId)?.resolve(outcome)
      } catch (error: Exception) {
        val cleanupFailure = runCatching { cleanupFailedStart() }.exceptionOrNull()
        PrnsAndroidRuntime.update(this, "failed", cleanupFailure?.let { "Startup failed; shutdown is incomplete: ${it.message}" } ?: error.message)
        if (PrnsAndroidRuntime.admission.completeStart(requestId)) requests.remove(requestId)?.reject("ERR_PRNS_START", error.message, error)
      }
    }
    return START_STICKY
  }

  internal fun refreshBluetooth() {
    if (!bridgePrepared || finishing) return
    bluetooth?.setAppForeground(PrnsAndroidRuntime.appForeground)
    if (PrnsAndroidRuntime.bluetoothReady(this)) {
      if (bluetooth?.status?.state in setOf(PrnsBluetoothLink.State.Stopping, PrnsBluetoothLink.State.Failed, PrnsBluetoothLink.State.Unavailable)) {
        if (bluetooth?.stop() == true) bluetooth = null
      }
      if (bluetooth == null) {
        val generation = ++bluetoothGeneration
        val candidate = PrnsBluetoothLink(
          applicationContext,
          onStatus = { status ->
            PrnsAndroidRuntime.lifecycle.execute {
              if (generation == bluetoothGeneration && ownsRuntime && !finishing) {
                PrnsAndroidRuntime.update(applicationContext, error = status.reason)
              }
            }
          },
          onPsmPublished = { psm ->
            PrnsAndroidRuntime.lifecycle.execute {
              if (generation == bluetoothGeneration && ownsRuntime && !finishing) {
                val previous = publishedPsm
                publishedPsm = psm
                if (previous != null && previous != psm && engineRunning) reconnectBluetooth()
              }
            }
          },
        )
        candidate.setAppForeground(PrnsAndroidRuntime.appForeground)
        if (candidate.start()) bluetooth = candidate
        else if (!candidate.stop()) bluetooth = candidate
      }
    } else if (bluetooth != null) {
      if (bluetooth?.stop() == true) bluetooth = null
      else PrnsAndroidRuntime.update(this, error = "Bluetooth is still stopping")
    }
  }

  private fun reconnectBluetooth() {
    // Upstream currently samples the local L2CAP PSM only at interface startup.
    // A replacement listener therefore needs a full, explicit generation restart.
    // Keep the service/identity, but do not pretend unrelated TCP is uninterrupted.
    val input = getSharedPreferences(PREFERENCES, MODE_PRIVATE).getString("startInput", null) ?: return
    try {
      PrnsAndroidRuntime.update(this, "stopping", "Reconnecting after Bluetooth changed")
      check(bluetooth?.stop() ?: true) { "Bluetooth workers are still stopping" }
      bluetooth = null
      check(isStopped(PrnsNative.nativeCall("stop", null, null))) { "Native node is still stopping" }
      check(PrnsNative.nativeReleaseBluetooth()) { "Previous Bluetooth session is still stopping" }
      bridgePrepared = false
      engineRunning = false
      publishedPsm = null
      PrnsAndroidRuntime.update(this, "starting")
      check(PrnsNative.nativePrepareBluetooth()) { "Could not prepare Bluetooth" }
      bridgePrepared = true
      refreshBluetooth()
      val outcome = PrnsAndroidRuntime.consume(PrnsNative.nativeCall("start", PrnsAndroidRuntime.storagePath(this), input.toByteArray(Charsets.UTF_8)))
      check(JSONObject(outcome).optString("type") in setOf("started", "alreadyRunning")) { "Could not restart the node after Bluetooth changed" }
      engineRunning = true
      PrnsAndroidRuntime.update(this, "running")
    } catch (error: Exception) {
      val cleanupFailure = runCatching { cleanupFailedStart() }.exceptionOrNull()
      PrnsAndroidRuntime.update(this, "failed", cleanupFailure?.message ?: error.message)
    }
  }

  private fun cleanupFailedStart() {
    getSharedPreferences(PREFERENCES, MODE_PRIVATE).edit().remove("startInput").apply()
    val drained = bluetooth?.stop() ?: true
    if (drained) {
      bluetooth = null
      val outcome = PrnsNative.nativeCall("stop", null, null)
      if (isStopped(outcome) && PrnsNative.nativeReleaseBluetooth()) {
        bridgePrepared = false
        engineRunning = false
        publishedPsm = null
        finishing = true
        stopSelf()
      }
    }
  }

  private fun stopNative(reset: Boolean): String {
    getSharedPreferences(PREFERENCES, MODE_PRIVATE).edit().remove("startInput").apply()
    PrnsAndroidRuntime.update(this, "stopping")
    check(bluetooth?.stop() ?: true) { "Bluetooth workers are still stopping; try Stop again" }
    bluetooth = null
    val outcome = PrnsAndroidRuntime.consume(PrnsNative.nativeCall(if (reset) "reset" else "stop", PrnsAndroidRuntime.storagePath(this), null))
    // Reset is gated by Rust's complete stop; never release a live generation.
    val stopped = isStopped(PrnsNative.nativeCall("stop", null, null))
    if (stopped && PrnsNative.nativeReleaseBluetooth()) {
      bridgePrepared = false
      engineRunning = false
      publishedPsm = null
      finishing = true
      PrnsAndroidRuntime.update(this, "stopped")
      stopForeground(STOP_FOREGROUND_REMOVE)
      stopSelf()
    } else {
      PrnsAndroidRuntime.update(this, "stopping", "Native shutdown has not completed")
    }
    return outcome
  }

  override fun onDestroy() {
    if (!ownsRuntime) {
      super.onDestroy()
      return
    }
    unregisterReceiver(radioReceiver)
    // Android may destroy the service without destroying the process. Tear down
    // through the same serial owner, retaining ownership if draining fails.
    PrnsAndroidRuntime.lifecycle.execute {
      if (!finishing) runCatching { stopNative(false) }.onFailure {
        PrnsAndroidRuntime.update(this, "stopping", it.message)
      }
      if (finishing) {
        PrnsAndroidRuntime.admission.releaseOwner(this, drained = true)
        ownsRuntime = false
      }
    }
    super.onDestroy()
  }

  private fun notification(): Notification {
    val manager = getSystemService(NotificationManager::class.java)
    manager.createNotificationChannel(NotificationChannel(CHANNEL, "Node connections", NotificationManager.IMPORTANCE_LOW))
    val stop = PendingIntent.getService(this, 1, Intent(this, PrnsRuntimeService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    val launch = packageManager.getLaunchIntentForPackage(packageName)
    val builder = Notification.Builder(this, CHANNEL)
      .setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
      .setContentTitle("prns is running")
      .setContentText("Keeping your node connections available")
      .setOngoing(true)
      .addAction(Notification.Action.Builder(null, "Stop", stop).build())
    if (launch != null) builder.setContentIntent(PendingIntent.getActivity(this, 0, launch, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE))
    return builder.build()
  }

  companion object {
    private const val CHANNEL = "prns-connections"
    private const val NOTIFICATION_ID = 3101
    private const val PREFERENCES = "prns-service"
    private const val ACTION_STOP = "rs.reticulum.prns.app.STOP"
    private const val EXTRA_INPUT = "startInput"
    private const val EXTRA_REQUEST = "request"
    private val requests = ConcurrentHashMap<Long, Promise>()

    private fun isStopped(json: String): Boolean = JSONObject(json).optString("type") in setOf("stopped", "alreadyStopped")

    internal fun startRuntime(context: Context, input: String, promise: Promise) {
      val id = PrnsAndroidRuntime.admission.enqueueStart()
      requests[id] = promise
      try {
        context.startForegroundService(Intent(context, PrnsRuntimeService::class.java).putExtra(EXTRA_INPUT, input).putExtra(EXTRA_REQUEST, id))
      } catch (error: Exception) {
        requests.remove(id)
        PrnsAndroidRuntime.admission.completeStart(id)
        PrnsAndroidRuntime.update(context, "failed", "Could not start the background connection service")
        promise.reject("ERR_PRNS_SERVICE", error.message, error)
      }
    }

    internal fun stopRuntime(context: Context, reset: Boolean, promise: Promise?) {
      PrnsAndroidRuntime.admission.invalidateStarts().forEach { id ->
        requests.remove(id)?.reject("ERR_PRNS_CANCELLED", "Node start was cancelled by Stop or Reset", null)
      }
      PrnsAndroidRuntime.lifecycle.execute {
        try {
          val service = PrnsAndroidRuntime.service
          val outcome = if (service != null) service.stopNative(reset) else {
            val result = PrnsAndroidRuntime.consume(PrnsNative.nativeCall(if (reset) "reset" else "stop", PrnsAndroidRuntime.storagePath(context), null))
            if (isStopped(PrnsNative.nativeCall("stop", null, null))) {
              PrnsNative.nativeReleaseBluetooth()
              PrnsAndroidRuntime.update(context, "stopped")
            }
            result
          }
          promise?.resolve(outcome)
        } catch (error: Exception) {
          PrnsAndroidRuntime.update(context, "stopping", error.message)
          promise?.reject("ERR_PRNS_STOP", error.message, error)
        }
      }
    }
  }
}
