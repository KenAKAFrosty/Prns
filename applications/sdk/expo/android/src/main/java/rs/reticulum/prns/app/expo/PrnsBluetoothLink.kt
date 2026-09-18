// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors
//
// Adapted from personal-hopspot/mobile/android/app/src/main/java/org/personal/hopspot/
// BleLink.kt and NativeBridge.kt. This is Android transport glue for the app-owned
// Rust runtime, not a second Hopspot runtime. Retains the upstream dual license;
// see LICENSE-MIT and LICENSE-APACHE in the Prns repository.

package rs.reticulum.prns.app.expo

import android.Manifest
import android.content.pm.PackageManager
import android.location.LocationManager
import android.annotation.SuppressLint
import android.annotation.TargetApi
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothServerSocket
import android.bluetooth.BluetoothSocket
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.os.Build
import android.os.ParcelUuid
import android.util.Log
import java.nio.ByteBuffer
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArraySet
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

@TargetApi(Build.VERSION_CODES.Q)
@SuppressLint("MissingPermission")
class PrnsBluetoothLink(
    context: Context,
    private val onStatus: (Status) -> Unit = {},
    private val onPsmPublished: (Int) -> Unit = {},
) {
    enum class State { Stopped, Starting, WaitingForRuntime, Active, Unavailable, Failed, Stopping }

    data class Status(val state: State, val reason: String? = null)

    @Volatile
    var status = Status(State.Stopped)
        private set

    private val context = context.applicationContext
    private val lifecycleLock = ReentrantLock()
    private var used = false
    private var radioFailure: String? = null
    private val radioCycle = PrnsBluetoothRadioCycle()
    private var serviceReady = false
    private var advertisingStarted = false
    private var radioEpoch = 0L
    private var activeScanCallback: ScanCallback? = null
    private var activeAdvertiseCallback: AdvertiseCallback? = null
    @Volatile
    private var appForeground = false

    /** Feed actual Activity visibility; a foreground service is not a visible Activity. */
    fun setAppForeground(foreground: Boolean) {
        if (appForeground == foreground) return
        appForeground = foreground
        // Visibility changes must not block the service behind a slow callback.
        // The bounded radio poll also notices the flag if this wake is skipped.
        if (!lifecycleLock.tryLock()) return
        try {
            if (running && owner.get() === this) PrnsBluetoothNative.nativeBleWakePumps()
        } finally {
            lifecycleLock.unlock()
        }
    }

    private fun canDiscover(): Boolean = prnsBluetoothDiscoveryAllowed(
        Build.VERSION.SDK_INT,
        appForeground,
        context.checkSelfPermission(Manifest.permission.ACCESS_BACKGROUND_LOCATION) ==
            PackageManager.PERMISSION_GRANTED,
    )

    private fun report(state: State, reason: String? = null) {
        val next = Status(state, reason)
        if (status == next) return
        status = next
        // Service callers must enqueue their response, not block a Bluetooth callback.
        runCatching { onStatus(next) }.onFailure {
            Log.w(TAG, "Bluetooth status observer failed", it)
        }
    }

    private fun unavailableReason(): String? {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return "Bluetooth requires Android 10 or newer."
        }
        if (requiredPermissions().any {
                context.checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED
            }
        ) {
            return "Bluetooth permissions have not been granted."
        }
        val radio = adapter ?: return "Bluetooth is unavailable on this device."
        if (!radio.isEnabled) return "Bluetooth is turned off."
        if (!radio.isMultipleAdvertisementSupported) {
            return "This device does not support Bluetooth peripheral advertising."
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            val location = context.getSystemService(Context.LOCATION_SERVICE) as? LocationManager
            if (location?.isLocationEnabled != true) {
                return "Turn on Location to discover Bluetooth devices on this Android version."
            }
        }
        return null
    }

    private fun failRadio(reason: String) {
        radioFailure = reason
        stopRadio()
        PrnsBluetoothNative.nativeBleWakePumps()
        report(State.Failed, reason)
    }

    private fun reportRadioState() {
        if (!running || !radioActive || radioFailure != null) return
        val discoveryPaused = scanningWanted && !canDiscover()
        if ((!scanningWanted || discoveryPaused || scanner != null) &&
            (!advertisingWanted || serviceReady && advertisingStarted)
        ) {
            // This reports local transport readiness, never a connected peer.
            report(
                State.Active,
                if (discoveryPaused) {
                    "Background Bluetooth discovery is paused; open the app or allow background discovery."
                } else null,
            )
        } else {
            report(State.Starting)
        }
    }

    private inline fun withCallback(epoch: Long, action: () -> Unit) {
        lifecycleLock.withLock {
            // stop() drains this lock before releasing the process-wide JNI owner.
            if (!running || owner.get() !== this || radioEpoch != epoch) return
            try {
                action()
            } catch (_: SecurityException) {
                failRadio("Bluetooth permission was revoked.")
            } catch (error: RuntimeException) {
                Log.w(TAG, "Bluetooth callback failed", error)
                failRadio("Android could not complete a Bluetooth operation.")
            }
        }
    }

    private val bluetoothManager =
        context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
    private val adapter: BluetoothAdapter? = bluetoothManager?.adapter
    private val peerCapacity = PrnsBluetoothNative.nativeBlePeerCapacity().coerceAtLeast(1)
    private val deviceCacheCapacity = 2 * peerCapacity

    @Volatile
    private var scanner: BluetoothLeScanner? = null

    @Volatile
    private var advertiser: BluetoothLeAdvertiser? = null

    @Volatile
    private var gattServer: BluetoothGattServer? = null

    @Volatile
    private var controlChar: BluetoothGattCharacteristic? = null

    @Volatile
    private var dataChar: BluetoothGattCharacteristic? = null

    @Volatile
    private var columbaRxChar: BluetoothGattCharacteristic? = null

    @Volatile
    private var columbaTxChar: BluetoothGattCharacteristic? = null

    @Volatile
    private var columbaIdentityChar: BluetoothGattCharacteristic? = null

    @Volatile
    private var l2capServer: BluetoothServerSocket? = null

    @Volatile
    private var running = false

    @Volatile
    private var radioActive = false

    @Volatile
    private var advertisingWanted = false

    @Volatile
    private var scanningWanted = false

    private val nextConnId = AtomicInteger(1)
    private val links = ConcurrentHashMap<Int, LinkState>()
    private val inboundByAddr = ConcurrentHashMap<String, Int>()
    private val columbaSubscribedCentrals = ConcurrentHashMap<String, BluetoothDevice>()
    private val devices = ConcurrentHashMap<String, BluetoothDevice>()
    private val workers = CopyOnWriteArraySet<Thread>()
    private val radioWorkers = CopyOnWriteArraySet<Thread>()
    private val linkWorkers = ConcurrentHashMap<Int, CopyOnWriteArraySet<Thread>>()
    private val l2capOpening = ConcurrentHashMap.newKeySet<Int>()

    private enum class BlePeerProtocol {
        Native,
        Columba,
    }

    private enum class OutboundAdmission {
        Accepted,
        Busy,
        Terminal,
    }

    private class LinkState(
        val connId: Int,
        val address: String,
        val dialed: Boolean,
        @Volatile var peerProtocol: BlePeerProtocol,
    ) {
        val gattState = PrnsGattState()

        @Volatile
        var central: BluetoothDevice? = null

        @Volatile
        var clientGatt: BluetoothGatt? = null

        @Volatile
        var clientControl: BluetoothGattCharacteristic? = null

        @Volatile
        var clientData: BluetoothGattCharacteristic? = null

        @Volatile
        var clientColumbaTx: BluetoothGattCharacteristic? = null

        @Volatile
        var peerIdentity: ByteArray? = null

        @Volatile
        var l2capSocket: BluetoothSocket? = null

        @Volatile
        var openingL2capSocket: BluetoothSocket? = null

        fun beginGattOperation(operation: PendingGattOperation): Boolean = gattState.begin(operation)

        fun completeGattOperation(kind: GattOperationKind, characteristic: UUID? = null): Boolean =
            gattState.complete(kind, characteristic)

        fun cancelGattOperation(operation: PendingGattOperation): Boolean = gattState.cancel(operation)
    }

    private fun scanCallback(epoch: Long) = object : ScanCallback() {
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            withCallback(epoch) {
                if (!running || !radioActive || !scanningWanted || activeScanCallback !== this) {
                    return
                }
                val device = result.device
                if (!rememberDevice(device)) {
                    Log.w(TAG, "sighting cache full; ignored ${device.address}")
                    return
                }
                val octets = parseMac(device.address) ?: return
                if (!shouldDial(octets, result)) {
                    return
                }
                val direct = ByteBuffer.allocateDirect(6)
                direct.put(octets)
                PrnsBluetoothNative.nativeBleSighting(direct, result.rssi)
            }
        }

        override fun onScanFailed(errorCode: Int) {
            withCallback(epoch) {
                if (!scanningWanted || activeScanCallback !== this) return
                failRadio("Bluetooth discovery failed (Android error $errorCode).")
            }
        }
    }

    private fun advertiseCallback(epoch: Long) = object : AdvertiseCallback() {
        override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
            withCallback(epoch) {
                if (!advertisingWanted || activeAdvertiseCallback !== this) return
                advertisingStarted = true
                reportRadioState()
            }
        }

        override fun onStartFailure(errorCode: Int) {
            withCallback(epoch) {
                if (!advertisingWanted || activeAdvertiseCallback !== this) return
                failRadio("Bluetooth advertising failed (Android error $errorCode).")
            }
        }
    }

    private fun gattServerCallback(epoch: Long) = object : BluetoothGattServerCallback() {
        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            withCallback(epoch) {
                if (newState == BluetoothProfile.STATE_CONNECTED) {
                    rememberDevice(device)
                    return
                }
                if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                    val connId = inboundByAddr.remove(device.address) ?: return
                    Log.i(TAG, "listener[$connId] ${device.address} disconnected")
                    closeLink(connId)
                }
            }
        }

        override fun onServiceAdded(status: Int, service: BluetoothGattService) {
            withCallback(epoch) {
                if (status != BluetoothGatt.GATT_SUCCESS) {
                    failRadio("Android could not publish the Bluetooth service (error $status).")
                    return
                }
                serviceReady = true
                val adapter = adapter ?: return
                if (running && radioActive && advertisingWanted) {
                    startAdvertise(adapter)
                }
            }
        }

        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic,
        ) {
            withCallback(epoch) {
                if (!running || !radioActive) {
                    return
                }
                when (characteristic.uuid) {
                    COLUMBA_IDENTITY -> {
                        val identity = localBleIdentity()
                        if (identity != null) {
                            Log.i(TAG, "columba identity read ${device.address}")
                            gattServer?.sendResponse(
                                device,
                                requestId,
                                BluetoothGatt.GATT_SUCCESS,
                                offset,
                                identity,
                            )
                        } else {
                            Log.w(TAG, "columba identity read before local identity was ready")
                            gattServer?.sendResponse(
                                device,
                                requestId,
                                BluetoothGatt.GATT_FAILURE,
                                offset,
                                null,
                            )
                        }
                    }
                    COLUMBA_TX -> {
                        gattServer?.sendResponse(
                            device,
                            requestId,
                            BluetoothGatt.GATT_SUCCESS,
                            offset,
                            ByteArray(0),
                        )
                    }
                    else -> {
                        gattServer?.sendResponse(
                            device,
                            requestId,
                            BluetoothGatt.GATT_FAILURE,
                            offset,
                            null,
                        )
                    }
                }
            }
        }

        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray,
        ) {
            withCallback(epoch) {
                if (!running || !radioActive) {
                    return
                }
                val admission = when (characteristic.uuid) {
                    NATIVE_CONTROL,
                    NATIVE_DATA,
                    -> inboundByAddr[device.address]?.let { connId ->
                        deliverGattInbound(
                            connId,
                            characteristic.uuid == NATIVE_DATA,
                            value,
                        )
                    } ?: PrnsBluetoothNative.BLE_INGRESS_CLOSED
                    COLUMBA_RX -> handleColumbaRxWrite(device, value)
                    else -> {
                        Log.w(TAG, "server write to unknown characteristic ${characteristic.uuid}")
                        PrnsBluetoothNative.BLE_INGRESS_CLOSED
                    }
                }
                if (responseNeeded) {
                    val status = when (admission) {
                        PrnsBluetoothNative.BLE_INGRESS_ACCEPTED -> BluetoothGatt.GATT_SUCCESS
                        PrnsBluetoothNative.BLE_INGRESS_FULL -> ATT_INSUFFICIENT_RESOURCES
                        else -> BluetoothGatt.GATT_FAILURE
                    }
                    gattServer?.sendResponse(device, requestId, status, offset, null)
                }
                if (admission == PrnsBluetoothNative.BLE_INGRESS_CLOSED) {
                    inboundByAddr[device.address]?.let(::closeLink)
                }
            }
        }

        override fun onDescriptorWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            descriptor: BluetoothGattDescriptor,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray,
        ) {
            withCallback(epoch) {
                if (!running || !radioActive) {
                    return
                }
                val subscribing = value.isNotEmpty() && value[0].toInt() != 0
                var responseStatus = BluetoothGatt.GATT_SUCCESS
                if (descriptor.characteristic.uuid == COLUMBA_TX) {
                    if (subscribing) {
                        if (columbaSubscribedCentrals.containsKey(device.address) ||
                            columbaSubscribedCentrals.size < peerCapacity
                        ) {
                            columbaSubscribedCentrals[device.address] = device
                            Log.i(TAG, "columba central ${device.address} subscribed; awaiting identity")
                        } else {
                            responseStatus = ATT_INSUFFICIENT_RESOURCES
                            Log.w(TAG, "columba subscription capacity rejected ${device.address}")
                        }
                    } else {
                        columbaSubscribedCentrals.remove(device.address)
                    }
                } else if (
                    subscribing &&
                    descriptor.characteristic.uuid == NATIVE_CONTROL &&
                    inboundByAddr[device.address] == null
                ) {
                    if (links.size >= peerCapacity) {
                        responseStatus = ATT_INSUFFICIENT_RESOURCES
                    } else {
                        val connId = nextConnId.getAndIncrement()
                        val link = LinkState(connId, device.address, dialed = false, peerProtocol = BlePeerProtocol.Native)
                        link.central = device
                        links[connId] = link
                        inboundByAddr[device.address] = connId
                        Log.i(TAG, "listener[$connId] ${device.address} subscribed")
                        val octets = parseMac(device.address)
                        if (octets == null) {
                            responseStatus = BluetoothGatt.GATT_FAILURE
                            closeLink(connId)
                        } else {
                            val direct = ByteBuffer.allocateDirect(6)
                            direct.put(octets)
                            if (!PrnsBluetoothNative.nativeBleLinkUp(connId, direct, RSSI_NONE, false)) {
                                Log.w(TAG, "listener[$connId] lifecycle admission rejected")
                                responseStatus = ATT_INSUFFICIENT_RESOURCES
                                closeLink(connId)
                            }
                        }
                    }
                }
                if (responseNeeded) {
                    gattServer?.sendResponse(device, requestId, responseStatus, offset, null)
                }
            }
        }

        override fun onNotificationSent(device: BluetoothDevice, status: Int) {
            withCallback(epoch) {
                if (!running || !radioActive) {
                    return
                }
                val connId = inboundByAddr[device.address] ?: return
                val link = links[connId] ?: return
                if (link.completeGattOperation(GattOperationKind.ServerNotify)) {
                    PrnsBluetoothNative.nativeBleWakePumps()
                    if (status != BluetoothGatt.GATT_SUCCESS) {
                        Log.w(TAG, "notification failed[$connId] status=$status")
                        closeLink(connId)
                    }
                }
            }
        }
    }

    private fun handleColumbaRxWrite(device: BluetoothDevice, value: ByteArray): Int {
        val address = device.address
        val existingConnId = inboundByAddr[address]
        if (existingConnId == null) {
            if (value.size != COLUMBA_IDENTITY_LEN) {
                Log.w(TAG, "columba RX from $address before identity (${value.size}B), dropping")
                return PrnsBluetoothNative.BLE_INGRESS_CLOSED
            }
            if (links.size >= peerCapacity) return PrnsBluetoothNative.BLE_INGRESS_FULL
            val octets = parseMac(address) ?: return PrnsBluetoothNative.BLE_INGRESS_CLOSED
            val connId = nextConnId.getAndIncrement()
            val link = LinkState(connId, address, dialed = false, peerProtocol = BlePeerProtocol.Columba)
            link.central = columbaSubscribedCentrals[address] ?: device
            link.peerIdentity = value.copyOf()
            links[connId] = link
            inboundByAddr[address] = connId
            Log.i(TAG, "columba listener[$connId] $address identity ${value.size}B")
            val admitted = PrnsBluetoothNative.nativeBleColumbaLinkUp(
                connId,
                directBufferOf(octets),
                RSSI_NONE,
                false,
                directBufferOf(value),
            )
            if (!admitted) {
                Log.w(TAG, "columba listener[$connId] lifecycle admission rejected")
                closeLink(connId)
                return PrnsBluetoothNative.BLE_INGRESS_CLOSED
            }
            return PrnsBluetoothNative.BLE_INGRESS_ACCEPTED
        }

        val link = links[existingConnId]
        if (link?.peerProtocol == BlePeerProtocol.Columba &&
            value.size == COLUMBA_IDENTITY_LEN &&
            link.peerIdentity?.contentEquals(value) == true
        ) {
            Log.i(TAG, "columba listener[$existingConnId] duplicate identity consumed")
            return PrnsBluetoothNative.BLE_INGRESS_ACCEPTED
        }

        return deliverGattInbound(existingConnId, true, value)
    }

    private fun deliverGattInbound(connId: Int, dataLane: Boolean, value: ByteArray): Int {
        val direct = ByteBuffer.allocateDirect(value.size)
        direct.put(value)
        val admission =
            if (dataLane) {
                PrnsBluetoothNative.nativeBleDataIn(connId, direct, value.size)
            } else {
                PrnsBluetoothNative.nativeBleControlIn(connId, direct, value.size)
            }
        if (admission != PrnsBluetoothNative.BLE_INGRESS_ACCEPTED) {
            val reason = if (admission == PrnsBluetoothNative.BLE_INGRESS_FULL) "full" else "closed"
            Log.w(TAG, "inbound Bluetooth LE queue $reason[$connId] ${value.size} B")
        }
        return admission
    }

    private fun localBleIdentity(): ByteArray? {
        val direct = ByteBuffer.allocateDirect(COLUMBA_IDENTITY_LEN)
        val n = PrnsBluetoothNative.nativeBleIdentity(direct)
        if (n != COLUMBA_IDENTITY_LEN) {
            return null
        }
        val out = ByteArray(COLUMBA_IDENTITY_LEN)
        direct.position(0)
        direct.get(out)
        return out
    }

    private fun directBufferOf(bytes: ByteArray): ByteBuffer {
        val direct = ByteBuffer.allocateDirect(bytes.size)
        direct.put(bytes)
        return direct
    }

    private fun rememberDevice(device: BluetoothDevice): Boolean {
        val address = device.address
        if (devices.containsKey(address) || devices.size < deviceCacheCapacity) {
            devices[address] = device
            return true
        }
        val victim = devices.keys.firstOrNull {
            !inboundByAddr.containsKey(it) &&
                !dialingAddrs.contains(it) &&
                !connectedAddrs.contains(it)
        } ?: return false
        devices.remove(victim)
        devices[address] = device
        return true
    }

    /**
     * Starts transport pumps, not a Rust node or a peer connection. Prepare the
     * app's native Bluetooth session first. A stopped instance is never reused.
     */
    fun start(): Boolean = lifecycleLock.withLock {
        if (running) return true
        if (used) {
            report(State.Failed, "A new Bluetooth link is required after shutdown.")
            return false
        }
        val unavailable = try {
            unavailableReason()
        } catch (_: SecurityException) {
            "Bluetooth permissions have not been granted."
        }
        if (unavailable != null) {
            report(State.Unavailable, unavailable)
            return false
        }
        if (!owner.compareAndSet(null, this)) {
            report(State.Failed, "Another Bluetooth session is still active or stopping.")
            return false
        }
        used = true
        running = true
        report(State.Starting)
        startRadioStatePump()
        startControlOutPump()
        startDataOutPump()
        startDialPump()
        startL2capOpenPump()
        true
    }

    private fun startWorker(name: String, task: () -> Unit) {
        startOwnedWorker(name, false, null, task)
    }

    private fun startRadioWorker(name: String, task: () -> Unit) {
        startOwnedWorker(name, true, null, task)
    }

    private fun startLinkWorker(connId: Int, name: String, task: () -> Unit) {
        startOwnedWorker(name, true, connId, task)
    }

    private fun startOwnedWorker(
        name: String,
        radioScoped: Boolean,
        connId: Int?,
        task: () -> Unit,
    ) {
        lifecycleLock.withLock {
            if (!running || radioScoped && !radioActive ||
                connId != null && !links.containsKey(connId)
            ) return
            val worker = Thread {
                try {
                    task()
                } catch (_: InterruptedException) {
                } catch (_: SecurityException) {
                    lifecycleLock.withLock {
                        if (running) failRadio("Bluetooth permission was revoked.")
                    }
                } catch (error: RuntimeException) {
                    Log.w(TAG, "Bluetooth worker failed", error)
                    lifecycleLock.withLock {
                        if (running) failRadio("An Android Bluetooth worker stopped unexpectedly.")
                    }
                } finally {
                    val current = Thread.currentThread()
                    workers.remove(current)
                    radioWorkers.remove(current)
                    if (connId != null) {
                        linkWorkers[connId]?.let { owned ->
                            owned.remove(current)
                            if (owned.isEmpty()) linkWorkers.remove(connId, owned)
                        }
                    }
                }
            }
            worker.name = "PrnsAppBle-$name"
            workers.add(worker)
            if (radioScoped) radioWorkers.add(worker)
            if (connId != null) {
                linkWorkers.computeIfAbsent(connId) { CopyOnWriteArraySet() }.add(worker)
            }
            worker.start()
        }
    }

    private fun startRadioStatePump() {
        startWorker("radio-state") {
            var generation = PrnsBluetoothNative.nativeBleWorkGeneration()
            while (running) {
                // Also recheck permissions/radio while desired Rust state is unchanged.
                applyDesiredRadioState(PrnsBluetoothNative.nativeBleDesiredState())
                generation = PrnsBluetoothNative.nativeBleWaitForWork(generation, RADIO_STATE_RETRY_MS)
            }
        }
    }

    private fun applyDesiredRadioState(state: Int) = lifecycleLock.withLock {
        val wantRadio = (state and PrnsBluetoothNative.BLE_RADIO_ENABLED) != 0
        if (!running) return
        if (!wantRadio) {
            stopRadio()
            radioFailure = null
            report(State.WaitingForRuntime)
            return
        }
        val unavailable = unavailableReason()
        if (unavailable != null) {
            radioCycle.observeAvailable(false)
            stopRadio()
            report(State.Unavailable, unavailable)
            return
        }
        // Only the service may replace a lost listener/native PSM generation.
        // Reopening here races command admission and starts a redundant scan.
        if (!radioCycle.observeAvailable(true)) return
        if (radioFailure != null) return
        advertisingWanted = (state and PrnsBluetoothNative.BLE_RADIO_ADVERTISING) != 0
        scanningWanted = (state and PrnsBluetoothNative.BLE_RADIO_SCANNING) != 0
        val radio = adapter ?: return
        val wasActive = radioActive
        radioActive = true
        if (!startL2capListener(radio)) {
            failRadio("Android could not open the Bluetooth data listener.")
            return
        }
        // Android 10–11 require background location for discovery from a service.
        // Pausing discovery must not tear down already connected peers.
        if (scanningWanted && canDiscover()) startScan(radio) else stopScan()
        if (advertisingWanted) {
            startGattServer()
            if (serviceReady) startAdvertise(radio)
        } else {
            stopAdvertise()
        }
        reportRadioState()
        if (!wasActive && radioActive) PrnsBluetoothNative.nativeBleWakePumps()
    }

    private fun startL2capListener(adapter: BluetoothAdapter): Boolean {
        if (!running || !radioActive || l2capServer != null) {
            return l2capServer != null
        }
        val server = try {
            adapter.listenUsingInsecureL2capChannel()
        } catch (e: Exception) {
            Log.w(TAG, "l2cap listen failed: $e")
            return false
        }
        l2capServer = server
        val psm = server.psm
        if (psm <= 0) {
            runCatching { server.close() }
            l2capServer = null
            return false
        }
        Log.i(TAG, "l2cap listener published psm=$psm")
        // The service must restart the native generation if its cached PSM changes.
        // The callback must enqueue that work, never stop Rust under this lock.
        prnsPublishBluetoothPsm(psm, PrnsBluetoothNative::nativeBleSetPsm, onPsmPublished)
        startRadioWorker("l2cap-listener") {
            while (running && radioActive) {
                val socket = try {
                    server.accept()
                } catch (e: Exception) {
                    lifecycleLock.withLock {
                        if (running && radioActive && l2capServer === server) {
                            failRadio("The Android Bluetooth data listener stopped unexpectedly.")
                        }
                    }
                    break
                }
                lifecycleLock.withLock {
                    if (!running || !radioActive || l2capServer !== server) {
                        runCatching { socket.close() }
                        return@withLock
                    }
                    val address = socket.remoteDevice?.address
                    val connId = address?.let { inboundByAddr[it] }
                    val link = connId?.let { links[it] }
                    if (connId == null || link == null || link.l2capSocket != null) {
                        Log.w(TAG, "L2CAP accept without an active listener link, dropping")
                        runCatching { socket.close() }
                        return@withLock
                    }
                    link.l2capSocket = socket
                    PrnsBluetoothNative.nativeBleL2capUp(connId)
                    startL2capPumps(connId, socket)
                }
            }
        }
        return true
    }

    private fun startL2capPumps(connId: Int, socket: BluetoothSocket) {
        startLinkWorker(connId, "l2cap-in-$connId") {
            val input = socket.inputStream
            val buf = ByteArray(L2CAP_CHUNK)
            val direct = ByteBuffer.allocateDirect(L2CAP_CHUNK)
            while (running && radioActive && links.containsKey(connId)) {
                val n = try {
                    input.read(buf)
                } catch (e: Exception) {
                    break
                }
                if (n < 0) break
                if (n > 0) {
                    direct.clear()
                    direct.put(buf, 0, n)
                    if (!PrnsBluetoothNative.nativeBleL2capIn(connId, direct, n)) {
                        Log.w(TAG, "inbound L2CAP queue full or closed[$connId] ${n}B")
                        break
                    }
                }
            }
            closeLink(connId)
        }
        startLinkWorker(connId, "l2cap-out-$connId") {
            val output = socket.outputStream
            val direct = ByteBuffer.allocateDirect(L2CAP_CHUNK)
            val scratch = ByteArray(L2CAP_CHUNK)
            var generation = PrnsBluetoothNative.nativeBleWorkGeneration()
            while (running && radioActive && links.containsKey(connId)) {
                direct.clear()
                val n = PrnsBluetoothNative.nativeBleL2capOut(connId, direct)
                if (n > 0) {
                    direct.position(0)
                    direct.get(scratch, 0, n)
                    try {
                        output.write(scratch, 0, n)
                        output.flush()
                    } catch (e: Exception) {
                        break
                    }
                } else {
                    generation = PrnsBluetoothNative.nativeBleWaitForWork(generation, 0)
                }
            }
        }
    }

    private fun startControlOutPump() {
        startWorker("control-out") {
            val direct = ByteBuffer.allocateDirect(CONTROL_CHUNK)
            val scratch = ByteArray(CONTROL_CHUNK)
            var generation = PrnsBluetoothNative.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = PrnsBluetoothNative.nativeBleWaitForWork(generation, 0)
                    continue
                }
                var pending = false
                var progressed = false
                for (link in links.values) {
                    direct.clear()
                    val n = PrnsBluetoothNative.nativeBleControlOut(link.connId, direct)
                    if (n > 0) {
                        pending = true
                        direct.position(0)
                        direct.get(scratch, 0, n)
                        when (deliverControl(link, scratch.copyOf(n))) {
                            OutboundAdmission.Accepted -> {
                                progressed = PrnsBluetoothNative.nativeBleCommitControlOut(link.connId)
                                if (!progressed) {
                                    Log.w(TAG, "control ownership commit failed[${link.connId}]")
                                    closeLink(link.connId)
                                }
                            }
                            OutboundAdmission.Busy -> {}
                            OutboundAdmission.Terminal -> closeLink(link.connId)
                        }
                    }
                }
                if (!progressed) {
                    generation = PrnsBluetoothNative.nativeBleWaitForWork(
                        generation,
                        if (pending) GATT_BUSY_RETRY_MS else 0,
                    )
                }
            }
        }
    }

    private fun deliverControl(link: LinkState, payload: ByteArray): OutboundAdmission {
        if (link.peerProtocol == BlePeerProtocol.Columba) {
            Log.w(TAG, "control queued for Columba link[${link.connId}]")
            return OutboundAdmission.Terminal
        }
        if (link.dialed) {
            val char = link.clientControl ?: return OutboundAdmission.Terminal
            return clientWriteAdmission(
                link,
                char,
                payload,
                BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT,
                "control",
            )
        }
        val char = controlChar ?: return OutboundAdmission.Terminal
        return serverNotifyAdmission(link, char, payload, "control")
    }

    private fun clientWriteAdmission(
        link: LinkState,
        char: BluetoothGattCharacteristic,
        payload: ByteArray,
        type: Int,
        lane: String,
    ): OutboundAdmission {
        val gatt = link.clientGatt ?: return OutboundAdmission.Terminal
        val responseBearing = type == BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
        val operation = PendingGattOperation(GattOperationKind.ClientWrite, char.uuid)
        if (responseBearing && !link.beginGattOperation(operation)) {
            return OutboundAdmission.Busy
        }
        val result = try {
            writeGattCharacteristic(gatt, char, payload, type)
        } catch (e: Exception) {
            if (responseBearing) {
                link.cancelGattOperation(operation)
            }
            Log.w(TAG, "$lane write[${link.connId}]: $e")
            return OutboundAdmission.Terminal
        }
        if (result == BluetoothGatt.GATT_SUCCESS) {
            return OutboundAdmission.Accepted
        }
        if (responseBearing) {
            link.cancelGattOperation(operation)
        }
        if (result == ERROR_GATT_WRITE_REQUEST_BUSY) {
            return OutboundAdmission.Busy
        }
        Log.w(TAG, "$lane write rejected[${link.connId}] result=$result")
        return OutboundAdmission.Terminal
    }

    private fun serverNotifyAdmission(
        link: LinkState,
        char: BluetoothGattCharacteristic,
        payload: ByteArray,
        lane: String,
    ): OutboundAdmission {
        val central = link.central ?: return OutboundAdmission.Terminal
        val server = gattServer ?: return OutboundAdmission.Terminal
        val operation = PendingGattOperation(GattOperationKind.ServerNotify, char.uuid)
        if (!link.beginGattOperation(operation)) {
            return OutboundAdmission.Busy
        }
        val result = try {
            notifyGattCharacteristic(server, central, char, payload)
        } catch (e: Exception) {
            link.cancelGattOperation(operation)
            Log.w(TAG, "$lane notify[${link.connId}]: $e")
            return OutboundAdmission.Terminal
        }
        if (result == BluetoothGatt.GATT_SUCCESS) {
            return OutboundAdmission.Accepted
        }
        link.cancelGattOperation(operation)
        if (result == ERROR_GATT_WRITE_REQUEST_BUSY) {
            return OutboundAdmission.Busy
        }
        Log.w(TAG, "$lane notify rejected[${link.connId}] result=$result")
        return OutboundAdmission.Terminal
    }

    private fun startDataOutPump() {
        startWorker("data-out") {
            val direct = ByteBuffer.allocateDirect(DATA_CHUNK)
            val scratch = ByteArray(DATA_CHUNK)
            var generation = PrnsBluetoothNative.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = PrnsBluetoothNative.nativeBleWaitForWork(generation, 0)
                    continue
                }
                var pending = false
                var progressed = false
                for (link in links.values) {
                    direct.clear()
                    val n = PrnsBluetoothNative.nativeBleDataOut(link.connId, direct)
                    if (n > 0) {
                        pending = true
                        Log.i(TAG, "data out[${link.connId}] ${n}B")
                        direct.position(0)
                        direct.get(scratch, 0, n)
                        when (deliverData(link, scratch.copyOf(n))) {
                            OutboundAdmission.Accepted -> {
                                progressed = PrnsBluetoothNative.nativeBleCommitDataOut(link.connId)
                                if (!progressed) {
                                    Log.w(TAG, "data ownership commit failed[${link.connId}]")
                                    closeLink(link.connId)
                                }
                            }
                            OutboundAdmission.Busy -> {}
                            OutboundAdmission.Terminal -> closeLink(link.connId)
                        }
                    }
                }
                if (!progressed) {
                    generation = PrnsBluetoothNative.nativeBleWaitForWork(
                        generation,
                        if (pending) GATT_BUSY_RETRY_MS else 0,
                    )
                }
            }
        }
    }

    private fun deliverData(link: LinkState, payload: ByteArray): OutboundAdmission {
        if (link.dialed) {
            val char = link.clientData ?: return OutboundAdmission.Terminal
            val writeType =
                if (link.peerProtocol == BlePeerProtocol.Columba && payload.size == COLUMBA_IDENTITY_LEN) {
                    BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
                } else {
                    BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE
                }
            return clientWriteAdmission(link, char, payload, writeType, "data")
        }
        val char = if (link.peerProtocol == BlePeerProtocol.Columba) columbaTxChar else dataChar
        char ?: return OutboundAdmission.Terminal
        return serverNotifyAdmission(link, char, payload, "data")
    }

    private fun startDialPump() {
        startWorker("dial") {
            val direct = ByteBuffer.allocateDirect(6)
            val octets = ByteArray(6)
            var generation = PrnsBluetoothNative.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = PrnsBluetoothNative.nativeBleWaitForWork(generation, 0)
                    continue
                }
                // Policy Reject queues closes before redials; drain them first so inbound
                // occupancy does not block the opener from dialing as central.
                var closed = false
                while (true) {
                    val connId = PrnsBluetoothNative.nativeBleNextClose()
                    if (connId < 0) {
                        break
                    }
                    Log.i(TAG, "policy close[$connId]")
                    closeLink(connId)
                    closed = true
                }
                if (closed) {
                    continue
                }
                direct.clear()
                if (!PrnsBluetoothNative.nativeBleNextDial(direct)) {
                    generation = PrnsBluetoothNative.nativeBleWaitForWork(generation, 0)
                    continue
                }
                direct.position(0)
                direct.get(octets, 0, 6)
                val address = formatMac(octets)
                if (inboundByAddr.containsKey(address) ||
                    dialingAddrs.contains(address) ||
                    connectedAddrs.contains(address)
                ) {
                    continue
                }
                val device = devices[address]
                if (device == null) {
                    Log.w(TAG, "dial $address requested but device not sighted")
                    continue
                }
                lifecycleLock.withLock {
                    // Register the physical handle before stop() can snapshot resources.
                    if (!running || !radioActive) return@withLock
                    if (links.size >= peerCapacity) {
                        PrnsBluetoothNative.nativeBleDialFailed(directBufferOf(octets))
                        return@withLock
                    }
                    dialingAddrs.add(address)
                    val connId = nextConnId.getAndIncrement()
                    val link = LinkState(connId, address, dialed = true, peerProtocol = BlePeerProtocol.Native)
                    links[connId] = link
                    val gatt = device.connectGatt(
                        context, false, clientCallback(connId, address), BluetoothDevice.TRANSPORT_LE,
                    )
                    link.clientGatt = gatt
                    if (gatt == null) {
                        PrnsBluetoothNative.nativeBleDialFailed(directBufferOf(octets))
                        closeLink(connId)
                    }
                }
            }
        }
    }

    private fun startL2capOpenPump() {
        startWorker("l2cap-open") {
            val direct = ByteBuffer.allocateDirect(6)
            val raw = ByteArray(6)
            var generation = PrnsBluetoothNative.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = PrnsBluetoothNative.nativeBleWaitForWork(generation, 0)
                    continue
                }
                direct.clear()
                if (!PrnsBluetoothNative.nativeBleNextL2capOpen(direct)) {
                    generation = PrnsBluetoothNative.nativeBleWaitForWork(generation, 0)
                    continue
                }
                direct.position(0)
                direct.get(raw, 0, 6)
                val connId = ((raw[0].toInt() and 0xff) shl 24) or
                    ((raw[1].toInt() and 0xff) shl 16) or
                    ((raw[2].toInt() and 0xff) shl 8) or
                    (raw[3].toInt() and 0xff)
                val psm = ((raw[4].toInt() and 0xff) shl 8) or (raw[5].toInt() and 0xff)
                val link = links[connId]
                val device = link?.let { it.central ?: it.clientGatt?.device ?: devices[it.address] }
                if (device == null) {
                    Log.w(TAG, "l2cap open[$connId] psm=$psm but no device")
                    continue
                }
                if (!l2capOpening.add(connId)) {
                    continue
                }
                startLinkWorker(connId, "l2cap-open-$connId") {
                    var attempt = 0
                    var opened = false
                    try {
                        while (attempt < L2CAP_OPEN_RETRIES &&
                            !opened &&
                            running &&
                            radioActive &&
                            links.containsKey(connId) &&
                            link.l2capSocket == null
                        ) {
                            val socket = try {
                                device.createInsecureL2capChannel(psm)
                            } catch (e: Exception) {
                                attempt++
                                Log.w(TAG, "l2cap client[$connId] psm=$psm attempt=$attempt failed: $e")
                                if (attempt < L2CAP_OPEN_RETRIES) {
                                    Thread.sleep(L2CAP_OPEN_RETRY_MS)
                                }
                                continue
                            }
                            val admitted = lifecycleLock.withLock {
                                if (!running || !radioActive || links[connId] !== link) {
                                    false
                                } else {
                                    link.openingL2capSocket = socket
                                    true
                                }
                            }
                            if (!admitted) {
                                runCatching { socket.close() }
                                break
                            }
                            try {
                                socket.connect()
                                lifecycleLock.withLock {
                                    if (!running || !radioActive || links[connId] !== link ||
                                        link.l2capSocket != null
                                    ) {
                                        runCatching { socket.close() }
                                    } else {
                                        link.openingL2capSocket = null
                                        link.l2capSocket = socket
                                        PrnsBluetoothNative.nativeBleL2capUp(connId)
                                        startL2capPumps(connId, socket)
                                        opened = true
                                    }
                                }
                            } catch (e: Exception) {
                                attempt++
                                runCatching { socket.close() }
                                Log.w(TAG, "l2cap client[$connId] psm=$psm attempt=$attempt failed: $e")
                                if (attempt < L2CAP_OPEN_RETRIES) {
                                    Thread.sleep(L2CAP_OPEN_RETRY_MS)
                                }
                            } finally {
                                if (link.openingL2capSocket === socket) {
                                    link.openingL2capSocket = null
                                }
                            }
                        }
                    } finally {
                        l2capOpening.remove(connId)
                    }
                    if (!opened && running && radioActive && links.containsKey(connId) &&
                        link.l2capSocket == null
                    ) {
                        Log.w(TAG, "l2cap client[$connId] psm=$psm gave up after $attempt attempts; staying on GATT")
                    }
                }
            }
        }
    }

    private fun clientCallback(connId: Int, address: String): BluetoothGattCallback =
        object : PrnsBluetoothGattCallback() {
            private val epoch = radioEpoch

            override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
                withCallback(epoch) {
                    if (!running || !radioActive) {
                        runCatching { gatt.disconnect() }
                        runCatching { gatt.close() }
                        closeLink(connId)
                        return
                    }
                    if (newState == BluetoothProfile.STATE_CONNECTED) {
                        val link = links[connId] ?: run {
                            runCatching { gatt.disconnect() }
                            runCatching { gatt.close() }
                            return
                        }
                        link.clientGatt = gatt
                        connectedAddrs.add(address)
                        if (!link.gattState.beginStartup(System.nanoTime() / 1_000_000)) return
                        scheduleClientOpenWatchdog(connId, address, gatt)
                        val mtuOperation = PendingGattOperation(GattOperationKind.Mtu, null)
                        // Failed admission is not a rejected Android request: a duplicate
                        // connection callback must never release an existing MTU operation.
                        if (!link.beginGattOperation(mtuOperation)) return
                        val mtuRequested = runCatching {
                            gatt.requestMtu(MAX_ATT_MTU)
                        }.getOrDefault(false)
                        Log.i(TAG, "dialer[$connId] connected; requested mtu=$mtuRequested")
                        if (!mtuRequested) {
                            link.cancelGattOperation(mtuOperation)
                            requestClientServices(gatt, link, "mtu request rejected")
                        }
                    } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                        Log.i(TAG, "dialer[$connId] $address disconnected status=$status")
                        if (!linkedConnIds.remove(connId)) {
                            parseMac(address)?.let { octets ->
                                val direct = ByteBuffer.allocateDirect(6)
                                direct.put(octets)
                                if (!PrnsBluetoothNative.nativeBleDialFailed(direct)) {
                                    Log.w(TAG, "dialer[$connId] failure event admission rejected")
                                }
                            }
                        }
                        dialingAddrs.remove(address)
                        connectedAddrs.remove(address)
                        runCatching { gatt.disconnect() }
                        runCatching { gatt.close() }
                        closeLink(connId)
                    }
                }
            }

            override fun onMtuChanged(gatt: BluetoothGatt, mtu: Int, status: Int) {
                withCallback(epoch) {
                    if (!running || !radioActive) {
                        return
                    }
                    val link = links[connId] ?: return
                    if (!link.completeGattOperation(GattOperationKind.Mtu)) return
                    Log.i(TAG, "dialer[$connId] att mtu=$mtu status=$status")
                    requestClientServices(gatt, link, "mtu changed")
                }
            }

            override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
                withCallback(epoch) {
                    if (!running || !radioActive) {
                        return
                    }
                    if (links[connId]?.completeGattOperation(GattOperationKind.ServiceDiscovery) != true) {
                        return
                    }
                    if (status != BluetoothGatt.GATT_SUCCESS) {
                        Log.w(TAG, "dialer[$connId] service discovery failed status=$status")
                        runCatching { gatt.disconnect() }
                        return
                    }
                    val service = gatt.getService(PRNS_SERVICE)
                    if (service == null) {
                        Log.w(TAG, "dialer[$connId] no Prns service")
                        runCatching { gatt.disconnect() }
                        return
                    }
                    val nativeControl = service.getCharacteristic(NATIVE_CONTROL)
                    if (nativeControl != null) {
                        val nativeData = service.getCharacteristic(NATIVE_DATA)
                        links[connId]?.clientControl = nativeControl
                        links[connId]?.clientData = nativeData
                        if (nativeData != null) {
                            gatt.setCharacteristicNotification(nativeData, true)
                        }
                        gatt.setCharacteristicNotification(nativeControl, true)
                        val cccd = nativeControl.getDescriptor(CCCD)
                        if (cccd != null) {
                            if (!startDescriptorWrite(connId, gatt, cccd)) {
                                runCatching { gatt.disconnect() }
                            }
                        }
                        return
                    }
    
                    val columbaRx = service.getCharacteristic(COLUMBA_RX)
                    val columbaTx = service.getCharacteristic(COLUMBA_TX)
                    val columbaIdentity = service.getCharacteristic(COLUMBA_IDENTITY)
                    if (columbaRx == null || columbaTx == null || columbaIdentity == null) {
                        Log.w(TAG, "dialer[$connId] no native or Columba characteristic set")
                        runCatching { gatt.disconnect() }
                        return
                    }
                    links[connId]?.apply {
                        peerProtocol = BlePeerProtocol.Columba
                        clientData = columbaRx
                        clientColumbaTx = columbaTx
                    }
                    Log.i(TAG, "dialer[$connId] found Columba profile; reading identity")
                    if (!startCharacteristicRead(connId, gatt, columbaIdentity)) {
                        Log.w(TAG, "dialer[$connId] Columba identity read did not start")
                        runCatching { gatt.disconnect() }
                    }
                }
            }

            override fun onCharacteristicRead(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
                status: Int,
            ) {
                withCallback(epoch) {
                    handleClientCharacteristicRead(connId, address, gatt, characteristic, value, status)
                }
            }

            @Suppress("DEPRECATION")
            override fun onCharacteristicRead(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int,
            ) {
                withCallback(epoch) {
                    handleClientCharacteristicRead(
                        connId,
                        address,
                        gatt,
                        characteristic,
                        characteristic.value ?: ByteArray(0),
                        status,
                    )
                }
            }

            private fun handleClientCharacteristicRead(
                connId: Int,
                address: String,
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
                status: Int,
            ) {
                if (!running || !radioActive) {
                    return
                }
                if (characteristic.uuid != COLUMBA_IDENTITY) {
                    return
                }
                if (links[connId]?.completeGattOperation(
                    GattOperationKind.CharacteristicRead,
                    characteristic.uuid,
                ) != true) return
                if (status != BluetoothGatt.GATT_SUCCESS || value.size != COLUMBA_IDENTITY_LEN) {
                    Log.w(
                        TAG,
                        "dialer[$connId] Columba identity read failed status=$status size=${value.size}",
                    )
                    runCatching { gatt.disconnect() }
                    return
                }
                val link = links[connId] ?: return
                val tx = link.clientColumbaTx
                if (tx == null) {
                    Log.w(TAG, "dialer[$connId] Columba TX missing after identity read")
                    runCatching { gatt.disconnect() }
                    return
                }
                link.peerIdentity = value.copyOf()
                gatt.setCharacteristicNotification(tx, true)
                val cccd = tx.getDescriptor(CCCD)
                if (cccd != null) {
                    if (!startDescriptorWrite(connId, gatt, cccd)) {
                        runCatching { gatt.disconnect() }
                    }
                } else {
                    Log.w(TAG, "dialer[$connId] Columba TX CCCD null")
                    runCatching { gatt.disconnect() }
                }
            }

            override fun onDescriptorWrite(
                gatt: BluetoothGatt,
                descriptor: BluetoothGattDescriptor,
                status: Int,
            ) {
                withCallback(epoch) {
                    if (!running || !radioActive) {
                        return
                    }
                    val link = links[connId] ?: return
                    if (!link.completeGattOperation(
                        GattOperationKind.DescriptorWrite,
                        descriptor.characteristic.uuid,
                    )) return
                    Log.i(
                        TAG,
                        "dialer[$connId] cccd ${descriptor.characteristic.uuid} status=$status",
                    )
                    if (status != BluetoothGatt.GATT_SUCCESS) {
                        Log.w(TAG, "dialer[$connId] subscription failed status=$status")
                        runCatching { gatt.disconnect() }
                        return
                    }
                    if (descriptor.characteristic.uuid == COLUMBA_TX) {
                        val identity = link.peerIdentity
                        if (identity == null) {
                            Log.w(TAG, "dialer[$connId] Columba TX subscribe failed status=$status")
                            runCatching { gatt.disconnect() }
                            return
                        }
                        Log.i(TAG, "dialer[$connId] $address subscribed (Columba TX ready)")
                        val octets = parseMac(address)
                        if (octets != null) {
                            if (!link.gattState.markReady()) return
                            linkedConnIds.add(connId)
                            if (!PrnsBluetoothNative.nativeBleColumbaLinkUp(
                                connId,
                                directBufferOf(octets),
                                RSSI_NONE,
                                true,
                                directBufferOf(identity),
                            )) {
                                Log.w(TAG, "dialer[$connId] Columba lifecycle admission rejected")
                                closeLink(connId)
                            }
                        }
                        return
                    }
                    if (descriptor.characteristic.uuid == NATIVE_CONTROL) {
                        val dataCccd = links[connId]?.clientData?.getDescriptor(CCCD)
                        if (dataCccd != null) {
                            if (!startDescriptorWrite(connId, gatt, dataCccd)) {
                                runCatching { gatt.disconnect() }
                            }
                            return
                        }
                        Log.w(TAG, "dialer[$connId] data CCCD null — DATA notifications NOT enabled")
                    }
                    Log.i(TAG, "dialer[$connId] $address subscribed (control + data ready)")
                    val octets = parseMac(address)
                    if (octets != null) {
                        if (!link.gattState.markReady()) return
                        linkedConnIds.add(connId)
                        val direct = ByteBuffer.allocateDirect(6)
                        direct.put(octets)
                        if (!PrnsBluetoothNative.nativeBleLinkUp(connId, direct, RSSI_NONE, true)) {
                            Log.w(TAG, "dialer[$connId] lifecycle admission rejected")
                            closeLink(connId)
                        }
                    }
                }
            }

            override fun onCharacteristicWrite(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int,
            ) {
                withCallback(epoch) {
                    if (!running || !radioActive) {
                        return
                    }
                    val link = links[connId] ?: return
                    if (link.completeGattOperation(GattOperationKind.ClientWrite, characteristic.uuid)) {
                        PrnsBluetoothNative.nativeBleWakePumps()
                        if (status != BluetoothGatt.GATT_SUCCESS) {
                            Log.w(TAG, "write completion failed[$connId] status=$status")
                            closeLink(connId)
                        }
                    }
                }
            }

            override fun onCharacteristicChanged(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
            ) {
                withCallback(epoch) {
                    if (!running || !radioActive) {
                        return
                    }
                    val dataLane = characteristic.uuid == NATIVE_DATA || characteristic.uuid == COLUMBA_TX
                    Log.i(TAG, "dialer[$connId] notify ${if (dataLane) "DATA" else "CONTROL"} ${value.size}B")
                    if (deliverGattInbound(connId, dataLane, value) == PrnsBluetoothNative.BLE_INGRESS_CLOSED) {
                        closeLink(connId)
                    }
                }
            }
        }

    private fun requestClientServices(gatt: BluetoothGatt, link: LinkState, reason: String) {
        if (!link.gattState.beginServiceDiscovery()) return
        val operation = PendingGattOperation(GattOperationKind.ServiceDiscovery, null)
        val started = runCatching { gatt.discoverServices() }.getOrDefault(false)
        Log.i(TAG, "dialer[${link.connId}] discovering services after $reason started=$started")
        if (!started) {
            link.cancelGattOperation(operation)
            runCatching { gatt.disconnect() }
        }
    }

    private fun startCharacteristicRead(
        connId: Int,
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
    ): Boolean {
        val link = links[connId] ?: return false
        val operation = PendingGattOperation(
            GattOperationKind.CharacteristicRead,
            characteristic.uuid,
        )
        if (!link.beginGattOperation(operation)) {
            return false
        }
        if (runCatching { gatt.readCharacteristic(characteristic) }.getOrDefault(false)) {
            return true
        }
        link.cancelGattOperation(operation)
        return false
    }

    private fun startDescriptorWrite(
        connId: Int,
        gatt: BluetoothGatt,
        descriptor: BluetoothGattDescriptor,
    ): Boolean {
        val link = links[connId] ?: return false
        val operation = PendingGattOperation(
            GattOperationKind.DescriptorWrite,
            descriptor.characteristic.uuid,
        )
        if (!link.beginGattOperation(operation)) {
            return false
        }
        val result = runCatching {
            writeGattDescriptor(gatt, descriptor, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
        }.getOrDefault(BluetoothGatt.GATT_FAILURE)
        if (result == BluetoothGatt.GATT_SUCCESS) {
            return true
        }
        link.cancelGattOperation(operation)
        Log.w(TAG, "dialer[$connId] descriptor write rejected result=$result")
        return false
    }

    private fun scheduleClientOpenWatchdog(connId: Int, address: String, gatt: BluetoothGatt) {
        startLinkWorker(connId, "client-watchdog-$connId") {
            // A local timeout cannot cancel Android's MTU request. Wait for its
            // callback, or close the whole unready link at the terminal deadline.
            Thread.sleep(CLIENT_LINK_READY_TIMEOUT_MS)
            val link = links[connId]
            if (running && radioActive && link != null && link.gattState.expireStartup(
                System.nanoTime() / 1_000_000,
                CLIENT_LINK_READY_TIMEOUT_MS,
            )) {
                Log.w(TAG, "dialer[$connId] $address did not become a Prns link; closing stale GATT")
                runCatching { gatt.disconnect() }
                closeLink(connId)
            }
        }
    }

    private fun closeLink(connId: Int) {
        val link = links.remove(connId)
        val current = Thread.currentThread()
        val ownedWorkers = linkWorkers.remove(connId).orEmpty().filter { it !== current }
        if (link == null) {
            ownedWorkers.forEach(Thread::interrupt)
            // A Rust close request owns its slot until this acknowledgement. The physical link may
            // already be gone, but the lifecycle transition still has to complete.
            PrnsBluetoothNative.nativeBleDisconnected(connId)
            return
        }
        link.gattState.close()
        inboundByAddr.remove(link.address, connId)
        columbaSubscribedCentrals.remove(link.address)
        dialingAddrs.remove(link.address)
        connectedAddrs.remove(link.address)
        runCatching { link.openingL2capSocket?.close() }
        runCatching { link.l2capSocket?.close() }
        if (!link.dialed) {
            link.central?.let { central -> runCatching { gattServer?.cancelConnection(central) } }
        }
        runCatching { link.clientGatt?.disconnect() }
        runCatching { link.clientGatt?.close() }
        ownedWorkers.forEach(Thread::interrupt)
        PrnsBluetoothNative.nativeBleDisconnected(connId)
    }

    private fun startGattServer() {
        if (!running || !radioActive || gattServer != null) {
            return
        }
        val manager = bluetoothManager ?: return
        val server = try {
            manager.openGattServer(context, gattServerCallback(radioEpoch))
        } catch (e: SecurityException) {
            failRadio("Bluetooth service permission was denied.")
            return
        } ?: run {
            failRadio("Android could not open the Bluetooth service.")
            return
        }
        gattServer = server
        val control = BluetoothGattCharacteristic(
            NATIVE_CONTROL,
            BluetoothGattCharacteristic.PROPERTY_WRITE or
                BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE or
                BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_WRITE,
        )
        control.addDescriptor(
            BluetoothGattDescriptor(
                CCCD,
                BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE,
            ),
        )
        controlChar = control
        val data = BluetoothGattCharacteristic(
            NATIVE_DATA,
            BluetoothGattCharacteristic.PROPERTY_WRITE or
                BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE or
                BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_WRITE,
        )
        data.addDescriptor(
            BluetoothGattDescriptor(
                CCCD,
                BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE,
            ),
        )
        dataChar = data
        val columbaRx = BluetoothGattCharacteristic(
            COLUMBA_RX,
            BluetoothGattCharacteristic.PROPERTY_WRITE or
                BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE,
            BluetoothGattCharacteristic.PERMISSION_WRITE,
        )
        columbaRxChar = columbaRx
        val columbaTx = BluetoothGattCharacteristic(
            COLUMBA_TX,
            BluetoothGattCharacteristic.PROPERTY_READ or
                BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_READ,
        )
        columbaTx.addDescriptor(
            BluetoothGattDescriptor(
                CCCD,
                BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE,
            ),
        )
        columbaTxChar = columbaTx
        val columbaIdentity = BluetoothGattCharacteristic(
            COLUMBA_IDENTITY,
            BluetoothGattCharacteristic.PROPERTY_READ,
            BluetoothGattCharacteristic.PERMISSION_READ,
        )
        columbaIdentityChar = columbaIdentity
        val service = BluetoothGattService(PRNS_SERVICE, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        service.addCharacteristic(control)
        service.addCharacteristic(data)
        service.addCharacteristic(columbaRx)
        service.addCharacteristic(columbaTx)
        service.addCharacteristic(columbaIdentity)
        if (!server.addService(service)) {
            failRadio("Android rejected the Bluetooth service.")
        }
    }

    private fun startScan(adapter: BluetoothAdapter) {
        if (!running || !radioActive || !scanningWanted || scanner != null) {
            return
        }
        val scanner = adapter.bluetoothLeScanner ?: run {
            failRadio("Bluetooth discovery is unavailable.")
            return
        }
        val filters = listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(PRNS_SERVICE)).build())
        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_BALANCED)
            .build()
        try {
            val callback = scanCallback(radioEpoch)
            activeScanCallback = callback
            this.scanner = scanner
            scanner.startScan(filters, settings, callback)
            Log.i(TAG, "scanning for service $PRNS_SERVICE")
        } catch (e: SecurityException) {
            failRadio("Bluetooth discovery permission was denied.")
        }
    }

    private fun startAdvertise(adapter: BluetoothAdapter) = lifecycleLock.withLock {
        if (!running || !radioActive || !advertisingWanted || advertiser != null) {
            return
        }
        val advertiser = adapter.bluetoothLeAdvertiser ?: run {
            failRadio("Bluetooth advertising is unavailable.")
            return
        }
        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_POWER)
            .setConnectable(true)
            .setTimeout(0)
            .build()
        val data = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(ParcelUuid(PRNS_SERVICE))
            .addManufacturerData(
                PRNS_ROLE_COMPANY_ID,
                byteArrayOf(PRNS_ROLE_VERSION, PRNS_ROLE_DUAL_MODE),
            )
            .build()
        try {
            val callback = advertiseCallback(radioEpoch)
            activeAdvertiseCallback = callback
            this.advertiser = advertiser
            advertiser.startAdvertising(settings, data, callback)
        } catch (e: SecurityException) {
            failRadio("Bluetooth advertising permission was denied.")
        }
    }

    private fun stopScan() {
        val callback = activeScanCallback
        activeScanCallback = null
        if (callback != null) runCatching { scanner?.stopScan(callback) }
        scanner = null
    }

    private fun stopAdvertise() {
        val callback = activeAdvertiseCallback
        activeAdvertiseCallback = null
        if (callback != null) runCatching { advertiser?.stopAdvertising(callback) }
        advertiser = null
        advertisingStarted = false
    }

    /**
     * Call off the UI thread, before stopping/releasing the Rust session.
     * False retains the global owner: retry teardown, never start a replacement.
     */
    fun stop(): Boolean {
        val deadline = System.nanoTime() + WORKER_SHUTDOWN_TIMEOUT_MS * 1_000_000
        val acquired = try {
            lifecycleLock.tryLock(WORKER_SHUTDOWN_TIMEOUT_MS, TimeUnit.MILLISECONDS)
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            return false
        }
        if (!acquired) {
            report(State.Stopping, "Waiting for a Bluetooth callback to finish.")
            return false
        }
        try {
            if (owner.get() !== this) {
                report(State.Stopped)
                return true
            }
            report(State.Stopping)
            running = false
            PrnsBluetoothNative.nativeBleWakePumps()
            stopRadio()
        } finally {
            lifecycleLock.unlock()
        }
        val current = Thread.currentThread()
        val stopping = workers.toList()
        stopping.filter { it !== current }.forEach(Thread::interrupt)
        for (worker in stopping) {
            if (worker === current) continue
            val remainingNanos = deadline - System.nanoTime()
            if (remainingNanos <= 0) break
            try {
                worker.join((remainingNanos / 1_000_000).coerceAtLeast(1))
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
                report(State.Stopping, "Bluetooth shutdown was interrupted.")
                return false
            }
        }
        if (workers.any { it.isAlive }) {
            report(State.Stopping, "Waiting for Bluetooth workers to finish.")
            return false
        }
        owner.compareAndSet(this, null)
        report(State.Stopped)
        return true
    }

    private fun stopRadio() = lifecycleLock.withLock {
        // Late Android callbacks from a previous radio cycle cannot touch this one.
        radioEpoch++
        radioActive = false
        advertisingWanted = false
        scanningWanted = false
        stopScan()
        stopAdvertise()
        runCatching { gattServer?.close() }
        runCatching { l2capServer?.close() }
        for (connId in links.keys.toList()) {
            closeLink(connId)
        }
        scanner = null
        advertiser = null
        gattServer = null
        serviceReady = false
        advertisingStarted = false
        controlChar = null
        dataChar = null
        columbaRxChar = null
        columbaTxChar = null
        columbaIdentityChar = null
        l2capServer = null
        devices.clear()
        inboundByAddr.clear()
        columbaSubscribedCentrals.clear()
        dialingAddrs.clear()
        connectedAddrs.clear()
        linkedConnIds.clear()
        l2capOpening.clear()
        val current = Thread.currentThread()
        radioWorkers.filter { it !== current }.forEach(Thread::interrupt)
    }

    private fun parseMac(addr: String): ByteArray? {
        val parts = addr.split(":")
        if (parts.size != 6) {
            return null
        }
        return try {
            ByteArray(6) { parts[it].toInt(16).toByte() }
        } catch (e: NumberFormatException) {
            null
        }
    }

    @SuppressLint("HardwareIds")
    private fun shouldDial(peerAddress: ByteArray, result: ScanResult): Boolean {
        val capabilities = result.scanRecord
            ?.getManufacturerSpecificData(PRNS_ROLE_COMPANY_ID)
        if (capabilities != null &&
            capabilities.size >= 2 &&
            capabilities[0] >= PRNS_ROLE_VERSION &&
            capabilities[1].toInt() and PRNS_ROLE_PERIPHERAL_ONLY.toInt() != 0
        ) {
            return true
        }
        val localAddress = runCatching { adapter?.address }.getOrNull()?.let(::parseMac) ?: return true
        if (localAddress.contentEquals(HIDDEN_LOCAL_ADDRESS)) {
            return true
        }
        for (index in localAddress.indices) {
            val local = localAddress[index].toInt() and 0xff
            val peer = peerAddress[index].toInt() and 0xff
            if (local != peer) {
                return local < peer
            }
        }
        return false
    }

    private fun formatMac(octets: ByteArray): String =
        octets.joinToString(":") { "%02X".format(it) }

    @Suppress("DEPRECATION")
    private fun writeGattCharacteristic(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        payload: ByteArray,
        writeType: Int,
    ): Int {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            return gatt.writeCharacteristic(characteristic, payload, writeType)
        }
        characteristic.writeType = writeType
        characteristic.value = payload
        return if (gatt.writeCharacteristic(characteristic)) {
            BluetoothGatt.GATT_SUCCESS
        } else {
            BluetoothGatt.GATT_FAILURE
        }
    }

    @Suppress("DEPRECATION")
    private fun notifyGattCharacteristic(
        server: BluetoothGattServer,
        device: BluetoothDevice,
        characteristic: BluetoothGattCharacteristic,
        payload: ByteArray,
    ): Int {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            return server.notifyCharacteristicChanged(device, characteristic, false, payload)
        }
        characteristic.value = payload
        return if (server.notifyCharacteristicChanged(device, characteristic, false)) {
            BluetoothGatt.GATT_SUCCESS
        } else {
            BluetoothGatt.GATT_FAILURE
        }
    }

    @Suppress("DEPRECATION")
    private fun writeGattDescriptor(
        gatt: BluetoothGatt,
        descriptor: BluetoothGattDescriptor,
        payload: ByteArray,
    ): Int {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            return gatt.writeDescriptor(descriptor, payload)
        }
        descriptor.value = payload
        return if (gatt.writeDescriptor(descriptor)) {
            BluetoothGatt.GATT_SUCCESS
        } else {
            BluetoothGatt.GATT_FAILURE
        }
    }

    companion object {
        private val owner = AtomicReference<PrnsBluetoothLink?>()

        /** Background location is a separate, optional discovery permission. */
        fun requiredPermissions(): List<String> =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                listOf(
                    Manifest.permission.BLUETOOTH_SCAN,
                    Manifest.permission.BLUETOOTH_CONNECT,
                    Manifest.permission.BLUETOOTH_ADVERTISE,
                )
            } else {
                listOf(
                    Manifest.permission.ACCESS_FINE_LOCATION,
                )
            }

        private const val TAG = "PrnsAppBle"
        private const val L2CAP_CHUNK = 2048
        private const val CONTROL_CHUNK = 64
        private const val DATA_CHUNK = 512
        private const val RADIO_STATE_RETRY_MS = 1_000L
        private const val RSSI_NONE = 127
        private const val MAX_ATT_MTU = 517
        private const val CLIENT_LINK_READY_TIMEOUT_MS = 8_000L
        private const val WORKER_SHUTDOWN_TIMEOUT_MS = 2_000L
        private const val GATT_BUSY_RETRY_MS = 4L
        private const val ERROR_GATT_WRITE_REQUEST_BUSY = 201
        private const val ATT_INSUFFICIENT_RESOURCES = 0x11
        private const val L2CAP_OPEN_RETRIES = 5
        private const val L2CAP_OPEN_RETRY_MS = 200L
        private const val PRNS_ROLE_COMPANY_ID = 0xFFFF
        private const val PRNS_ROLE_VERSION: Byte = 0x03
        private const val PRNS_ROLE_DUAL_MODE: Byte = 0x00
        private const val PRNS_ROLE_PERIPHERAL_ONLY: Byte = 0x01
        private val HIDDEN_LOCAL_ADDRESS = byteArrayOf(2, 0, 0, 0, 0, 0)
        val PRNS_SERVICE: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e3")
        val COLUMBA_TX: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e4")
        val COLUMBA_RX: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e5")
        val COLUMBA_IDENTITY: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e6")
        val NATIVE_CONTROL: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e7")
        val NATIVE_DATA: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e8")
        val CCCD: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
        private const val COLUMBA_IDENTITY_LEN = 16
    }

    private val dialingAddrs = ConcurrentHashMap.newKeySet<String>()
    private val connectedAddrs = ConcurrentHashMap.newKeySet<String>()
    private val linkedConnIds = ConcurrentHashMap.newKeySet<Int>()
}

/**
 * Android 10–12 deliver the deprecated notification overload. Forward it to
 * the same handler as API 33+, rather than dropping every incoming notification.
 */
internal abstract class PrnsBluetoothGattCallback : BluetoothGattCallback() {
    // Declare this on our class too: API 29 has no three-argument platform method
    // for the legacy forwarding call to resolve against.
    abstract override fun onCharacteristicChanged(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        value: ByteArray,
    )

    @Suppress("DEPRECATION")
    override fun onCharacteristicChanged(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
    ) {
        onCharacteristicChanged(gatt, characteristic, characteristic.value ?: ByteArray(0))
    }
}

/** Nearby-device permissions are checked separately before any radio work. */
internal fun prnsBluetoothDiscoveryAllowed(
    sdk: Int,
    activityVisible: Boolean,
    backgroundLocationGranted: Boolean,
): Boolean = sdk >= 31 || activityVisible || backgroundLocationGranted

internal fun prnsPublishBluetoothPsm(
    psm: Int,
    publishNative: (Int) -> Unit,
    notifyOwner: (Int) -> Unit,
) {
    require(psm > 0) { "A Bluetooth listener must have a valid PSM" }
    publishNative(psm)
    notifyOwner(psm)
}
