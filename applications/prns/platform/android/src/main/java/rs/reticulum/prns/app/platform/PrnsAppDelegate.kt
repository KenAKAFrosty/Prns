package rs.reticulum.prns.app.platform

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.graphics.drawable.Icon
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.withLock
import rs.reticulum.prns.chat.app.AppBootstrapException
import rs.reticulum.prns.chat.app.appOpenExisting
import rs.reticulum.prns.chat.app.appStartNew
import rs.reticulum.prns.chat.app.appStop
import rs.reticulum.prns.chat.app.appOwner
import rs.reticulum.prns.expo.PrnsBluetoothRuntimeDelegate
import rs.reticulum.prns.expo.PrnsAsyncRuntimeDelegate
import rs.reticulum.prns.expo.PrnsBluetoothEnvironment
import rs.reticulum.prns.expo.PrnsRuntimeStart
import rs.reticulum.prns.expo.PrnsRuntimeStop
import rs.reticulum.prns.host.bindings.CommandSettlement
import rs.reticulum.prns.host.bindings.HostCommand
import rs.reticulum.prns.host.bindings.HostClientHandle
import rs.reticulum.prns.host.bindings.InterfaceConfig
import rs.reticulum.prns.host.bindings.InterfaceKind
import uniffi.prns_service_runtime_uniffi.ServiceRuntime
import uniffi.prns_service_runtime.ServiceRuntimeNetworkPhase

internal class PrnsAppDelegate(context: Context) : PrnsAsyncRuntimeDelegate, PrnsBluetoothRuntimeDelegate {
  private val application = context.applicationContext
  override val bluetoothEnabled: Boolean get() = PrnsAppProcess.bluetooth(application)
  override val notificationId = 7411
  override fun notification(service: Service, stop: PendingIntent): Notification {
    val manager = service.getSystemService(NotificationManager::class.java)
    manager.createNotificationChannel(NotificationChannel("prns-connection", "PRNS connection", NotificationManager.IMPORTANCE_LOW))
    return Notification.Builder(service, "prns-connection")
      .setSmallIcon(service.applicationInfo.icon.takeIf { it != 0 } ?: android.R.drawable.stat_sys_upload)
      .setContentTitle("PRNS is running")
      .setContentText("Connections remain available while the app is in the background.")
      .setOngoing(true)
      .addAction(Notification.Action.Builder(Icon.createWithResource(service, android.R.drawable.ic_media_pause), "Stop", stop).build())
      .build()
  }
  override fun savedInput(): ByteArray? = PrnsAppProcess.saved(application)?.encode()
  override fun saveInput(input: ByteArray) {
    PrnsAppProcess.selection(application).saveInput(PrnsAppInput.decode(input))
  }
  override fun clearInput() { PrnsAppProcess.selection(application).clearInput() }


  override fun start(input: ByteArray, completion: (Result<PrnsRuntimeStart>) -> Unit) {
    val request: PrnsAppInput
    val owner: ServiceRuntime
    try {
      request = PrnsAppInput.decode(input)
      PrnsAppProcess.select(request.bluetooth)
      val root = PrnsAppStorage.root(application, request.create).toString()
      owner = if (request.create) appStartNew(root) else appOpenExisting(root)
    } catch (error: Exception) { completion(Result.failure(error)); return }
    val generation = owner.networkStatus().generation
    // Construction returns the retained owner immediately. The lifecycle lane
    // remains free for Stop while this canonical readiness observer is pending.
    PrnsAppProcess.observers.launch {
      try {
        owner.ready()
        owner.networkReady()
        val host = owner.host()
        requireNetworkGeneration(owner, generation)
        val transportError = try {
          PrnsAppProcess.transportAttachment.withLock {
            if (request.bluetooth) {
              check(PrnsBluetoothEnvironment.hasPermission(application)) { "Bluetooth permission was not granted; saved connections remain available" }
              val groups = PrnsAppProcess.selection(application).discoveryGroups()
              attach(host, InterfaceKind.AUTOMATIC_BLUETOOTH_LE, InterfaceConfig.ConfiguredBluetoothLe(groups))
              requireNetworkGeneration(owner, generation)
              PrnsAppProcess.appliedGroups = groups
            }
          }
          null
        } catch (error: Exception) { error.message ?: error.javaClass.simpleName }
        completion(Result.success(PrnsRuntimeStart(null, running = true, error = transportError)))
      } catch (error: Exception) { completion(Result.failure(error)) }
    }
  }

  private fun requireNetworkGeneration(owner: ServiceRuntime, generation: ULong) {
    val state = owner.networkStatus()
    check(state.generation == generation && state.phase == ServiceRuntimeNetworkPhase.READY) { "Bluetooth change was cancelled by network replacement" }
  }

  private suspend fun attach(host: HostClientHandle, kind: InterfaceKind, config: InterfaceConfig) {
    if (host.snapshot().interfaces.any { it.kind == kind }) return
    val outcome = host.execute(HostCommand.AttachInterface(config, null))
    if (outcome is CommandSettlement.Failed) error(outcome.failure.toString())
  }

  override fun detachBluetooth(completion: (Result<Unit>) -> Unit) {
    val owner: ServiceRuntime
    val host: HostClientHandle
    val generation: ULong
    try {
      owner = appOwner()
      generation = owner.networkStatus().generation
      host = owner.host()
      requireNetworkGeneration(owner, generation)
    } catch (error: Exception) { completion(Result.failure(error)); return }
    PrnsAppProcess.observers.launch {
      completion(runCatching {
        PrnsAppProcess.transportAttachment.withLock {
          requireNetworkGeneration(owner, generation)
          PrnsAppProcess.appliedGroups = null
          for (interfaceRow in host.snapshot().interfaces.filter { it.kind == InterfaceKind.AUTOMATIC_BLUETOOTH_LE }) {
            val outcome = host.execute(HostCommand.DetachInterface(interfaceRow.interfaceId))
            if (outcome is CommandSettlement.Failed) error(outcome.failure.toString())
          }
        }
      })
    }
  }

  override fun attachBluetooth(completion: (Result<Unit>) -> Unit) {
    if (!bluetoothEnabled) { completion(Result.success(Unit)); return }
    val owner: ServiceRuntime
    val host: HostClientHandle
    val generation: ULong
    try {
      owner = appOwner()
      generation = owner.networkStatus().generation
      host = owner.host()
      requireNetworkGeneration(owner, generation)
    } catch (error: Exception) { completion(Result.failure(error)); return }
    PrnsAppProcess.observers.launch {
      completion(runCatching {
        PrnsAppProcess.transportAttachment.withLock {
          check(PrnsBluetoothEnvironment.hasPermission(application)) { "Bluetooth permission was not granted" }
          val groups = PrnsAppProcess.selection(application).discoveryGroups()
          attach(host, InterfaceKind.AUTOMATIC_BLUETOOTH_LE, InterfaceConfig.ConfiguredBluetoothLe(groups))
          requireNetworkGeneration(owner, generation)
          PrnsAppProcess.appliedGroups = groups
        }
      })
    }
  }

  override fun stop(reset: Boolean, completion: (Result<PrnsRuntimeStop>) -> Unit) {
    // Platform Stop preserves the installation; reviewed deletion is a separate operation.
    synchronized(PrnsAppProcess.startStopGate) { PrnsAppProcess.permissionEpoch.incrementAndGet() }
    PrnsAppProcess.appliedGroups = null
    val close = try { appStop() }
    catch (_: AppBootstrapException.NotStarted) {
      PrnsAppProcess.released()
      completion(Result.success(PrnsRuntimeStop(null, stopped = true))); return
    } catch (error: Exception) { completion(Result.failure(error)); return }
    PrnsAppProcess.observers.launch {
      try {
        close.wait()
        val drained = (close.status().phase == ServiceRuntimeNetworkPhase.OFFLINE)
        if (drained) PrnsAppProcess.released()
        completion(Result.success(PrnsRuntimeStop(null, stopped = drained)))
      } catch (error: Exception) {
        // Only the canonical offline settlement permits physical transport teardown.
        if ((close.status().phase == ServiceRuntimeNetworkPhase.OFFLINE)) {
          PrnsAppProcess.released()
          completion(Result.success(PrnsRuntimeStop(null, stopped = true)))
        } else { completion(Result.failure(error)) }
      }
    }
  }
}
