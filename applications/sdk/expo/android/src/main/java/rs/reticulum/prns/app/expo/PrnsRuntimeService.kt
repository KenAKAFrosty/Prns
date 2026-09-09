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
  @Volatile private var serviceDestroyed = false
  private val bluetoothGeneration = PrnsBluetoothGeneration()
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
    // Stop remains available for a retained owner after an incomplete drain.
    if (intent?.action == ACTION_STOP) {
      stopRuntime(this, false, null)
      return START_NOT_STICKY
    }
    val requestId = intent?.getLongExtra(EXTRA_REQUEST, 0) ?: PrnsAndroidRuntime.admission.enqueueStart()
    if (!ownsRuntime || finishing || serviceDestroyed || !PrnsAndroidRuntime.admission.isOwnerActive(this)) {
      rejectStart(requestId, "The previous node is still stopping")
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
    if (!PrnsAndroidRuntime.admission.isStartCurrent(requestId)) {
      rejectStart(requestId, "Node start was cancelled by Stop or Reset")
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
      // The service can retire while this worker waits behind Stop or a failed Start.
      if (!ownsRuntime || finishing || serviceDestroyed || !PrnsAndroidRuntime.admission.canStart(this, requestId)) {
        rejectStart(requestId, "Node start was cancelled because its service is stopping")
        return@execute
      }
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

  internal fun refreshBluetooth(): Boolean {
    if (!bridgePrepared || finishing || serviceDestroyed || !PrnsAndroidRuntime.admission.isOwnerActive(this)) return false
    bluetooth?.setAppForeground(PrnsAndroidRuntime.appForeground)
    if (PrnsAndroidRuntime.bluetoothReady(this)) {
      if (bluetooth?.status?.state in setOf(PrnsBluetoothLink.State.Stopping, PrnsBluetoothLink.State.Failed, PrnsBluetoothLink.State.Unavailable)) {
        if (bluetooth?.stop() != true) return false
        bluetooth = null
      }
      if (bluetooth == null) {
        // A replacement listener receives a new Android PSM. Retire the native
        // generation BEFORE opening it, not from its later publication callback:
        // that would start two scans and cancel a Check admitted in between.
        if (bluetoothGeneration.restartBeforeReplacement(engineRunning)) {
          return reconnectBluetooth()
        }
        val generation = bluetoothGeneration.beginListener()
        val candidate = PrnsBluetoothLink(
          applicationContext,
          onStatus = { status ->
            PrnsAndroidRuntime.lifecycle.execute {
              if (bluetoothGeneration.isCurrent(generation) && ownsRuntime && !finishing && PrnsAndroidRuntime.admission.isOwnerActive(this)) {
                PrnsAndroidRuntime.update(applicationContext, error = status.reason)
              }
            }
          },
          onPsmPublished = { psm ->
            PrnsAndroidRuntime.lifecycle.execute {
              if (bluetoothGeneration.isCurrent(generation) && ownsRuntime && !finishing && PrnsAndroidRuntime.admission.isOwnerActive(this)) {
                if (bluetoothGeneration.publish(generation, psm) && engineRunning) reconnectBluetooth()
              }
            }
          },
        )
        candidate.setAppForeground(PrnsAndroidRuntime.appForeground)
        if (candidate.start()) bluetooth = candidate
        else if (!candidate.stop()) {
          bluetooth = candidate
          return false
        }
      }
    } else if (bluetooth != null) {
      if (bluetooth?.stop() == true) bluetooth = null
      else {
        PrnsAndroidRuntime.update(this, error = "Bluetooth is still stopping")
        return false
      }
    }
    return true
  }

  private fun reconnectBluetooth(): Boolean {
    if (finishing || serviceDestroyed || !PrnsAndroidRuntime.admission.isOwnerActive(this)) return false
    // Upstream currently samples the local L2CAP PSM only at interface startup.
    // A replacement listener therefore needs a full, explicit generation restart.
    // The public disable/enable API has no completion acknowledgement, so it
    // cannot safely replace this ordered drain with an immediate enable toggle.
    // Keep the service/identity, but do not pretend unrelated TCP is uninterrupted.
    val input = getSharedPreferences(PREFERENCES, MODE_PRIVATE).getString("startInput", null) ?: return false
    return try {
      PrnsAndroidRuntime.update(this, "stopping", "Reconnecting after Bluetooth changed")
      // Retire callbacks already queued by the old listener before draining it.
      bluetoothGeneration.retireListener()
      check(bluetooth?.stop() ?: true) { "Bluetooth workers are still stopping" }
      bluetooth = null
      check(isStopped(PrnsNative.nativeCall("stop", null, null))) { "Native node is still stopping" }
      check(PrnsNative.nativeReleaseBluetooth()) { "Previous Bluetooth session is still stopping" }
      bridgePrepared = false
      engineRunning = false
      bluetoothGeneration.nativeStopped()
      PrnsAndroidRuntime.update(this, "starting")
      check(PrnsNative.nativePrepareBluetooth()) { "Could not prepare Bluetooth" }
      bridgePrepared = true
      check(refreshBluetooth()) { "Bluetooth workers are still stopping" }
      val outcome = PrnsAndroidRuntime.consume(PrnsNative.nativeCall("start", PrnsAndroidRuntime.storagePath(this), input.toByteArray(Charsets.UTF_8)))
      check(JSONObject(outcome).optString("type") in setOf("started", "alreadyRunning")) { "Could not restart the node after Bluetooth changed" }
      engineRunning = true
      PrnsAndroidRuntime.update(this, "running")
      true
    } catch (error: Exception) {
      val cleanupFailure = runCatching { cleanupFailedStart() }.exceptionOrNull()
      PrnsAndroidRuntime.update(this, "failed", cleanupFailure?.message ?: error.message)
      false
    }
  }

  private fun cleanupFailedStart() {
    PrnsAndroidRuntime.admission.retireOwner(this)
    getSharedPreferences(PREFERENCES, MODE_PRIVATE).edit().remove("startInput").apply()
    val drained = bluetooth?.stop() ?: true
    if (drained) {
      bluetooth = null
      val outcome = PrnsNative.nativeCall("stop", null, null)
      if (isStopped(outcome) && PrnsNative.nativeReleaseBluetooth()) {
        bridgePrepared = false
        engineRunning = false
        bluetoothGeneration.nativeStopped()
        finishing = true
        releaseDestroyedOwner()
        stopSelf()
      }
    }
  }

  private fun stopNative(reset: Boolean): String {
    PrnsAndroidRuntime.admission.retireOwner(this)
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
      bluetoothGeneration.nativeStopped()
      finishing = true
      releaseDestroyedOwner()
      PrnsAndroidRuntime.update(this, "stopped")
      stopForeground(STOP_FOREGROUND_REMOVE)
      stopSelf()
    } else {
      PrnsAndroidRuntime.update(this, "stopping", "Native shutdown has not completed")
    }
    return outcome
  }

  override fun onDestroy() {
    serviceDestroyed = true
    PrnsAndroidRuntime.admission.retireOwner(this)
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
        releaseDestroyedOwner()
      }
    }
    super.onDestroy()
  }

  private fun releaseDestroyedOwner() {
    // onDestroy is delivered only once. If its first drain failed, a later
    // explicit Stop must release the retained owner itself after draining.
    if (serviceDestroyed && finishing) {
      PrnsAndroidRuntime.admission.releaseOwner(this, drained = true)
      ownsRuntime = false
    }
  }

  private fun rejectStart(requestId: Long, reason: String) {
    PrnsAndroidRuntime.admission.completeStart(requestId)
    // Always remove the promise: Stop may have invalidated a ticket just before
    // startRuntime registered its promise in the concurrent map.
    requests.remove(requestId)?.reject("ERR_PRNS_CANCELLED", reason, null)
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
    internal const val CHANNEL = "prns-connections"
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
      PrnsAndroidRuntime.admission.beginStop().forEach { id ->
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
