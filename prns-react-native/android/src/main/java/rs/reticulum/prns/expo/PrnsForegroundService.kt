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
import java.util.concurrent.CancellationException

/** A foreground service owns native lifetime; destroying a React screen does not. */
abstract class PrnsForegroundService : Service() {
  protected abstract val runtime: PrnsForegroundRuntime
  /** Constructed by Android, including process restart, without a React runtime. */
  protected abstract fun createDelegate(): PrnsRuntimeDelegate
  private lateinit var delegate: PrnsRuntimeDelegate
  private var bluetooth: PrnsBluetoothLink? = null
  private var lanRuntime: PrnsLanRuntime? = null
  @Volatile private var bridgePrepared = false
  @Volatile private var engineRunning = false
  @Volatile private var finishing = false
  @Volatile private var ownsRuntime = false
  @Volatile private var serviceDestroyed = false
  private var reconnecting = false
  private val bluetoothGeneration = PrnsBluetoothGeneration()
  private val radioReceiver = object : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) { runtime.refresh(context) }
  }

  override fun onCreate() {
    super.onCreate()
    delegate = createDelegate()
    ownsRuntime = runtime.admission.reserveOwner(this)
    // A rejected replacement still receives the foreground-start Intent. Let
    // onStartCommand acknowledge Android's deadline before stopping that start.
    if (!ownsRuntime || finishing) return
    lanRuntime = PrnsLanRuntime(applicationContext).also { it.start() }
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
    // Every startForegroundService delivery must be acknowledged before native
    // admission, including cancelled starts and a replacement refused during drain.
    try {
      startForeground(delegate.notificationId, delegate.notification(this, stopIntent()), delegate.foregroundServiceType)
    } catch (error: Exception) {
      runtime.requests.remove(requestId)?.reject("ERR_PRNS_SERVICE", error.message, error)
      runtime.admission.completeStart(requestId)
      runtime.update(this, "failed", "Could not start the background connection service")
      stopSelf()
      return START_NOT_STICKY
    }
    if (!ownsRuntime || finishing || serviceDestroyed || !runtime.admission.isOwnerActive(this)) {
      rejectStart(requestId, "The previous node is still stopping")
      stopSelf(startId)
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
        prnsStartDelegate(runtime.lifecycle, delegate, input) { result ->
          // Admit this completion against the exact current owner/ticket. A Stop
          // racing after admission queues behind this save on the same lane and
          // must clear it before acknowledging shutdown. Call no delegate under
          // the admission monitor. Readiness first delivered after Stop is refused.
          if (!ownsRuntime || finishing || serviceDestroyed || !runtime.admission.canStart(this, requestId)) {
            rejectStart(requestId, "Node start was cancelled because its service is stopping")
            return@prnsStartDelegate
          }
          result.fold(
            onSuccess = { outcome ->
              if (outcome.running) {
                try {
                  engineRunning = true
                  delegate.saveInput(input)
                  val transportError = outcome.error.takeIf { delegate is PrnsAsyncRuntimeDelegate }
                  runtime.update(this, "running", transportError)
                  if (runtime.admission.completeStart(requestId)) {
                    val promise = runtime.requests.remove(requestId)
                    if (transportError != null) promise?.reject("ERR_PRNS_TRANSPORT", transportError, null)
                    else promise?.resolve(outcome.response)
                  }
                } catch (error: Exception) { failedStart(requestId, error = error) }
              } else failedStart(requestId, outcome = outcome)
            },
            onFailure = { failedStart(requestId, error = it) },
          )
        }
      } catch (error: Exception) { failedStart(requestId, error = error) }
    }
    return START_STICKY
  }

  fun refreshBluetooth(): Boolean {
    lanRuntime?.refresh()
    if (finishing || serviceDestroyed || !runtime.admission.isOwnerActive(this)) return false
    if (!delegate.bluetoothEnabled) return true
    if (!bridgePrepared || reconnecting) return false
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
    if (delegate is PrnsBluetoothRuntimeDelegate) {
      reconfigureBluetooth { result -> runtime.update(this, error = result.exceptionOrNull()?.message) }
      return false
    }
    if (reconnecting || finishing || serviceDestroyed || !runtime.admission.isOwnerActive(this)) return false
    // A replacement listener changes the PSM sampled at native interface startup.
    // Retain this service, and fence each continuation with an ordinary start
    // ticket so a concurrent Stop/destroy cannot restart the old generation.
    val input = delegate.savedInput() ?: return false
    val ticket = runtime.admission.enqueueStart()
    reconnecting = true
    runtime.update(this, "stopping", "Reconnecting after Bluetooth changed")
    bluetoothGeneration.retireListener()
    prnsStopDelegate(runtime.lifecycle, delegate, false) { stopped ->
      if (!canReconnect(ticket)) {
        finishReconnect(ticket)
        return@prnsStopDelegate
      }
      try {
        check(stopped.getOrThrow().stopped) { "Native node is still stopping" }
        check(bluetooth?.stop() ?: true) { "Bluetooth workers are still stopping" }
        bluetooth = null
        check(PrnsBluetoothLifecycle.nativeReleaseBluetooth()) { "Previous Bluetooth session is still stopping" }
        bridgePrepared = false
        engineRunning = false
        bluetoothGeneration.nativeStopped()
        runtime.update(this, "starting")
        check(PrnsBluetoothLifecycle.nativePrepareBluetooth()) { "Could not prepare Bluetooth" }
        bridgePrepared = true
        // Suppress replacement recovery while installing its first listener.
        reconnecting = false
        check(refreshBluetooth()) { "Bluetooth workers are still stopping" }
        reconnecting = true
        if (!canReconnect(ticket)) {
          finishReconnect(ticket)
          return@prnsStopDelegate
        }
        prnsStartDelegate(runtime.lifecycle, delegate, input) { started ->
          if (!canReconnect(ticket)) {
            finishReconnect(ticket)
            return@prnsStartDelegate
          }
          try {
            val outcome = started.getOrThrow()
            check(outcome.running) { outcome.error ?: "Could not restart the node after Bluetooth changed" }
            engineRunning = true
            runtime.update(this, "running", outcome.error.takeIf { delegate is PrnsAsyncRuntimeDelegate })
            finishReconnect(ticket)
          } catch (error: Exception) { reconnectFailed(ticket, error) }
        }
      } catch (error: Exception) { reconnectFailed(ticket, error) }
    }
    // An async delegate has not acknowledged readiness yet; outbound work must
    // retry after recovery. Synchronous delegates preserve their immediate result.
    return !reconnecting && engineRunning && runtime.admission.isOwnerActive(this)
  }

  fun reconfigureBluetooth(admitted: () -> Unit = {}, completion: (Result<Unit>) -> Unit) {
    runtime.lifecycle.execute {
      val transport = delegate as? PrnsBluetoothRuntimeDelegate
      if (transport == null || reconnecting || !engineRunning || finishing || serviceDestroyed ||
        !runtime.admission.isOwnerActive(this)) {
        completion(Result.failure(IllegalStateException("Bluetooth is unavailable or still changing")))
        return@execute
      }
      val ticket = runtime.admission.enqueueStart()
      reconnecting = true
      prnsReplaceBluetooth(runtime.lifecycle, { canReconnect(ticket) }, transport::detachBluetooth, {
        check(bluetooth?.stop() ?: true) { "Bluetooth workers are still stopping" }
        bluetooth = null
        if (bridgePrepared) check(PrnsBluetoothLifecycle.nativeReleaseBluetooth()) { "Bluetooth is still stopping" }
        bridgePrepared = false
        bluetoothGeneration.nativeStopped()
        if (delegate.bluetoothEnabled) {
          check(PrnsBluetoothLifecycle.nativePrepareBluetooth()) { "Could not prepare Bluetooth" }
          bridgePrepared = true
          reconnecting = false
          try { check(refreshBluetooth()) { "Bluetooth workers are still stopping" } }
          finally { reconnecting = true }
        }
      }, transport::attachBluetooth, admitted = {
        admitted()
        bluetoothGeneration.retireListener()
      }) { result ->
        finishReconnect(ticket)
        runtime.update(this, error = result.exceptionOrNull()?.message)
        completion(result)
      }
    }
  }

  private fun canReconnect(ticket: Long): Boolean =
    ownsRuntime && !finishing && !serviceDestroyed && runtime.admission.canStart(this, ticket)

  private fun finishReconnect(ticket: Long) {
    runtime.admission.completeStart(ticket)
    reconnecting = false
  }

  private fun reconnectFailed(ticket: Long, error: Throwable) {
    finishReconnect(ticket)
    cleanupFailedStart { cleanup ->
      if (cleanup.exceptionOrNull() !is CancellationException) {
        runtime.update(this, "failed", cleanup.exceptionOrNull()?.message ?: error.message)
      }
    }
  }

  private fun failedStart(requestId: Long, outcome: PrnsRuntimeStart? = null, error: Throwable? = null) {
    val promise = if (runtime.admission.completeStart(requestId)) runtime.requests.remove(requestId) else null
    cleanupFailedStart { cleanup ->
      if (cleanup.exceptionOrNull() !is CancellationException) {
        val failure = cleanup.exceptionOrNull()?.let { "Startup failed; shutdown is incomplete: ${it.message}" }
          ?: "Startup failed; shutdown has not completed".takeIf { cleanup.getOrNull()?.stopped == false }
        runtime.update(this, "failed", failure ?: error?.message ?: outcome?.error)
      }
      if (error != null) promise?.reject("ERR_PRNS_START", error.message, error)
      else promise?.resolve(outcome?.response)
    }
  }

  private fun cleanupFailedStart(completion: (Result<PrnsRuntimeStop>) -> Unit) {
    val operation = beginStop(runtime)
    stopNative(operation, completion)
  }

  private fun stopNative(
    operation: PrnsRuntimeStopOperation,
    completion: (Result<PrnsRuntimeStop>) -> Unit,
  ) {
    if (!runtime.admission.isStopCurrent(operation) || runtime.service !== this) {
      completion(Result.failure(CancellationException("A newer Stop owns native shutdown")))
      return
    }
    runtime.admission.retireOwner(this)
    bluetoothGeneration.retireListener()
    runtime.update(this, "stopping")
    // The async entrypoint fences native startup now; its drain waiter must not
    // occupy this executor. Release physical transport only after acknowledgement.
    prnsStopAndClearInput(runtime.lifecycle, delegate, operation.reset) { result, clearFailure ->
      if (!runtime.admission.isStopCurrent(operation) || runtime.service !== this) {
        completion(Result.failure(CancellationException("A newer Stop owns native shutdown")))
        return@prnsStopAndClearInput
      }
      val drained = result.mapCatching { outcome ->
        val complete = outcome.stopped && drainTransport()
        if (clearFailure != null) throw clearFailure
        if (complete) {
          engineRunning = false
          reconnecting = false
          bluetoothGeneration.nativeStopped()
          finishing = true
          runtime.admission.completeStop(operation, drained = true)
          releaseDestroyedOwner()
          runtime.update(this, "stopped")
          stopForeground(STOP_FOREGROUND_REMOVE)
          stopSelf()
        } else runtime.update(this, "stopping", "Native shutdown has not completed")
        // A native-only acknowledgement cannot claim unfinished SDK drain.
        outcome.copy(stopped = complete)
      }
      drained.exceptionOrNull()?.let { runtime.update(this, "stopping", it.message) }
      completion(drained)
    }
  }

  private fun drainTransport(): Boolean {
    if (bluetooth?.stop() == false) return false
    bluetooth = null
    if (lanRuntime?.stop() == false) return false
    lanRuntime = null
    if (bridgePrepared && !PrnsBluetoothLifecycle.nativeReleaseBluetooth()) return false
    bridgePrepared = false
    return true
  }

  override fun onDestroy() {
    serviceDestroyed = true
    runtime.admission.retireOwner(this)
    if (!ownsRuntime) {
      super.onDestroy()
      return
    }
    // Android may destroy the service while its Start or Stop is still awaiting
    // native completion. Fence immediately, then join drain on the same lane.
    if (finishing) runtime.lifecycle.execute { releaseDestroyedOwner() }
    else {
      val operation = beginStop(runtime)
      runtime.lifecycle.execute {
        try {
          stopNative(operation) { result ->
            result.exceptionOrNull()?.takeUnless { it is CancellationException }?.let {
              runtime.update(this, "stopping", it.message)
            }
          }
        } catch (error: Exception) { runtime.update(this, "stopping", error.message) }
      }
    }
    // Schedule native fencing before any potentially blocking platform teardown.
    runCatching { unregisterReceiver(radioReceiver) }
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

    private fun beginStop(runtime: PrnsForegroundRuntime, reset: Boolean = false): PrnsRuntimeStopOperation {
      val operation = runtime.admission.beginStopOperation(reset)
      operation.cancelledStarts.forEach { id ->
        runtime.requests.remove(id)?.reject("ERR_PRNS_CANCELLED", "Host start was cancelled by Stop or Reset", null)
      }
      return operation
    }

    fun stopRuntime(context: Context, runtime: PrnsForegroundRuntime, delegate: PrnsRuntimeDelegate, reset: Boolean, promise: Promise?) {
      val operation = beginStop(runtime, reset)
      runtime.lifecycle.execute {
        val service = runtime.service
        // Completion semantics belong to the retained owner, even if the caller
        // supplied a different fallback factory while a service already exists.
        val responseDelegate = service?.delegate ?: delegate
        val completion: (Result<PrnsRuntimeStop>) -> Unit = { result ->
          prnsStopResponse(responseDelegate, result).fold(
            onSuccess = { promise?.resolve(it) },
            onFailure = { error ->
              if (error !is CancellationException) runtime.update(context, "stopping", error.message)
              promise?.reject("ERR_PRNS_STOP", error.message, error)
            },
          )
        }
        try {
          if (!runtime.admission.isStopCurrent(operation)) {
            completion(Result.failure(CancellationException("A newer Stop owns native shutdown")))
            return@execute
          }
          if (service != null) service.stopNative(operation, completion)
          else {
            runtime.update(context, "stopping")
            prnsStopAndClearInput(runtime.lifecycle, delegate, operation.reset) { result, clearFailure ->
              if (!runtime.admission.isStopCurrent(operation)) {
                completion(Result.failure(CancellationException("A newer Stop owns native shutdown")))
                return@prnsStopAndClearInput
              }
              val drained = result.mapCatching { outcome ->
                val complete = outcome.stopped && PrnsBluetoothLifecycle.nativeReleaseBluetooth()
                if (clearFailure != null) throw clearFailure
                runtime.admission.completeStop(operation, drained = complete)
                runtime.update(context, if (complete) "stopped" else "stopping")
                outcome.copy(stopped = complete)
              }
              completion(drained)
            }
          }
        } catch (error: Exception) { completion(Result.failure(error)) }
      }
    }
  }
}
