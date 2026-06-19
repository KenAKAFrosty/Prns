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
            Log.i(TAG, "CONTROL-IN ${device.address} ${value.size}B prepared=$preparedWrite off=$offset: ${hex(value)}")
            if (responseNeeded) {
                gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, null)
            }
        }

        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic,
        ) {
            Log.i(TAG, "read-req ${device.address} ${characteristic.uuid} off=$offset")
            gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, ByteArray(0))
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
        startGattServer()
        startScan(adapter)
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
        scanner = null
        advertiser = null
        gattServer = null
    }

    private companion object {
        private const val TAG = "HopspotBle"
        val PRNS_SERVICE: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e3")
        val NATIVE_CONTROL: UUID = UUID.fromString("37145b00-442d-4a94-917f-8f42c5da28e7")
        val CCCD: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")

        fun hex(bytes: ByteArray): String = bytes.joinToString(" ") { "%02x".format(it) }
    }
}
