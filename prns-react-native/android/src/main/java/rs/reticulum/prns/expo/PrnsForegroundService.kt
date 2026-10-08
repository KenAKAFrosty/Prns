package rs.reticulum.prns.expo

import rs.reticulum.prns.expo.PrnsBluetoothLink
import rs.reticulum.prns.expo.PrnsBluetoothGeneration
import rs.reticulum.prns.expo.PrnsBluetoothLifecycle

import android.app.PendingIntent
import android.app.Service
import android.bluetooth.BluetoothAdapter
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.location.LocationManager
import android.os.Build
import android.os.IBinder
import expo.modules.kotlin.Promise

/** A foreground service owns native lifetime; destroying a React screen does not. */
abstract class PrnsForegroundService : Service() {
  protected abstract val runtime: PrnsForegroundRuntime
  /** Constructed by Android, including process restart, without a React runtime. */
  protected abstract fun createDelegate(): PrnsRuntimeDelegate
  private lateinit var delegate: PrnsRuntimeDelegate
  private var bluetooth: PrnsBluetoothLink? = null
  @Volatile private var bridgePrepared = false
  @Volatile private var engineRunning = false
  @Volatile private var finishing = false
  @Volatile private var ownsRuntime = false
  @Volatile private var serviceDestroyed = false
  private val bluetoothGeneration = PrnsBluetoothGeneration()
  private val radioReceiver = object : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) { runtime.refresh(context) }
  }

  override fun onCreate() {
    super.onCreate()
    delegate = createDelegate()
    ownsRuntime = runtime.admission.reserveOwner(this)
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
      stopRuntime(this, runtime, delegate, false, null)
      return START_NOT_STICKY
    }
    val requestId = intent?.getLongExtra(EXTRA_REQUEST, 0) ?: runtime.admission.enqueueStart()
    if (!ownsRuntime || finishing || serviceDestroyed || !runtime.admission.isOwnerActive(this)) {
      rejectStart(requestId, "The previous node is still stopping")
      stopSelf(startId)
      return START_NOT_STICKY
    }
    try {
      startForeground(delegate.notificationId, delegate.notification(this, stopIntent()), delegate.foregroundServiceType)
    } catch (error: Exception) {
      runtime.requests.remove(requestId)?.reject("ERR_PRNS_SERVICE", error.message, error)
      runtime.admission.completeStart(requestId)
      runtime.update(this, "failed", "Could not start the background connection service")
      stopSelf()
      return START_NOT_STICKY
    }
    if (!runtime.admission.isStartCurrent(requestId)) {
      rejectStart(requestId, "Node start was cancelled by Stop or Reset")
      if (!engineRunning && !bridgePrepared) stopSelf(startId)
      return START_NOT_STICKY
    }
    val input = intent?.getByteArrayExtra(EXTRA_INPUT) ?: delegate.savedInput()
    if (input == null) {
      runtime.admission.completeStart(requestId)
      stopSelf()
      return START_NOT_STICKY
    }
    runtime.lifecycle.execute {
      // The service can retire while this worker waits behind Stop or a failed Start.
      if (!ownsRuntime || finishing || serviceDestroyed || !runtime.admission.canStart(this, requestId)) {
        rejectStart(requestId, "Node start was cancelled because its service is stopping")
        return@execute
      }
      try {
        runtime.update(this, "starting")
        if (delegate.bluetoothEnabled && !engineRunning && !bridgePrepared) {
          check(PrnsBluetoothLifecycle.nativePrepareBluetooth()) { "Previous Bluetooth session is still stopping" }
          bridgePrepared = true
        }
        refreshBluetooth()
        val outcome = delegate.start(input)
        if (outcome.running) {
          engineRunning = true
          delegate.saveInput(input)
          runtime.update(this, "running")
        } else {
          cleanupFailedStart()
          runtime.update(this, "failed", outcome.error)
        }
        if (runtime.admission.completeStart(requestId)) runtime.requests.remove(requestId)?.resolve(outcome.response)
      } catch (error: Exception) {
        val cleanupFailure = runCatching { cleanupFailedStart() }.exceptionOrNull()
        runtime.update(this, "failed", cleanupFailure?.let { "Startup failed; shutdown is incomplete: ${it.message}" } ?: error.message)
        if (runtime.admission.completeStart(requestId)) runtime.requests.remove(requestId)?.reject("ERR_PRNS_START", error.message, error)
      }
    }
    return START_STICKY
  }

  fun refreshBluetooth(): Boolean {
    if (!delegate.bluetoothEnabled) return true
    if (!bridgePrepared || finishing || serviceDestroyed || !runtime.admission.isOwnerActive(this)) return false
    bluetooth?.setAppForeground(runtime.appForeground)
    if (runtime.bluetoothReady(this)) {
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
            runtime.lifecycle.execute {
              if (bluetoothGeneration.isCurrent(generation) && ownsRuntime && !finishing && runtime.admission.isOwnerActive(this)) {
                runtime.update(applicationContext, error = status.reason)
              }
            }
          },
          onPsmPublished = { psm ->
            runtime.lifecycle.execute {
              if (bluetoothGeneration.isCurrent(generation) && ownsRuntime && !finishing && runtime.admission.isOwnerActive(this)) {
                if (bluetoothGeneration.publish(generation, psm) && engineRunning) reconnectBluetooth()
              }
            }
          },
        )
        candidate.setAppForeground(runtime.appForeground)
        if (candidate.start()) bluetooth = candidate
        else if (!candidate.stop()) {
          bluetooth = candidate
          return false
        }
      }
    } else if (bluetooth != null) {
      if (bluetooth?.stop() == true) bluetooth = null
      else {
        runtime.update(this, error = "Bluetooth is still stopping")
        return false
      }
    }
    return true
  }

  private fun reconnectBluetooth(): Boolean {
    if (finishing || serviceDestroyed || !runtime.admission.isOwnerActive(this)) return false
    // Upstream currently samples the local L2CAP PSM only at interface startup.
    // A replacement listener therefore needs a full, explicit generation restart.
    // The public disable/enable API has no completion acknowledgement, so it
    // cannot safely replace this ordered drain with an immediate enable toggle.
    // Keep the service/identity, but do not pretend unrelated TCP is uninterrupted.
    val input = delegate.savedInput() ?: return false
    return try {
      runtime.update(this, "stopping", "Reconnecting after Bluetooth changed")
      // Retire callbacks already queued by the old listener before draining it.
      bluetoothGeneration.retireListener()
      check(bluetooth?.stop() ?: true) { "Bluetooth workers are still stopping" }
      bluetooth = null
      check(delegate.stop(false).stopped) { "Native node is still stopping" }
      check(PrnsBluetoothLifecycle.nativeReleaseBluetooth()) { "Previous Bluetooth session is still stopping" }
      bridgePrepared = false
      engineRunning = false
      bluetoothGeneration.nativeStopped()
      runtime.update(this, "starting")
      check(PrnsBluetoothLifecycle.nativePrepareBluetooth()) { "Could not prepare Bluetooth" }
      bridgePrepared = true
      check(refreshBluetooth()) { "Bluetooth workers are still stopping" }
      val outcome = delegate.start(input)
      check(outcome.running) { "Could not restart the node after Bluetooth changed" }
      engineRunning = true
      runtime.update(this, "running")
      true
    } catch (error: Exception) {
      val cleanupFailure = runCatching { cleanupFailedStart() }.exceptionOrNull()
      runtime.update(this, "failed", cleanupFailure?.message ?: error.message)
      false
    }
  }

  private fun cleanupFailedStart() {
    runtime.admission.retireOwner(this)
    delegate.clearInput()
    val drained = bluetooth?.stop() ?: true
    if (drained) {
      bluetooth = null
      val outcome = delegate.stop(false)
      if (outcome.stopped && (!bridgePrepared || PrnsBluetoothLifecycle.nativeReleaseBluetooth())) {
        bridgePrepared = false
        engineRunning = false
        bluetoothGeneration.nativeStopped()
        finishing = true
        releaseDestroyedOwner()
        stopSelf()
      }
    }
  }

  private fun stopNative(reset: Boolean): PrnsRuntimeStop {
    runtime.admission.retireOwner(this)
    delegate.clearInput()
    runtime.update(this, "stopping")
    check(bluetooth?.stop() ?: true) { "Bluetooth workers are still stopping; try Stop again" }
    bluetooth = null
    val outcome = delegate.stop(reset)
    // The delegate must acknowledge joined native teardown before bridge release.
    if (outcome.stopped && (!bridgePrepared || PrnsBluetoothLifecycle.nativeReleaseBluetooth())) {
      bridgePrepared = false
      engineRunning = false
      bluetoothGeneration.nativeStopped()
      finishing = true
      releaseDestroyedOwner()
      runtime.update(this, "stopped")
      stopForeground(STOP_FOREGROUND_REMOVE)
      stopSelf()
    } else {
      runtime.update(this, "stopping", "Native shutdown has not completed")
    }
    return outcome
  }

  override fun onDestroy() {
    serviceDestroyed = true
    runtime.admission.retireOwner(this)
    if (!ownsRuntime) {
      super.onDestroy()
      return
    }
    unregisterReceiver(radioReceiver)
    // Android may destroy the service without destroying the process. Tear down
    // through the same serial owner, retaining ownership if draining fails.
    runtime.lifecycle.execute {
      if (!finishing) runCatching { stopNative(false) }.onFailure {
        runtime.update(this, "stopping", it.message)
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
      runtime.admission.releaseOwner(this, drained = true)
      ownsRuntime = false
    }
  }

  private fun rejectStart(requestId: Long, reason: String) {
    runtime.admission.completeStart(requestId)
    // Always remove the promise: Stop may have invalidated a ticket just before
    // startRuntime registered its promise in the concurrent map.
    runtime.requests.remove(requestId)?.reject("ERR_PRNS_CANCELLED", reason, null)
  }

  private fun stopIntent(): PendingIntent = PendingIntent.getService(
    this, 1, Intent(this, javaClass).setAction(ACTION_STOP),
    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
  )

  companion object {
    private const val ACTION_STOP = "rs.reticulum.prns.expo.STOP"
    private const val EXTRA_INPUT = "startInputUniFFI"
    private const val EXTRA_REQUEST = "request"

    fun startRuntime(
      context: Context,
      serviceClass: Class<out PrnsForegroundService>,
      runtime: PrnsForegroundRuntime,
      input: ByteArray,
      promise: Promise,
      validateInput: (ByteArray) -> Unit,
    ) {
      // Reject malformed input before reserving a service/start ticket.
      validateInput(input)
      val id = runtime.admission.enqueueStart()
      runtime.requests[id] = promise
      try {
        context.startForegroundService(Intent(context, serviceClass).putExtra(EXTRA_INPUT, input).putExtra(EXTRA_REQUEST, id))
      } catch (error: Exception) {
        runtime.requests.remove(id)
        runtime.admission.completeStart(id)
        runtime.update(context, "failed", "Could not start the background connection service")
        promise.reject("ERR_PRNS_SERVICE", error.message, error)
      }
    }

    fun stopRuntime(context: Context, runtime: PrnsForegroundRuntime, delegate: PrnsRuntimeDelegate, reset: Boolean, promise: Promise?) {
      runtime.admission.beginStop().forEach { id ->
        runtime.requests.remove(id)?.reject("ERR_PRNS_CANCELLED", "Host start was cancelled by Stop or Reset", null)
      }
      runtime.lifecycle.execute {
        try {
          val service = runtime.service
          val outcome = if (service != null) service.stopNative(reset) else {
            val result = delegate.stop(reset)
            if (result.stopped) {
              PrnsBluetoothLifecycle.nativeReleaseBluetooth()
              runtime.update(context, "stopped")
            }
            result
          }
          promise?.resolve(outcome.response)
        } catch (error: Exception) {
          runtime.update(context, "stopping", error.message)
          promise?.reject("ERR_PRNS_STOP", error.message, error)
        }
      }
    }
  }
}
