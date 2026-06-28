package org.personal.hopspot

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
import android.bluetooth.BluetoothStatusCodes
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
import android.os.ParcelUuid
import android.util.Log
import java.nio.ByteBuffer
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

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
    private var l2capServer: BluetoothServerSocket? = null

    @Volatile
    private var running = false

    private val nextConnId = AtomicInteger(1)
    private val links = ConcurrentHashMap<Int, LinkState>()
    private val inboundByAddr = ConcurrentHashMap<String, Int>()
    private val devices = ConcurrentHashMap<String, BluetoothDevice>()

    private class LinkState(val connId: Int, val address: String, val dialed: Boolean) {
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
        var l2capSocket: BluetoothSocket? = null
    }

    private val scanCallback = object : ScanCallback() {
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            val device = result.device
            devices[device.address] = device
            val octets = parseMac(device.address) ?: return
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
            startAdvertise(adapter)
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
            val connId = inboundByAddr[device.address]
            if (connId != null) {
                val direct = ByteBuffer.allocateDirect(value.size)
                direct.put(value)
                if (characteristic.uuid == NATIVE_DATA) {
                    NativeBridge.nativeBleDataIn(connId, direct, value.size)
                } else {
                    NativeBridge.nativeBleControlIn(connId, direct, value.size)
                }
            }
            if (responseNeeded) {
                gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, null)
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
            val subscribing = value.isNotEmpty() && value[0].toInt() != 0
            if (subscribing && inboundByAddr[device.address] == null) {
                val connId = nextConnId.getAndIncrement()
                val link = LinkState(connId, device.address, dialed = false)
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
            inboundByAddr[device.address]?.let { links[it]?.sendGate?.release() }
        }
    }

    fun start() {
        val adapter = adapter
        if (adapter == null || !adapter.isEnabled) {
            Log.w(TAG, "bluetooth adapter unavailable or off")
            return
        }
        running = true
        startL2capListener(adapter)
        startGattServer()
        startScan(adapter)
        startControlOutPump()
        startDataOutPump()
        startDialPump()
        startL2capOpenPump()
    }

    private fun startL2capListener(adapter: BluetoothAdapter) {
        val server = try {
            adapter.listenUsingInsecureL2capChannel()
        } catch (e: Exception) {
            Log.w(TAG, "l2cap listen failed: $e")
            return
        }
        l2capServer = server
        val psm = server.psm
        Log.i(TAG, "l2cap listener published psm=$psm")
        NativeBridge.nativeBleSetPsm(psm)
        Thread {
            while (running) {
                val socket = try {
                    server.accept()
                } catch (e: Exception) {
                    Log.w(TAG, "l2cap accept ended: $e")
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
    }

    private fun startL2capPumps(connId: Int, socket: BluetoothSocket) {
        Thread {
            val input = socket.inputStream
            val buf = ByteArray(L2CAP_CHUNK)
            val direct = ByteBuffer.allocateDirect(L2CAP_CHUNK)
            while (running && links.containsKey(connId)) {
                val n = try {
                    input.read(buf)
                } catch (e: Exception) {
                    break
                }
                if (n < 0) break
                if (n > 0) {
                    direct.clear()
                    direct.put(buf, 0, n)
                    NativeBridge.nativeBleL2capIn(connId, direct, n)
                }
            }
            closeLink(connId)
        }.start()
        Thread {
            val output = socket.outputStream
            val direct = ByteBuffer.allocateDirect(L2CAP_CHUNK)
            val scratch = ByteArray(L2CAP_CHUNK)
            while (running && links.containsKey(connId)) {
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
                    Thread.sleep(IDLE_MS)
                }
            }
        }.start()
    }

    private fun startControlOutPump() {
        Thread {
            val direct = ByteBuffer.allocateDirect(CONTROL_CHUNK)
            val scratch = ByteArray(CONTROL_CHUNK)
            while (running) {
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
                    Thread.sleep(IDLE_MS)
                }
            }
        }.start()
    }

    private fun deliverControl(link: LinkState, payload: ByteArray) {
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
            gatt.writeCharacteristic(char, payload, type)
        } catch (e: Exception) {
            link.sendGate.release()
            Log.w(TAG, "$lane write[${link.connId}]: $e")
            return
        }
        if (result != BluetoothStatusCodes.SUCCESS) {
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
            server.notifyCharacteristicChanged(central, char, false, payload)
        } catch (e: Exception) {
            link.sendGate.release()
            Log.w(TAG, "$lane notify[${link.connId}]: $e")
            return
        }
        if (result != BluetoothStatusCodes.SUCCESS) {
            link.sendGate.release()
            Log.w(TAG, "$lane notify rejected[${link.connId}] result=$result")
        }
    }

    private fun startDataOutPump() {
        Thread {
            val direct = ByteBuffer.allocateDirect(DATA_CHUNK)
            val scratch = ByteArray(DATA_CHUNK)
            while (running) {
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
                    Thread.sleep(IDLE_MS)
                }
            }
        }.start()
    }

    private fun deliverData(link: LinkState, payload: ByteArray) {
        if (link.dialed) {
            val char = link.clientData ?: return
            val gatt = link.clientGatt ?: return
            var attempts = 0
            while (attempts < DATA_WRITE_RETRIES) {
                val result = try {
                    gatt.writeCharacteristic(
                        char,
                        payload,
                        BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE,
                    )
                } catch (e: Exception) {
                    Log.w(TAG, "data write[${link.connId}]: $e")
                    return
                }
                if (result == BluetoothStatusCodes.SUCCESS) {
                    Log.i(TAG, "data write ok[${link.connId}] retries=$attempts")
                    return
                }
                if (result == BluetoothStatusCodes.ERROR_GATT_WRITE_REQUEST_BUSY) {
                    Thread.sleep(DATA_WRITE_RETRY_MS)
                    attempts++
                    continue
                }
                Log.w(TAG, "data write rejected[${link.connId}] result=$result")
                return
            }
            Log.w(TAG, "data write gave up[${link.connId}] after busy retries")
        } else {
            val char = dataChar ?: return
            gatedServerNotify(link, char, payload, "data")
        }
    }

    private fun startDialPump() {
        Thread {
            val direct = ByteBuffer.allocateDirect(6)
            val octets = ByteArray(6)
            while (running) {
                direct.clear()
                if (!NativeBridge.nativeBleNextDial(direct)) {
                    Thread.sleep(IDLE_MS)
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
                links[connId] = LinkState(connId, address, dialed = true)
                device.connectGatt(context, false, clientCallback(connId, address), BluetoothDevice.TRANSPORT_LE)
            }
        }.start()
    }

    private fun startL2capOpenPump() {
        Thread {
            val direct = ByteBuffer.allocateDirect(6)
            val raw = ByteArray(6)
            while (running) {
                direct.clear()
                if (!NativeBridge.nativeBleNextL2capOpen(direct)) {
                    Thread.sleep(IDLE_MS)
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
                while (attempt < L2CAP_OPEN_RETRIES && !opened && running && links.containsKey(connId)) {
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
                if (newState == BluetoothProfile.STATE_CONNECTED) {
                    val link = links[connId] ?: return
                    link.clientGatt = gatt
                    connectedAddrs.add(address)
                    runCatching {
                        gatt.requestConnectionPriority(BluetoothGatt.CONNECTION_PRIORITY_HIGH)
                    }
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
                Log.i(TAG, "dialer[$connId] att mtu=$mtu status=$status")
                links[connId]?.let { requestClientServices(gatt, it, "mtu changed") }
            }

            override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
                if (status != BluetoothGatt.GATT_SUCCESS) {
                    Log.w(TAG, "dialer[$connId] service discovery failed status=$status")
                    runCatching { gatt.disconnect() }
                    return
                }
                val service = gatt.getService(PRNS_SERVICE)
                val control = service?.getCharacteristic(NATIVE_CONTROL)
                if (control == null) {
                    Log.w(TAG, "dialer[$connId] no Prns control characteristic")
                    runCatching { gatt.disconnect() }
                    return
                }
                val data = service.getCharacteristic(NATIVE_DATA)
                links[connId]?.clientControl = control
                links[connId]?.clientData = data
                if (data != null) {
                    gatt.setCharacteristicNotification(data, true)
                }
                gatt.setCharacteristicNotification(control, true)
                val cccd = control.getDescriptor(CCCD)
                if (cccd != null) {
                    gatt.writeDescriptor(cccd, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
                }
            }

            override fun onDescriptorWrite(
                gatt: BluetoothGatt,
                descriptor: BluetoothGattDescriptor,
                status: Int,
            ) {
                Log.i(
                    TAG,
                    "dialer[$connId] cccd ${descriptor.characteristic.uuid} status=$status",
                )
                if (descriptor.characteristic.uuid == NATIVE_CONTROL) {
                    val dataCccd = links[connId]?.clientData?.getDescriptor(CCCD)
                    if (dataCccd != null) {
                        gatt.writeDescriptor(
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
                if (characteristic.uuid == NATIVE_CONTROL) {
                    links[connId]?.sendGate?.release()
                }
            }

            override fun onCharacteristicChanged(
                gatt: BluetoothGatt,
                characteristic: BluetoothGattCharacteristic,
                value: ByteArray,
            ) {
                val lane = if (characteristic.uuid == NATIVE_DATA) "DATA" else "CONTROL"
                Log.i(TAG, "dialer[$connId] notify $lane ${value.size}B")
                val direct = ByteBuffer.allocateDirect(value.size)
                direct.put(value)
                if (characteristic.uuid == NATIVE_DATA) {
                    NativeBridge.nativeBleDataIn(connId, direct, value.size)
                } else {
                    NativeBridge.nativeBleControlIn(connId, direct, value.size)
                }
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
            if (running && link != null && !linkedConnIds.contains(connId)) {
                requestClientServices(gatt, link, "mtu callback timeout")
            }
            Thread.sleep(CLIENT_LINK_READY_TIMEOUT_MS - MTU_DISCOVERY_FALLBACK_MS)
            if (running && links.containsKey(connId) && !linkedConnIds.contains(connId)) {
                Log.w(TAG, "dialer[$connId] $address did not become a Prns link; closing stale GATT")
                runCatching { gatt.disconnect() }
                closeLink(connId)
            }
        }.start()
    }

    private fun closeLink(connId: Int) {
        val link = links.remove(connId) ?: return
        inboundByAddr.remove(link.address, connId)
        dialingAddrs.remove(link.address)
        connectedAddrs.remove(link.address)
        runCatching { link.l2capSocket?.close() }
        runCatching { link.clientGatt?.close() }
        NativeBridge.nativeBleDisconnected(connId)
    }

    private fun startGattServer() {
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
        val service = BluetoothGattService(PRNS_SERVICE, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        service.addCharacteristic(control)
        service.addCharacteristic(data)
        runCatching { server.addService(service) }
        Log.i(TAG, "gatt server open; Prns service added")
    }

    private fun startScan(adapter: BluetoothAdapter) {
        val scanner = adapter.bluetoothLeScanner ?: return
        this.scanner = scanner
        val filters = listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(PRNS_SERVICE)).build())
        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
            .build()
        try {
            scanner.startScan(filters, settings, scanCallback)
            Log.i(TAG, "scanning for service $PRNS_SERVICE")
        } catch (e: SecurityException) {
            Log.w(TAG, "scan permission denied: $e")
        }
    }

    private fun startAdvertise(adapter: BluetoothAdapter) {
        val advertiser = adapter.bluetoothLeAdvertiser ?: return
        this.advertiser = advertiser
        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
            .setConnectable(true)
            .setTimeout(0)
            .build()
        val data = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(ParcelUuid(PRNS_SERVICE))
            .build()
        try {
            advertiser.startAdvertising(settings, data, advertiseCallback)
        } catch (e: SecurityException) {
            Log.w(TAG, "advertise permission denied: $e")
        }
    }

    fun stop() {
        running = false
        runCatching { scanner?.stopScan(scanCallback) }
        runCatching { advertiser?.stopAdvertising(advertiseCallback) }
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
        l2capServer = null
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

    private fun formatMac(octets: ByteArray): String =
        octets.joinToString(":") { "%02X".format(it) }

    private companion object {
        private const val TAG = "HopspotBle"
        private const val L2CAP_CHUNK = 2048
        private const val CONTROL_CHUNK = 64
        private const val SEND_GATE_TIMEOUT_MS = 1000L
        private const val DATA_CHUNK = 512
        private const val IDLE_MS = 2L
        private const val RSSI_NONE = 127
        private const val MAX_ATT_MTU = 517
        private const val MTU_DISCOVERY_FALLBACK_MS = 750L
        private const val CLIENT_LINK_READY_TIMEOUT_MS = 8_000L
        private const val DATA_WRITE_RETRIES = 60
        private const val DATA_WRITE_RETRY_MS = 4L
        private const val L2CAP_OPEN_RETRIES = 5
        private const val L2CAP_OPEN_RETRY_MS = 200L
        val PRNS_SERVICE: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e3")
        val NATIVE_CONTROL: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e7")
        val NATIVE_DATA: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e8")
        val CCCD: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
    }

    private val dialingAddrs = ConcurrentHashMap.newKeySet<String>()
    private val connectedAddrs = ConcurrentHashMap.newKeySet<String>()
    private val linkedConnIds = ConcurrentHashMap.newKeySet<Int>()
}
