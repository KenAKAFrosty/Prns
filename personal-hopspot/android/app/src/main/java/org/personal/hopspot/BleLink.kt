package org.personal.hopspot

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
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
import android.os.ParcelUuid
import android.util.Log
import java.nio.ByteBuffer
import java.util.UUID

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
    private var central: BluetoothDevice? = null

    @Volatile
    private var l2capServer: BluetoothServerSocket? = null

    @Volatile
    private var l2capSocket: BluetoothSocket? = null

    @Volatile
    private var running = false

    private val scanCallback = object : ScanCallback() {
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            val record = result.scanRecord
            val name = record?.deviceName ?: "?"
            val services = record?.serviceUuids?.joinToString(",") { it.uuid.toString() } ?: "none"
            Log.i(TAG, "SIGHT ${result.device.address} rssi=${result.rssi} name=$name services=$services")
        }

        override fun onScanFailed(errorCode: Int) {
            Log.w(TAG, "scan failed code=$errorCode")
        }
    }

    private val advertiseCallback = object : AdvertiseCallback() {
        override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
            Log.i(TAG, "advertising $PRNS_SERVICE mode=${settingsInEffect.mode} txPower=${settingsInEffect.txPowerLevel}")
        }

        override fun onStartFailure(errorCode: Int) {
            Log.w(TAG, "advertise failed code=$errorCode")
        }
    }

    private val gattServerCallback = object : BluetoothGattServerCallback() {
        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            Log.i(TAG, "server conn ${device.address} status=$status state=$newState")
            if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                if (central?.address == device.address) {
                    central = null
                }
                NativeBridge.nativeBleDisconnected()
            }
        }

        override fun onServiceAdded(status: Int, service: BluetoothGattService) {
            Log.i(TAG, "server service added status=$status uuid=${service.uuid}")
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
            Log.i(TAG, "CONTROL-IN ${device.address} ${value.size}B: ${hex(value)}")
            val direct = ByteBuffer.allocateDirect(value.size)
            direct.put(value)
            NativeBridge.nativeBleControlIn(direct, value.size)
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
            Log.i(TAG, "subscribe ${device.address} cccd=${descriptor.uuid} value=${hex(value)}")
            central = device
            val octets = parseMac(device.address)
            if (octets != null) {
                val direct = ByteBuffer.allocateDirect(6)
                direct.put(octets)
                NativeBridge.nativeBleCentralReady(direct)
            }
            if (responseNeeded) {
                gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, null)
            }
        }

        override fun onNotificationSent(device: BluetoothDevice, status: Int) {
            Log.i(TAG, "notify-sent ${device.address} status=$status")
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
                Log.i(TAG, "l2cap accepted from ${socket.remoteDevice?.address}")
                l2capSocket = socket
                NativeBridge.nativeBleL2capUp()
                startL2capPumps(socket)
            }
        }.start()
    }

    private fun startL2capPumps(socket: BluetoothSocket) {
        Thread {
            val input = socket.inputStream
            val buf = ByteArray(L2CAP_CHUNK)
            val direct = ByteBuffer.allocateDirect(L2CAP_CHUNK)
            while (running) {
                val n = try {
                    input.read(buf)
                } catch (e: Exception) {
                    Log.w(TAG, "l2cap read ended: $e")
                    break
                }
                if (n < 0) break
                if (n > 0) {
                    direct.clear()
                    direct.put(buf, 0, n)
                    NativeBridge.nativeBleL2capIn(direct, n)
                }
            }
        }.start()
        Thread {
            val output = socket.outputStream
            val direct = ByteBuffer.allocateDirect(L2CAP_CHUNK)
            val scratch = ByteArray(L2CAP_CHUNK)
            while (running) {
                direct.clear()
                val n = NativeBridge.nativeBleL2capOut(direct)
                if (n > 0) {
                    direct.position(0)
                    direct.get(scratch, 0, n)
                    try {
                        output.write(scratch, 0, n)
                        output.flush()
                    } catch (e: Exception) {
                        Log.w(TAG, "l2cap write ended: $e")
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
                direct.clear()
                val n = NativeBridge.nativeBleControlOut(direct)
                if (n <= 0) {
                    Thread.sleep(IDLE_MS)
                    continue
                }
                direct.position(0)
                direct.get(scratch, 0, n)
                val payload = scratch.copyOf(n)
                val target = central
                val char = controlChar
                val server = gattServer
                if (target != null && char != null && server != null) {
                    try {
                        val result = server.notifyCharacteristicChanged(target, char, false, payload)
                        Log.i(TAG, "CONTROL-OUT ${n}B result=$result: ${hex(payload)}")
                    } catch (e: Exception) {
                        Log.w(TAG, "notify failed: $e")
                    }
                } else {
                    Log.w(TAG, "control-out ${n}B dropped — no subscribed central")
                }
            }
        }.start()
    }

    private fun startGattServer() {
        val manager = bluetoothManager
        if (manager == null) {
            Log.w(TAG, "no BluetoothManager")
            return
        }
        val server = try {
            manager.openGattServer(context, gattServerCallback)
        } catch (e: SecurityException) {
            Log.w(TAG, "openGattServer denied: $e")
            return
        }
        if (server == null) {
            Log.w(TAG, "openGattServer returned null")
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
        val service = BluetoothGattService(PRNS_SERVICE, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        service.addCharacteristic(control)
        try {
            server.addService(service)
            Log.i(TAG, "gatt server open; adding Prns service (control $NATIVE_CONTROL)")
        } catch (e: SecurityException) {
            Log.w(TAG, "addService denied: $e")
        }
    }

    private fun startScan(adapter: BluetoothAdapter) {
        val scanner = adapter.bluetoothLeScanner
        if (scanner == null) {
            Log.w(TAG, "no BLE scanner")
            return
        }
        this.scanner = scanner
        val filters = listOf(
            ScanFilter.Builder().setServiceUuid(ParcelUuid(PRNS_SERVICE)).build(),
        )
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
        val advertiser = adapter.bluetoothLeAdvertiser
        if (advertiser == null) {
            Log.w(TAG, "no BLE advertiser (peripheral role unsupported)")
            return
        }
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
        try {
            scanner?.stopScan(scanCallback)
        } catch (e: SecurityException) {
            Log.w(TAG, "stop scan: $e")
        }
        try {
            advertiser?.stopAdvertising(advertiseCallback)
        } catch (e: SecurityException) {
            Log.w(TAG, "stop advertise: $e")
        }
        try {
            gattServer?.close()
        } catch (e: SecurityException) {
            Log.w(TAG, "close server: $e")
        }
        runCatching { l2capSocket?.close() }
        runCatching { l2capServer?.close() }
        scanner = null
        advertiser = null
        gattServer = null
        controlChar = null
        central = null
        l2capSocket = null
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

    private companion object {
        private const val TAG = "HopspotBle"
        private const val L2CAP_CHUNK = 2048
        private const val CONTROL_CHUNK = 64
        private const val IDLE_MS = 2L
        val PRNS_SERVICE: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e3")
        val NATIVE_CONTROL: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e7")
        val CCCD: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")

        fun hex(bytes: ByteArray): String = bytes.joinToString(" ") { "%02x".format(it) }
    }
}
