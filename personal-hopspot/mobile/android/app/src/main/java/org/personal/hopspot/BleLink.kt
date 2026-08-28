package org.personal.hopspot

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
import java.util.concurrent.Semaphore
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

@TargetApi(Build.VERSION_CODES.Q)
@SuppressLint("MissingPermission")
class BleLink(private val context: Context) {
    private val bluetoothManager =
        context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
    private val adapter: BluetoothAdapter? = bluetoothManager?.adapter
    private val peerCapacity = NativeBridge.nativeBlePeerCapacity().coerceAtLeast(1)
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
    /** Last scan-time discovery-group match for a peer address. Missing means not yet observed. */
    private val peerDiscoveryAllowed = ConcurrentHashMap<String, Boolean>()
    private val workers = CopyOnWriteArraySet<Thread>()
    private val radioWorkers = CopyOnWriteArraySet<Thread>()
    private val linkWorkers = ConcurrentHashMap<Int, CopyOnWriteArraySet<Thread>>()
    private val l2capOpening = ConcurrentHashMap.newKeySet<Int>()

    private enum class BlePeerProtocol {
        Native,
        Columba,
    }

    private enum class GattOperationKind {
        Mtu,
        ServiceDiscovery,
        CharacteristicRead,
        DescriptorWrite,
        ClientWrite,
        ServerNotify,
    }

    private enum class OutboundAdmission {
        Accepted,
        Busy,
        Terminal,
    }

    private data class PendingGattOperation(
        val kind: GattOperationKind,
        val characteristic: UUID?,
    )

    private class LinkState(
        val connId: Int,
        val address: String,
        val dialed: Boolean,
        @Volatile var peerProtocol: BlePeerProtocol,
    ) {
        private val operationGate = Semaphore(1)
        val servicesRequested = AtomicBoolean(false)

        @Volatile
        private var pendingGattOperation: PendingGattOperation? = null

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

        @Synchronized
        fun beginGattOperation(operation: PendingGattOperation): Boolean {
            if (!operationGate.tryAcquire()) {
                return false
            }
            pendingGattOperation = operation
            return true
        }

        @Synchronized
        fun completeGattOperation(kind: GattOperationKind, characteristic: UUID? = null): Boolean {
            val pending = pendingGattOperation ?: return false
            if (pending.kind != kind ||
                characteristic != null && pending.characteristic != characteristic
            ) {
                return false
            }
            pendingGattOperation = null
            operationGate.release()
            return true
        }

        @Synchronized
        fun cancelGattOperation(operation: PendingGattOperation) {
            if (pendingGattOperation != operation) {
                return
            }
            pendingGattOperation = null
            operationGate.release()
        }
    }

    private val scanCallback = object : ScanCallback() {
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            if (!running || !radioActive || !scanningWanted) {
                return
            }
            val device = result.device
            if (!rememberDevice(device)) {
                Log.w(TAG, "sighting cache full; ignored ${device.address}")
                return
            }
            val octets = parseMac(device.address) ?: return
            val capabilities = result.scanRecord
                ?.getManufacturerSpecificData(PRNS_ROLE_COMPANY_ID)
            // Only cache when manufacturer data is present so truncated ads don't
            // permanently mark a same-group peer as mismatched.
            if (capabilities != null) {
                val allowed = matchesLocalDiscoveryGroup(capabilities)
                val previous = peerDiscoveryAllowed.put(device.address, allowed)
                if (previous != allowed) {
                    Log.i(
                        TAG,
                        "scan group ${device.address}: allowed=$allowed " +
                            "mfg=${capabilities.joinToString("") { "%02x".format(it) }}",
                    )
                }
                if (!allowed && previous != false) {
                    tearDownLinksForAddress(device.address)
                }
            }
            if (!shouldDial(octets, result)) {
                return
            }
            val direct = ByteBuffer.allocateDirect(6)
            direct.put(octets)
            NativeBridge.nativeBleSighting(direct, result.rssi)
        }

        override fun onScanFailed(errorCode: Int) {
            Log.w(TAG, "scan failed code=$errorCode")
        }
    }

    private val advertiseCallback = object : AdvertiseCallback() {
        override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
            Log.i(TAG, "advertising $PRNS_SERVICE mode=${settingsInEffect.mode}")
        }

        override fun onStartFailure(errorCode: Int) {
            Log.w(TAG, "advertise failed code=$errorCode")
        }
    }

    private val gattServerCallback = object : BluetoothGattServerCallback() {
        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            if (newState == BluetoothProfile.STATE_CONNECTED) {
                rememberDevice(device)
                if (!inboundDiscoveryPermitted(device.address)) {
                    Log.w(TAG, "rejecting inbound ${device.address}: discovery group not permitted")
                    runCatching { gattServer?.cancelConnection(device) }
                }
                return
            }
            if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                val connId = inboundByAddr.remove(device.address) ?: return
                Log.i(TAG, "listener[$connId] ${device.address} disconnected")
                closeLink(connId)
            }
        }

        override fun onServiceAdded(status: Int, service: BluetoothGattService) {
            Log.i(TAG, "server service added status=$status")
            val adapter = adapter ?: return
            if (running && radioActive && advertisingWanted) {
                startAdvertise(adapter)
            }
        }

        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic,
        ) {
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

        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray,
        ) {
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
                } ?: NativeBridge.BLE_INGRESS_CLOSED
                COLUMBA_RX -> handleColumbaRxWrite(device, value)
                else -> {
                    Log.w(TAG, "server write to unknown characteristic ${characteristic.uuid}")
                    NativeBridge.BLE_INGRESS_CLOSED
                }
            }
            if (responseNeeded) {
                val status = when (admission) {
                    NativeBridge.BLE_INGRESS_ACCEPTED -> BluetoothGatt.GATT_SUCCESS
                    NativeBridge.BLE_INGRESS_FULL -> ATT_INSUFFICIENT_RESOURCES
                    else -> BluetoothGatt.GATT_FAILURE
                }
                gattServer?.sendResponse(device, requestId, status, offset, null)
            }
            if (admission == NativeBridge.BLE_INGRESS_CLOSED) {
                inboundByAddr[device.address]?.let(::closeLink)
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
            if (!running || !radioActive) {
                return
            }
            val subscribing = value.isNotEmpty() && value[0].toInt() != 0
            var responseStatus = BluetoothGatt.GATT_SUCCESS
            if (descriptor.characteristic.uuid == COLUMBA_TX) {
                if (subscribing) {
                    if (!inboundDiscoveryPermitted(device.address)) {
                        responseStatus = BluetoothGatt.GATT_FAILURE
                        Log.w(TAG, "columba subscription reject ${device.address}: discovery group not permitted")
                        runCatching { gattServer?.cancelConnection(device) }
                    } else if (columbaSubscribedCentrals.containsKey(device.address) ||
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
                if (!inboundDiscoveryPermitted(device.address)) {
                    responseStatus = BluetoothGatt.GATT_FAILURE
                    Log.w(TAG, "listener reject ${device.address}: discovery group not permitted")
                    runCatching { gattServer?.cancelConnection(device) }
                } else if (links.size >= peerCapacity) {
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
                        if (!NativeBridge.nativeBleLinkUp(connId, direct, RSSI_NONE, false)) {
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

        override fun onNotificationSent(device: BluetoothDevice, status: Int) {
            if (!running || !radioActive) {
                return
            }
            val connId = inboundByAddr[device.address] ?: return
            val link = links[connId] ?: return
            if (link.completeGattOperation(GattOperationKind.ServerNotify)) {
                NativeBridge.nativeBleWakePumps()
                if (status != BluetoothGatt.GATT_SUCCESS) {
                    Log.w(TAG, "notification failed[$connId] status=$status")
                    closeLink(connId)
                }
            }
        }
    }

    private fun handleColumbaRxWrite(device: BluetoothDevice, value: ByteArray): Int {
        val address = device.address
        val existingConnId = inboundByAddr[address]
        if (existingConnId == null) {
            if (!inboundDiscoveryPermitted(address)) {
                Log.w(TAG, "columba reject $address: discovery group not permitted")
                runCatching { gattServer?.cancelConnection(device) }
                return NativeBridge.BLE_INGRESS_CLOSED
            }
            if (value.size != COLUMBA_IDENTITY_LEN) {
                Log.w(TAG, "columba RX from $address before identity (${value.size}B), dropping")
                return NativeBridge.BLE_INGRESS_CLOSED
            }
            val octets = parseMac(address) ?: return NativeBridge.BLE_INGRESS_CLOSED
            val connId = nextConnId.getAndIncrement()
            val link = LinkState(connId, address, dialed = false, peerProtocol = BlePeerProtocol.Columba)
            link.central = columbaSubscribedCentrals[address] ?: device
            link.peerIdentity = value.copyOf()
            links[connId] = link
            inboundByAddr[address] = connId
            Log.i(TAG, "columba listener[$connId] $address identity ${value.size}B")
            val admitted = NativeBridge.nativeBleColumbaLinkUp(
                connId,
                directBufferOf(octets),
                RSSI_NONE,
                false,
                directBufferOf(value),
            )
            if (!admitted) {
                Log.w(TAG, "columba listener[$connId] lifecycle admission rejected")
                closeLink(connId)
                return NativeBridge.BLE_INGRESS_CLOSED
            }
            return NativeBridge.BLE_INGRESS_ACCEPTED
        }

        val link = links[existingConnId]
        if (link?.peerProtocol == BlePeerProtocol.Columba &&
            value.size == COLUMBA_IDENTITY_LEN &&
            link.peerIdentity?.contentEquals(value) == true
        ) {
            Log.i(TAG, "columba listener[$existingConnId] duplicate identity consumed")
            return NativeBridge.BLE_INGRESS_ACCEPTED
        }

        return deliverGattInbound(existingConnId, true, value)
    }

    private fun deliverGattInbound(connId: Int, dataLane: Boolean, value: ByteArray): Int {
        val direct = ByteBuffer.allocateDirect(value.size)
        direct.put(value)
        val admission =
            if (dataLane) {
                NativeBridge.nativeBleDataIn(connId, direct, value.size)
            } else {
                NativeBridge.nativeBleControlIn(connId, direct, value.size)
            }
        if (admission != NativeBridge.BLE_INGRESS_ACCEPTED) {
            val reason = if (admission == NativeBridge.BLE_INGRESS_FULL) "full" else "closed"
            Log.w(TAG, "inbound Bluetooth LE queue $reason[$connId] ${value.size} B")
        }
        return admission
    }

    private fun localBleIdentity(): ByteArray? {
        val direct = ByteBuffer.allocateDirect(COLUMBA_IDENTITY_LEN)
        val n = NativeBridge.nativeBleIdentity(direct)
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

    fun start() {
        val adapter = adapter
        if (adapter == null) {
            Log.w(TAG, "bluetooth adapter unavailable")
        } else if (!adapter.isEnabled) {
            Log.w(TAG, "bluetooth adapter unavailable or off")
        }
        running = true
        startRadioStatePump()
        startControlOutPump()
        startDataOutPump()
        startDialPump()
        startL2capOpenPump()
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
        val worker = Thread {
            try {
                task()
            } catch (_: InterruptedException) {
            } finally {
                val current = Thread.currentThread()
                workers.remove(current)
                radioWorkers.remove(current)
                if (connId != null) {
                    linkWorkers[connId]?.let { owned ->
                        owned.remove(current)
                        if (owned.isEmpty()) {
                            linkWorkers.remove(connId, owned)
                        }
                    }
                }
            }
        }
        worker.name = "HopspotBle-$name"
        workers.add(worker)
        if (radioScoped) {
            radioWorkers.add(worker)
        }
        if (connId != null) {
            linkWorkers.computeIfAbsent(connId) { CopyOnWriteArraySet() }.add(worker)
        }
        worker.start()
    }

    private fun startRadioStatePump() {
        startWorker("radio-state") {
            var lastState = Int.MIN_VALUE
            var generation = NativeBridge.nativeBleWorkGeneration()
            while (running) {
                val state = NativeBridge.nativeBleDesiredState()
                val wantsRadio = (state and NativeBridge.BLE_RADIO_ENABLED) != 0
                if (state != lastState || wantsRadio && !radioActive) {
                    val wasActive = radioActive
                    applyDesiredRadioState(state)
                    lastState = state
                    if (!wasActive && radioActive) {
                        NativeBridge.nativeBleWakePumps()
                    }
                }
                generation = NativeBridge.nativeBleWaitForWork(generation, RADIO_STATE_RETRY_MS)
            }
            applyDesiredRadioState(0)
        }
    }

    @Synchronized
    private fun applyDesiredRadioState(state: Int) {
        val wantRadio = (state and NativeBridge.BLE_RADIO_ENABLED) != 0
        advertisingWanted = (state and NativeBridge.BLE_RADIO_ADVERTISING) != 0
        scanningWanted = (state and NativeBridge.BLE_RADIO_SCANNING) != 0
        if (!running || !wantRadio) {
            stopRadio()
            return
        }
        val adapter = adapter
        if (adapter == null || !adapter.isEnabled) {
            if (radioActive) {
                Log.w(TAG, "bluetooth adapter unavailable or off")
            }
            stopRadio()
            return
        }
        radioActive = true
        if (!startL2capListener(adapter)) {
            radioActive = false
            return
        }
        if (scanningWanted) {
            startScan(adapter)
        } else {
            stopScan()
        }
        if (advertisingWanted) {
            startGattServer()
            if (gattServer != null) {
                startAdvertise(adapter)
            }
        } else {
            stopAdvertise()
        }
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
        Log.i(TAG, "l2cap listener published psm=$psm")
        NativeBridge.nativeBleSetPsm(psm)
        startRadioWorker("l2cap-listener") {
            while (running && radioActive) {
                val socket = try {
                    server.accept()
                } catch (e: Exception) {
                    Log.w(TAG, "l2cap accept ended: $e")
                    break
                }
                if (!radioActive) {
                    runCatching { socket.close() }
                    break
                }
                val address = socket.remoteDevice?.address
                val connId = address?.let { inboundByAddr[it] }
                if (connId == null) {
                    Log.w(TAG, "l2cap accept from $address with no listener link, dropping")
                    runCatching { socket.close() }
                    continue
                }
                Log.i(TAG, "l2cap accepted[$connId] from $address")
                links[connId]?.l2capSocket = socket
                NativeBridge.nativeBleL2capUp(connId)
                startL2capPumps(connId, socket)
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
                    if (!NativeBridge.nativeBleL2capIn(connId, direct, n)) {
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
            var generation = NativeBridge.nativeBleWorkGeneration()
            while (running && radioActive && links.containsKey(connId)) {
                direct.clear()
                val n = NativeBridge.nativeBleL2capOut(connId, direct)
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
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
                }
            }
        }
    }

    private fun startControlOutPump() {
        startWorker("control-out") {
            val direct = ByteBuffer.allocateDirect(CONTROL_CHUNK)
            val scratch = ByteArray(CONTROL_CHUNK)
            var generation = NativeBridge.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
                    continue
                }
                var pending = false
                var progressed = false
                for (link in links.values) {
                    direct.clear()
                    val n = NativeBridge.nativeBleControlOut(link.connId, direct)
                    if (n > 0) {
                        pending = true
                        direct.position(0)
                        direct.get(scratch, 0, n)
                        when (deliverControl(link, scratch.copyOf(n))) {
                            OutboundAdmission.Accepted -> {
                                progressed = NativeBridge.nativeBleCommitControlOut(link.connId)
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
                    generation = NativeBridge.nativeBleWaitForWork(
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
            var generation = NativeBridge.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
                    continue
                }
                var pending = false
                var progressed = false
                for (link in links.values) {
                    direct.clear()
                    val n = NativeBridge.nativeBleDataOut(link.connId, direct)
                    if (n > 0) {
                        pending = true
                        Log.i(TAG, "data out[${link.connId}] ${n}B")
                        direct.position(0)
                        direct.get(scratch, 0, n)
                        when (deliverData(link, scratch.copyOf(n))) {
                            OutboundAdmission.Accepted -> {
                                progressed = NativeBridge.nativeBleCommitDataOut(link.connId)
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
                    generation = NativeBridge.nativeBleWaitForWork(
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
            var generation = NativeBridge.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
                    continue
                }
                // Policy Reject queues closes before redials; drain them first so inbound
                // occupancy does not block the opener from dialing as central.
                var closed = false
                while (true) {
                    val connId = NativeBridge.nativeBleNextClose()
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
                if (!NativeBridge.nativeBleNextDial(direct)) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
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
                dialingAddrs.add(address)
                val connId = nextConnId.getAndIncrement()
                Log.i(TAG, "dialing[$connId] $address as gatt client")
                links[connId] = LinkState(connId, address, dialed = true, peerProtocol = BlePeerProtocol.Native)
                device.connectGatt(context, false, clientCallback(connId, address), BluetoothDevice.TRANSPORT_LE)
            }
        }
    }

    private fun startL2capOpenPump() {
        startWorker("l2cap-open") {
            val direct = ByteBuffer.allocateDirect(6)
            val raw = ByteArray(6)
            var generation = NativeBridge.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
                    continue
                }
                direct.clear()
                if (!NativeBridge.nativeBleNextL2capOpen(direct)) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
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
                            links.containsKey(connId)
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
                            link.openingL2capSocket = socket
                            try {
                                socket.connect()
                                if (!running || !radioActive || !links.containsKey(connId)) {
                                    runCatching { socket.close() }
                                    break
                                }
                                Log.i(TAG, "l2cap client[$connId] connected to psm=$psm attempt=$attempt")
                                link.openingL2capSocket = null
                                link.l2capSocket = socket
                                NativeBridge.nativeBleL2capUp(connId)
                                startL2capPumps(connId, socket)
                                opened = true
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
                    if (!opened && running && radioActive && links.containsKey(connId)) {
                        Log.w(TAG, "l2cap client[$connId] psm=$psm gave up after $attempt attempts; staying on GATT")
                    }
                }
            }
        }
    }

    private fun clientCallback(connId: Int, address: String): BluetoothGattCallback =
        object : BluetoothGattCallback() {
            override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
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
                    val mtuOperation = PendingGattOperation(GattOperationKind.Mtu, null)
                    val mtuRequested = link.beginGattOperation(mtuOperation) &&
                        runCatching { gatt.requestMtu(MAX_ATT_MTU) }.getOrDefault(false)
                    Log.i(TAG, "dialer[$connId] connected; requested mtu=$mtuRequested")
                    if (!mtuRequested) {
                        link.cancelGattOperation(mtuOperation)
                        requestClientServices(gatt, link, "mtu request rejected")
                    }
                    scheduleClientOpenFallback(connId, address, gatt)
                } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                    Log.i(TAG, "dialer[$connId] $address disconnected status=$status")
                    if (!linkedConnIds.remove(connId)) {
                        parseMac(address)?.let { octets ->
                            val direct = ByteBuffer.allocateDirect(6)
                            direct.put(octets)
                            if (!NativeBridge.nativeBleDialFailed(direct)) {
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

            override fun onMtuChanged(gatt: BluetoothGatt, mtu: Int, status: Int) {
                if (!running || !radioActive) {
                    return
                }
                links[connId]?.completeGattOperation(GattOperationKind.Mtu)
                Log.i(TAG, "dialer[$connId] att mtu=$mtu status=$status")
                links[connId]?.let { requestClientServices(gatt, it, "mtu changed") }
            }

            override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
                if (!running || !radioActive) {
                    return
                }
                links[connId]?.completeGattOperation(GattOperationKind.ServiceDiscovery)
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

            override fun onCharacteristicRead(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
                status: Int,
            ) {
                handleClientCharacteristicRead(connId, address, gatt, characteristic, value, status)
            }

            @Suppress("DEPRECATION")
            override fun onCharacteristicRead(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int,
            ) {
                handleClientCharacteristicRead(
                    connId,
                    address,
                    gatt,
                    characteristic,
                    characteristic.value ?: ByteArray(0),
                    status,
                )
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
                links[connId]?.completeGattOperation(
                    GattOperationKind.CharacteristicRead,
                    characteristic.uuid,
                )
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
                if (!running || !radioActive) {
                    return
                }
                links[connId]?.completeGattOperation(
                    GattOperationKind.DescriptorWrite,
                    descriptor.characteristic.uuid,
                )
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
                    val link = links[connId] ?: return
                    val identity = link.peerIdentity
                    if (identity == null) {
                        Log.w(TAG, "dialer[$connId] Columba TX subscribe failed status=$status")
                        runCatching { gatt.disconnect() }
                        return
                    }
                    Log.i(TAG, "dialer[$connId] $address subscribed (Columba TX ready)")
                    linkedConnIds.add(connId)
                    val octets = parseMac(address)
                    if (octets != null) {
                        if (!NativeBridge.nativeBleColumbaLinkUp(
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
                linkedConnIds.add(connId)
                val octets = parseMac(address)
                if (octets != null) {
                    val direct = ByteBuffer.allocateDirect(6)
                    direct.put(octets)
                    if (!NativeBridge.nativeBleLinkUp(connId, direct, RSSI_NONE, true)) {
                        Log.w(TAG, "dialer[$connId] lifecycle admission rejected")
                        closeLink(connId)
                    }
                }
            }

            override fun onCharacteristicWrite(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                status: Int,
            ) {
                if (!running || !radioActive) {
                    return
                }
                val link = links[connId] ?: return
                if (link.completeGattOperation(GattOperationKind.ClientWrite, characteristic.uuid)) {
                    NativeBridge.nativeBleWakePumps()
                    if (status != BluetoothGatt.GATT_SUCCESS) {
                        Log.w(TAG, "write completion failed[$connId] status=$status")
                        closeLink(connId)
                    }
                }
            }

            override fun onCharacteristicChanged(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
            ) {
                if (!running || !radioActive) {
                    return
                }
                val dataLane = characteristic.uuid == NATIVE_DATA || characteristic.uuid == COLUMBA_TX
                Log.i(TAG, "dialer[$connId] notify ${if (dataLane) "DATA" else "CONTROL"} ${value.size}B")
                if (deliverGattInbound(connId, dataLane, value) == NativeBridge.BLE_INGRESS_CLOSED) {
                    closeLink(connId)
                }
            }
        }

    private fun requestClientServices(gatt: BluetoothGatt, link: LinkState, reason: String) {
        if (!link.servicesRequested.compareAndSet(false, true)) {
            return
        }
        val operation = PendingGattOperation(GattOperationKind.ServiceDiscovery, null)
        if (!link.beginGattOperation(operation)) {
            link.servicesRequested.set(false)
            return
        }
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

    private fun scheduleClientOpenFallback(connId: Int, address: String, gatt: BluetoothGatt) {
        startLinkWorker(connId, "client-fallback-$connId") {
            Thread.sleep(MTU_DISCOVERY_FALLBACK_MS)
            val link = links[connId]
            if (running && radioActive && link != null && !linkedConnIds.contains(connId)) {
                link.cancelGattOperation(PendingGattOperation(GattOperationKind.Mtu, null))
                requestClientServices(gatt, link, "mtu callback timeout")
            }
            Thread.sleep(CLIENT_LINK_READY_TIMEOUT_MS - MTU_DISCOVERY_FALLBACK_MS)
            if (running && radioActive && links.containsKey(connId) && !linkedConnIds.contains(connId)) {
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
            NativeBridge.nativeBleDisconnected(connId)
            return
        }
        inboundByAddr.remove(link.address, connId)
        peerDiscoveryAllowed.remove(link.address)
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
        NativeBridge.nativeBleDisconnected(connId)
    }

    private fun startGattServer() {
        if (!running || !radioActive || gattServer != null) {
            return
        }
        val manager = bluetoothManager ?: return
        val server = try {
            manager.openGattServer(context, gattServerCallback)
        } catch (e: SecurityException) {
            Log.w(TAG, "openGattServer denied: $e")
            return
        } ?: return
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
        runCatching { server.addService(service) }
        Log.i(TAG, "gatt server open; Prns native + Columba service added")
    }

    private fun startScan(adapter: BluetoothAdapter) {
        if (!running || !radioActive || !scanningWanted || scanner != null) {
            return
        }
        val scanner = adapter.bluetoothLeScanner ?: return
        val filters = listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(PRNS_SERVICE)).build())
        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_BALANCED)
            .build()
        try {
            scanner.startScan(filters, settings, scanCallback)
            this.scanner = scanner
            Log.i(TAG, "scanning for service $PRNS_SERVICE")
        } catch (e: SecurityException) {
            Log.w(TAG, "scan permission denied: $e")
        }
    }

    @Synchronized
    private fun startAdvertise(adapter: BluetoothAdapter) {
        if (!running || !radioActive || !advertisingWanted || advertiser != null) {
            return
        }
        val advertiser = adapter.bluetoothLeAdvertiser ?: return
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
                localGroupTag().let { tag ->
                    byteArrayOf(
                        PRNS_ROLE_VERSION,
                        PRNS_ROLE_DUAL_MODE,
                        tag[0],
                        tag[1],
                        tag[2],
                        tag[3],
                    )
                },
            )
            .build()
        try {
            advertiser.startAdvertising(settings, data, advertiseCallback)
            this.advertiser = advertiser
        } catch (e: SecurityException) {
            Log.w(TAG, "advertise permission denied: $e")
        }
    }

    private fun stopScan() {
        runCatching { scanner?.stopScan(scanCallback) }
        scanner = null
    }

    private fun stopAdvertise() {
        runCatching { advertiser?.stopAdvertising(advertiseCallback) }
        advertiser = null
    }

    fun stop() {
        running = false
        NativeBridge.nativeBleWakePumps()
        stopRadio()
        val current = Thread.currentThread()
        val stopping = workers.filter { it !== current }
        stopping.forEach(Thread::interrupt)
        val deadline = System.nanoTime() + WORKER_SHUTDOWN_TIMEOUT_MS * 1_000_000
        for (worker in stopping) {
            val remainingNanos = deadline - System.nanoTime()
            if (remainingNanos <= 0) {
                break
            }
            worker.join((remainingNanos / 1_000_000).coerceAtLeast(1))
        }
        val remaining = workers.count { it !== current && it.isAlive }
        if (remaining > 0) {
            Log.w(TAG, "$remaining Bluetooth LE workers remained after shutdown deadline")
        }
    }

    @Synchronized
    private fun stopRadio() {
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
        controlChar = null
        dataChar = null
        columbaRxChar = null
        columbaTxChar = null
        columbaIdentityChar = null
        l2capServer = null
        devices.clear()
        peerDiscoveryAllowed.clear()
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
        if (!matchesLocalDiscoveryGroup(capabilities)) {
            return false
        }
        if (capabilities != null &&
            capabilities.size >= 2 &&
            capabilities[0] >= PRNS_ROLE_VERSION_MIN &&
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

    private fun matchesLocalDiscoveryGroup(capabilities: ByteArray?): Boolean {
        val local = localGroupTag()
        // Missing or legacy (v3) manufacturer payloads map to the default reticulum group.
        if (capabilities == null ||
            capabilities.size < 2 ||
            capabilities[0] < PRNS_ROLE_VERSION
        ) {
            return local.contentEquals(PRNS_DEFAULT_GROUP_TAG)
        }
        if (capabilities.size < 6) {
            return local.contentEquals(PRNS_DEFAULT_GROUP_TAG)
        }
        return capabilities[2] == local[0] &&
            capabilities[3] == local[1] &&
            capabilities[4] == local[2] &&
            capabilities[5] == local[3]
    }

    /**
     * Inbound GATT admission:
     * - known mismatch (`false`) → reject early
     * - known match / unknown → allow; Hello/Welcome group tag is the authoritative gate
     *
     * Unknown must not reject on custom groups: the peer that dials first is often not in the
     * scan cache yet (truncated ads / no manufacturer payload), which would deadlock same-group peering.
     */
    private fun inboundDiscoveryPermitted(address: String): Boolean {
        return peerDiscoveryAllowed[address] != false
    }

    private fun tearDownLinksForAddress(address: String) {
        val victims = links.filterValues { it.address == address }.keys.toList()
        if (victims.isEmpty()) {
            return
        }
        Log.w(TAG, "tearing down ${victims.size} link(s) for $address: discovery group mismatch")
        for (connId in victims) {
            closeLink(connId)
        }
    }

    private fun localGroupTag(): ByteArray {
        val buffer = ByteBuffer.allocateDirect(4)
        val n = NativeBridge.nativeBleGroupTag(buffer)
        if (n < 4) {
            return PRNS_DEFAULT_GROUP_TAG
        }
        val tag = ByteArray(4)
        buffer.get(tag)
        return tag
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

    private companion object {
        private const val TAG = "HopspotBle"
        private const val L2CAP_CHUNK = 2048
        private const val CONTROL_CHUNK = 64
        private const val DATA_CHUNK = 512
        private const val RADIO_STATE_RETRY_MS = 1_000L
        private const val RSSI_NONE = 127
        private const val MAX_ATT_MTU = 517
        private const val MTU_DISCOVERY_FALLBACK_MS = 750L
        private const val CLIENT_LINK_READY_TIMEOUT_MS = 8_000L
        private const val WORKER_SHUTDOWN_TIMEOUT_MS = 2_000L
        private const val GATT_BUSY_RETRY_MS = 4L
        private const val ERROR_GATT_WRITE_REQUEST_BUSY = 201
        private const val ATT_INSUFFICIENT_RESOURCES = 0x11
        private const val L2CAP_OPEN_RETRIES = 5
        private const val L2CAP_OPEN_RETRY_MS = 200L
        private const val PRNS_ROLE_COMPANY_ID = 0xFFFF
        /** Oldest manufacturer role payload we still accept while scanning. */
        private const val PRNS_ROLE_VERSION_MIN: Byte = 0x03
        /** Advertised manufacturer payload version that includes a discovery group tag. */
        private const val PRNS_ROLE_VERSION: Byte = 0x04
        private const val PRNS_ROLE_DUAL_MODE: Byte = 0x00
        private const val PRNS_ROLE_PERIPHERAL_ONLY: Byte = 0x01
        /** sha256("reticulum")[0..4] — must match prns-core DEFAULT_GROUP_TAG. */
        private val PRNS_DEFAULT_GROUP_TAG =
            byteArrayOf(0xEA.toByte(), 0xC4.toByte(), 0xD7.toByte(), 0x0B)
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
