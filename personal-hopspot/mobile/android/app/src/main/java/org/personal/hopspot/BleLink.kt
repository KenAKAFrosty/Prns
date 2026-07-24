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
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

@TargetApi(Build.VERSION_CODES.Q)
@SuppressLint("MissingPermission")
class BleLink(private val context: Context) {
    private val bluetoothManager =
        context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
    private val adapter: BluetoothAdapter? = bluetoothManager?.adapter

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

    private enum class BlePeerProtocol {
        Native,
        Columba,
    }

    private class LinkState(
        val connId: Int,
        val address: String,
        val dialed: Boolean,
        @Volatile var peerProtocol: BlePeerProtocol,
    ) {
        val sendGate = Semaphore(1)
        val servicesRequested = AtomicBoolean(false)

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
    }

    private val scanCallback = object : ScanCallback() {
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            if (!running || !radioActive || !scanningWanted) {
                return
            }
            val device = result.device
            devices[device.address] = device
            val octets = parseMac(device.address) ?: return
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
            val accepted = when (characteristic.uuid) {
                NATIVE_CONTROL,
                NATIVE_DATA,
                -> inboundByAddr[device.address]?.let { connId ->
                    deliverGattInbound(
                        connId,
                        characteristic.uuid == NATIVE_DATA,
                        value,
                    )
                } ?: false
                COLUMBA_RX -> handleColumbaRxWrite(device, value)
                else -> {
                    Log.w(TAG, "server write to unknown characteristic ${characteristic.uuid}")
                    false
                }
            }
            if (responseNeeded) {
                val status =
                    if (accepted) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_FAILURE
                gattServer?.sendResponse(device, requestId, status, offset, null)
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
            if (descriptor.characteristic.uuid == COLUMBA_TX) {
                if (subscribing) {
                    columbaSubscribedCentrals[device.address] = device
                    Log.i(TAG, "columba central ${device.address} subscribed; awaiting identity")
                } else {
                    columbaSubscribedCentrals.remove(device.address)
                }
            } else if (
                subscribing &&
                descriptor.characteristic.uuid == NATIVE_CONTROL &&
                inboundByAddr[device.address] == null
            ) {
                val connId = nextConnId.getAndIncrement()
                val link = LinkState(connId, device.address, dialed = false, peerProtocol = BlePeerProtocol.Native)
                link.central = device
                links[connId] = link
                inboundByAddr[device.address] = connId
                Log.i(TAG, "listener[$connId] ${device.address} subscribed")
                val octets = parseMac(device.address)
                if (octets != null) {
                    val direct = ByteBuffer.allocateDirect(6)
                    direct.put(octets)
                    NativeBridge.nativeBleLinkUp(connId, direct, RSSI_NONE, false)
                }
            }
            if (responseNeeded) {
                gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, null)
            }
        }

        override fun onNotificationSent(device: BluetoothDevice, status: Int) {
            if (!running || !radioActive) {
                return
            }
            inboundByAddr[device.address]?.let { links[it]?.sendGate?.release() }
        }
    }

    private fun handleColumbaRxWrite(device: BluetoothDevice, value: ByteArray): Boolean {
        val address = device.address
        val existingConnId = inboundByAddr[address]
        if (existingConnId == null) {
            if (value.size != COLUMBA_IDENTITY_LEN) {
                Log.w(TAG, "columba RX from $address before identity (${value.size}B), dropping")
                return false
            }
            val octets = parseMac(address) ?: return false
            val connId = nextConnId.getAndIncrement()
            val link = LinkState(connId, address, dialed = false, peerProtocol = BlePeerProtocol.Columba)
            link.central = columbaSubscribedCentrals[address] ?: device
            link.peerIdentity = value.copyOf()
            links[connId] = link
            inboundByAddr[address] = connId
            Log.i(TAG, "columba listener[$connId] $address identity ${value.size}B")
            NativeBridge.nativeBleColumbaLinkUp(
                connId,
                directBufferOf(octets),
                RSSI_NONE,
                false,
                directBufferOf(value),
            )
            return true
        }

        val link = links[existingConnId]
        if (link?.peerProtocol == BlePeerProtocol.Columba &&
            value.size == COLUMBA_IDENTITY_LEN &&
            link.peerIdentity?.contentEquals(value) == true
        ) {
            Log.i(TAG, "columba listener[$existingConnId] duplicate identity consumed")
            return true
        }

        return deliverGattInbound(existingConnId, true, value)
    }

    private fun deliverGattInbound(connId: Int, dataLane: Boolean, value: ByteArray): Boolean {
        val direct = ByteBuffer.allocateDirect(value.size)
        direct.put(value)
        val accepted =
            if (dataLane) {
                NativeBridge.nativeBleDataIn(connId, direct, value.size)
            } else {
                NativeBridge.nativeBleControlIn(connId, direct, value.size)
            }
        if (!accepted) {
            Log.w(TAG, "inbound Bluetooth LE queue full or closed[$connId] ${value.size} B")
        }
        return accepted
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

    private fun startRadioStatePump() {
        Thread {
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
        }.start()
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
        Thread {
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
        }.start()
        return true
    }

    private fun startL2capPumps(connId: Int, socket: BluetoothSocket) {
        Thread {
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
        }.start()
        Thread {
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
        }.start()
    }

    private fun startControlOutPump() {
        Thread {
            val direct = ByteBuffer.allocateDirect(CONTROL_CHUNK)
            val scratch = ByteArray(CONTROL_CHUNK)
            var generation = NativeBridge.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
                    continue
                }
                var any = false
                for (link in links.values) {
                    direct.clear()
                    val n = NativeBridge.nativeBleControlOut(link.connId, direct)
                    if (n > 0) {
                        any = true
                        direct.position(0)
                        direct.get(scratch, 0, n)
                        deliverControl(link, scratch.copyOf(n))
                    }
                }
                if (!any) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
                }
            }
        }.start()
    }

    private fun deliverControl(link: LinkState, payload: ByteArray) {
        if (link.peerProtocol == BlePeerProtocol.Columba) {
            return
        }
        if (link.dialed) {
            val char = link.clientControl ?: return
            gatedClientWrite(link, char, payload, BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT, "control")
        } else {
            val char = controlChar ?: return
            gatedServerNotify(link, char, payload, "control")
        }
    }

    private fun gatedClientWrite(
        link: LinkState,
        char: BluetoothGattCharacteristic,
        payload: ByteArray,
        type: Int,
        lane: String,
    ) {
        val gatt = link.clientGatt ?: return
        if (!link.sendGate.tryAcquire(SEND_GATE_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
            Log.w(TAG, "$lane write gate timeout[${link.connId}]")
            return
        }
        val result = try {
            writeGattCharacteristic(gatt, char, payload, type)
        } catch (e: Exception) {
            link.sendGate.release()
            Log.w(TAG, "$lane write[${link.connId}]: $e")
            return
        }
        if (result != BluetoothGatt.GATT_SUCCESS) {
            link.sendGate.release()
            Log.w(TAG, "$lane write rejected[${link.connId}] result=$result")
        }
    }

    private fun gatedServerNotify(
        link: LinkState,
        char: BluetoothGattCharacteristic,
        payload: ByteArray,
        lane: String,
    ) {
        val central = link.central ?: return
        val server = gattServer ?: return
        if (!link.sendGate.tryAcquire(SEND_GATE_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
            Log.w(TAG, "$lane notify gate timeout[${link.connId}]")
            return
        }
        val result = try {
            notifyGattCharacteristic(server, central, char, payload)
        } catch (e: Exception) {
            link.sendGate.release()
            Log.w(TAG, "$lane notify[${link.connId}]: $e")
            return
        }
        if (result != BluetoothGatt.GATT_SUCCESS) {
            link.sendGate.release()
            Log.w(TAG, "$lane notify rejected[${link.connId}] result=$result")
        }
    }

    private fun startDataOutPump() {
        Thread {
            val direct = ByteBuffer.allocateDirect(DATA_CHUNK)
            val scratch = ByteArray(DATA_CHUNK)
            var generation = NativeBridge.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
                    continue
                }
                var any = false
                for (link in links.values) {
                    direct.clear()
                    val n = NativeBridge.nativeBleDataOut(link.connId, direct)
                    if (n > 0) {
                        any = true
                        Log.i(TAG, "data out[${link.connId}] ${n}B")
                        direct.position(0)
                        direct.get(scratch, 0, n)
                        deliverData(link, scratch.copyOf(n))
                    }
                }
                if (!any) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
                }
            }
        }.start()
    }

    private fun deliverData(link: LinkState, payload: ByteArray) {
        if (link.dialed) {
            val char = link.clientData ?: return
            val gatt = link.clientGatt ?: return
            val writeType =
                if (link.peerProtocol == BlePeerProtocol.Columba && payload.size == COLUMBA_IDENTITY_LEN) {
                    BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT
                } else {
                    BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE
                }
            var attempts = 0
            while (attempts < DATA_WRITE_RETRIES) {
                val result = try {
                    writeGattCharacteristic(
                        gatt,
                        char,
                        payload,
                        writeType,
                    )
                } catch (e: Exception) {
                    Log.w(TAG, "data write[${link.connId}]: $e")
                    return
                }
                if (result == BluetoothGatt.GATT_SUCCESS) {
                    Log.i(TAG, "data write ok[${link.connId}] retries=$attempts")
                    return
                }
                if (result == ERROR_GATT_WRITE_REQUEST_BUSY) {
                    Thread.sleep(DATA_WRITE_RETRY_MS)
                    attempts++
                    continue
                }
                Log.w(TAG, "data write rejected[${link.connId}] result=$result")
                return
            }
            Log.w(TAG, "data write gave up[${link.connId}] after busy retries")
        } else {
            val char = if (link.peerProtocol == BlePeerProtocol.Columba) columbaTxChar else dataChar
            if (char == null) {
                return
            }
            gatedServerNotify(link, char, payload, "data")
        }
    }

    private fun startDialPump() {
        Thread {
            val direct = ByteBuffer.allocateDirect(6)
            val octets = ByteArray(6)
            var generation = NativeBridge.nativeBleWorkGeneration()
            while (running) {
                if (!radioActive) {
                    generation = NativeBridge.nativeBleWaitForWork(generation, 0)
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
        }.start()
    }

    private fun startL2capOpenPump() {
        Thread {
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
                var attempt = 0
                var opened = false
                while (attempt < L2CAP_OPEN_RETRIES && !opened && running && radioActive && links.containsKey(connId)) {
                    try {
                        val socket = device.createInsecureL2capChannel(psm)
                        socket.connect()
                        Log.i(TAG, "l2cap client[$connId] connected to psm=$psm attempt=$attempt")
                        link.l2capSocket = socket
                        NativeBridge.nativeBleL2capUp(connId)
                        startL2capPumps(connId, socket)
                        opened = true
                    } catch (e: Exception) {
                        attempt++
                        Log.w(TAG, "l2cap client[$connId] psm=$psm attempt=$attempt failed: $e")
                        if (attempt < L2CAP_OPEN_RETRIES) {
                            Thread.sleep(L2CAP_OPEN_RETRY_MS)
                        }
                    }
                }
                if (!opened) {
                    Log.w(TAG, "l2cap client[$connId] psm=$psm gave up after $attempt attempts; staying on GATT")
                }
            }
        }.start()
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
                    val mtuRequested = runCatching { gatt.requestMtu(MAX_ATT_MTU) }.getOrDefault(false)
                    Log.i(TAG, "dialer[$connId] connected; requested mtu=$mtuRequested")
                    if (!mtuRequested) {
                        requestClientServices(gatt, link, "mtu request rejected")
                    }
                    scheduleClientOpenFallback(connId, address, gatt)
                } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                    Log.i(TAG, "dialer[$connId] $address disconnected status=$status")
                    if (!linkedConnIds.remove(connId)) {
                        parseMac(address)?.let { octets ->
                            val direct = ByteBuffer.allocateDirect(6)
                            direct.put(octets)
                            NativeBridge.nativeBleDialFailed(direct)
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
                Log.i(TAG, "dialer[$connId] att mtu=$mtu status=$status")
                links[connId]?.let { requestClientServices(gatt, it, "mtu changed") }
            }

            override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
                if (!running || !radioActive) {
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
                        writeGattDescriptor(gatt, cccd, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
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
                if (!gatt.readCharacteristic(columbaIdentity)) {
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
                    writeGattDescriptor(gatt, cccd, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
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
                Log.i(
                    TAG,
                    "dialer[$connId] cccd ${descriptor.characteristic.uuid} status=$status",
                )
                if (descriptor.characteristic.uuid == COLUMBA_TX) {
                    val link = links[connId] ?: return
                    val identity = link.peerIdentity
                    if (status != BluetoothGatt.GATT_SUCCESS || identity == null) {
                        Log.w(TAG, "dialer[$connId] Columba TX subscribe failed status=$status")
                        runCatching { gatt.disconnect() }
                        return
                    }
                    Log.i(TAG, "dialer[$connId] $address subscribed (Columba TX ready)")
                    linkedConnIds.add(connId)
                    val octets = parseMac(address)
                    if (octets != null) {
                        NativeBridge.nativeBleColumbaLinkUp(
                            connId,
                            directBufferOf(octets),
                            RSSI_NONE,
                            true,
                            directBufferOf(identity),
                        )
                    }
                    return
                }
                if (descriptor.characteristic.uuid == NATIVE_CONTROL) {
                    val dataCccd = links[connId]?.clientData?.getDescriptor(CCCD)
                    if (dataCccd != null) {
                        writeGattDescriptor(
                            gatt,
                            dataCccd,
                            BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE,
                        )
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
                    NativeBridge.nativeBleLinkUp(connId, direct, RSSI_NONE, true)
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
                if (characteristic.uuid == NATIVE_CONTROL) {
                    links[connId]?.sendGate?.release()
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
                deliverGattInbound(connId, dataLane, value)
            }
        }

    private fun requestClientServices(gatt: BluetoothGatt, link: LinkState, reason: String) {
        if (!link.servicesRequested.compareAndSet(false, true)) {
            return
        }
        val started = runCatching { gatt.discoverServices() }.getOrDefault(false)
        Log.i(TAG, "dialer[${link.connId}] discovering services after $reason started=$started")
        if (!started) {
            runCatching { gatt.disconnect() }
        }
    }

    private fun scheduleClientOpenFallback(connId: Int, address: String, gatt: BluetoothGatt) {
        Thread {
            Thread.sleep(MTU_DISCOVERY_FALLBACK_MS)
            val link = links[connId]
            if (running && radioActive && link != null && !linkedConnIds.contains(connId)) {
                requestClientServices(gatt, link, "mtu callback timeout")
            }
            Thread.sleep(CLIENT_LINK_READY_TIMEOUT_MS - MTU_DISCOVERY_FALLBACK_MS)
            if (running && radioActive && links.containsKey(connId) && !linkedConnIds.contains(connId)) {
                Log.w(TAG, "dialer[$connId] $address did not become a Prns link; closing stale GATT")
                runCatching { gatt.disconnect() }
                closeLink(connId)
            }
        }.start()
    }

    private fun closeLink(connId: Int) {
        val link = links.remove(connId) ?: return
        inboundByAddr.remove(link.address, connId)
        columbaSubscribedCentrals.remove(link.address)
        dialingAddrs.remove(link.address)
        connectedAddrs.remove(link.address)
        runCatching { link.l2capSocket?.close() }
        runCatching { link.clientGatt?.close() }
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
            .setScanMode(ScanSettings.SCAN_MODE_LOW_POWER)
            .build()
        try {
            scanner.startScan(filters, settings, scanCallback)
            this.scanner = scanner
            Log.i(TAG, "scanning for service $PRNS_SERVICE")
        } catch (e: SecurityException) {
            Log.w(TAG, "scan permission denied: $e")
        }
    }

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
                byteArrayOf(PRNS_ROLE_VERSION, PRNS_ROLE_DUAL_MODE),
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
        inboundByAddr.clear()
        columbaSubscribedCentrals.clear()
        dialingAddrs.clear()
        connectedAddrs.clear()
        linkedConnIds.clear()
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

    private companion object {
        private const val TAG = "HopspotBle"
        private const val L2CAP_CHUNK = 2048
        private const val CONTROL_CHUNK = 64
        private const val SEND_GATE_TIMEOUT_MS = 1000L
        private const val DATA_CHUNK = 512
        private const val RADIO_STATE_RETRY_MS = 1_000L
        private const val RSSI_NONE = 127
        private const val MAX_ATT_MTU = 517
        private const val MTU_DISCOVERY_FALLBACK_MS = 750L
        private const val CLIENT_LINK_READY_TIMEOUT_MS = 8_000L
        private const val DATA_WRITE_RETRIES = 60
        private const val DATA_WRITE_RETRY_MS = 4L
        private const val ERROR_GATT_WRITE_REQUEST_BUSY = 201
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
