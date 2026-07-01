package org.personal.hopspot

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbConstants
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbDeviceConnection
import android.hardware.usb.UsbEndpoint
import android.hardware.usb.UsbInterface
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
import com.hoho.android.usbserial.driver.CdcAcmSerialDriver
import com.hoho.android.usbserial.driver.UsbSerialDriver
import com.hoho.android.usbserial.driver.UsbSerialPort
import com.hoho.android.usbserial.driver.UsbSerialProber
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException
import java.nio.ByteBuffer

class UsbLink(private val context: Context) {
    private val usbManager = context.getSystemService(Context.USB_SERVICE) as UsbManager
    private val rxBuffer = ByteBuffer.allocateDirect(RX_CAPACITY)

    @Volatile
    private var session: UsbSession? = null

    @Volatile
    private var running = false

    @Volatile
    private var pumpGeneration = 0

    @Volatile
    private var recoveryGeneration = 0

    @Volatile
    private var scanning = false

    @Volatile
    private var permissionPending = false

    @Volatile
    private var rxTotal = 0L

    @Volatile
    private var txTotal = 0L

    @Volatile
    private var lastRxLogMs = 0L

    @Volatile
    private var lastTxLogMs = 0L

    @Volatile
    private var sessionStartedMs = 0L

    @Volatile
    private var lastRxMs = 0L

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent) {
            when (intent.action) {
                ACTION_USB_PERMISSION -> {
                    permissionPending = false
                    val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
                    Log.i(TAG, "permission result granted=$granted")
                    if (granted) connect()
                }
                UsbManager.ACTION_USB_DEVICE_ATTACHED -> {
                    Log.i(TAG, "device attached")
                    connect()
                }
                UsbManager.ACTION_USB_DEVICE_DETACHED -> {
                    Log.i(TAG, "device detached")
                    disconnect()
                }
                UsbManager.ACTION_USB_ACCESSORY_ATTACHED -> {
                    Log.i(TAG, "accessory attached")
                    connect()
                }
                UsbManager.ACTION_USB_ACCESSORY_DETACHED -> {
                    Log.i(TAG, "accessory detached")
                    disconnect()
                }
            }
        }
    }

    fun start() {
        val filter = IntentFilter().apply {
            addAction(ACTION_USB_PERMISSION)
            addAction(UsbManager.ACTION_USB_DEVICE_ATTACHED)
            addAction(UsbManager.ACTION_USB_DEVICE_DETACHED)
            addAction(UsbManager.ACTION_USB_ACCESSORY_ATTACHED)
            addAction(UsbManager.ACTION_USB_ACCESSORY_DETACHED)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            context.registerReceiver(receiver, filter)
        }
        Log.i(TAG, "start; scanning for a USB Auto device")
        scanning = true
        Thread {
            while (scanning) {
                if (session == null) connect()
                Thread.sleep(SCAN_INTERVAL_MS)
            }
        }.start()
    }

    fun stop() {
        scanning = false
        disconnect()
        runCatching { context.unregisterReceiver(receiver) }
    }

    @Synchronized
    private fun connect() {
        if (session != null) return
        val candidate = findCandidate() ?: return
        if (!candidate.hasPermission(usbManager)) {
            if (!permissionPending) {
                permissionPending = true
                Log.i(
                    TAG,
                    "found ${candidate.description}; requesting permission",
                )
                requestPermission(candidate)
            }
            return
        }
        Log.i(TAG, "opening ${candidate.description}")
        open(candidate)
    }

    @Synchronized
    private fun open(candidate: UsbCandidate) {
        if (session != null) return
        val opened = when (candidate) {
            is UsbCandidate.Serial -> openSerial(candidate.driver)
            is UsbCandidate.Bulk -> openBulk(candidate.device, candidate.endpoints)
            is UsbCandidate.Accessory -> openAccessory(candidate.accessory)
        } ?: return
        session = opened
        rxTotal = 0
        txTotal = 0
        sessionStartedMs = System.currentTimeMillis()
        lastRxMs = sessionStartedMs
        NativeBridge.nativeUsbConnected(true)
        Log.i(TAG, "${opened.description} open; nativeUsbConnected(true)")
        startPumps(opened)
    }

    private fun openSerial(driver: UsbSerialDriver): UsbSession? {
        val connection = usbManager.openDevice(driver.device)
        if (connection == null) {
            Log.w(TAG, "openDevice returned null")
            return null
        }
        val serialPort = driver.ports.firstOrNull()
        if (serialPort == null) {
            Log.w(TAG, "driver exposes no ports")
            connection.close()
            return null
        }
        return try {
            serialPort.open(connection)
            serialPort.setParameters(BAUD, 8, UsbSerialPort.STOPBITS_1, UsbSerialPort.PARITY_NONE)
            runCatching {
                serialPort.setRTS(false)
                serialPort.setDTR(false)
            }.onFailure { Log.w(TAG, "control lines: $it") }
            SerialSession(serialPort)
        } catch (e: Exception) {
            Log.w(TAG, "serial open failed: $e")
            runCatching { serialPort.close() }
            connection.close()
            null
        }
    }

    private fun openBulk(device: UsbDevice, endpoints: BulkEndpoints): UsbSession? {
        val connection = usbManager.openDevice(device)
        if (connection == null) {
            Log.w(TAG, "openDevice returned null")
            return null
        }
        return try {
            if (!connection.claimInterface(endpoints.usbInterface, true)) {
                Log.w(TAG, "claimInterface failed")
                connection.close()
                null
            } else {
                BulkSession(connection, endpoints)
            }
        } catch (e: Exception) {
            Log.w(TAG, "bulk open failed: $e")
            runCatching { connection.releaseInterface(endpoints.usbInterface) }
            connection.close()
            null
        }
    }

    private fun openAccessory(accessory: UsbAccessory): UsbSession? {
        val descriptor = usbManager.openAccessory(accessory)
        if (descriptor == null) {
            Log.w(TAG, "openAccessory returned null")
            return null
        }
        return AccessorySession(descriptor)
    }

    private fun startPumps(opened: UsbSession) {
        running = true
        val generation = nextPumpGeneration()

        Thread {
            Thread.sleep(STARTUP_RX_WATCHDOG_MS)
            val now = System.currentTimeMillis()
            if (generation == pumpGeneration && shouldRecoverStartupWithoutRx(now)) {
                Log.w(
                    TAG,
                    "startup RX watchdog: tx=$txTotal rx=$rxTotal after " +
                        "${now - sessionStartedMs}ms; reopening USB Auto",
                )
                recoverAfterIoError()
            }
        }.start()

        Thread {
            val scratch = ByteArray(RX_CAPACITY)
            while (running && generation == pumpGeneration) {
                val n = try {
                    opened.read(scratch, READ_TIMEOUT_MS)
                } catch (e: Exception) {
                    Log.w(TAG, "read: $e")
                    if (generation == pumpGeneration) recoverAfterIoError()
                    break
                }
                if (n <= 0) continue
                val now = System.currentTimeMillis()
                if (now - lastRxLogMs > 700) {
                    lastRxLogMs = now
                    val head = scratch.take(minOf(n, 28)).joinToString(" ") { "%02x".format(it) }
                    Log.i(TAG, "RX ${n}B total=${rxTotal + n}: $head")
                }
                rxTotal += n
                lastRxMs = now
                rxBuffer.clear()
                rxBuffer.put(scratch, 0, minOf(n, rxBuffer.capacity()))
                NativeBridge.nativeUsbRx(rxBuffer, minOf(n, rxBuffer.capacity()))
            }
        }.start()

        Thread {
            val txBuffer = ByteBuffer.allocateDirect(TX_CAPACITY)
            val scratch = ByteArray(TX_CAPACITY)
            while (running && generation == pumpGeneration) {
                txBuffer.clear()
                val n = NativeBridge.nativeUsbTx(txBuffer)
                if (n > 0) {
                    txTotal += n
                    val now = System.currentTimeMillis()
                    if (now - lastTxLogMs > 700) {
                        lastTxLogMs = now
                        Log.i(TAG, "TX ${n}B total=$txTotal")
                    }
                    txBuffer.position(0)
                    txBuffer.get(scratch, 0, n)
                    if (generation != pumpGeneration) break
                    try {
                        opened.write(scratch, n, WRITE_TIMEOUT_MS)
                    } catch (e: Exception) {
                        Log.w(TAG, "write: $e")
                        if (generation == pumpGeneration) recoverAfterIoError()
                        break
                    }
                } else {
                    Thread.sleep(IDLE_SLEEP_MS)
                }
            }
        }.start()
    }

    private fun disconnect() {
        closeSession(reportDisconnected = true, reason = "disconnect")
    }

    private fun recoverAfterIoError() {
        closeSession(reportDisconnected = false, reason = "recover")
        val generation = nextRecoveryGeneration()
        Thread {
            Thread.sleep(RECONNECT_GRACE_MS)
            if (!scanning || session != null || generation != recoveryGeneration) return@Thread
            if (findCandidate() == null) {
                Log.i(TAG, "recovery grace expired; no USB Auto device present")
                NativeBridge.nativeUsbConnected(false)
            } else {
                Log.i(TAG, "recovery grace expired; USB Auto device still present")
                connect()
            }
        }.start()
    }

    private fun shouldRecoverStartupWithoutRx(now: Long): Boolean =
        txTotal >= STARTUP_RX_WATCHDOG_MIN_TX_BYTES &&
            rxTotal == 0L &&
            now - sessionStartedMs >= STARTUP_RX_WATCHDOG_MS &&
            now - lastRxMs >= STARTUP_RX_WATCHDOG_MS

    @Synchronized
    private fun closeSession(reportDisconnected: Boolean, reason: String) {
        val opened = session
        if (opened == null) {
            if (reportDisconnected) {
                recoveryGeneration += 1
                NativeBridge.nativeUsbConnected(false)
            }
            return
        }
        Log.i(TAG, "$reason (rx=$rxTotal tx=$txTotal reportDisconnected=$reportDisconnected)")
        running = false
        pumpGeneration += 1
        if (reportDisconnected) recoveryGeneration += 1
        if (reportDisconnected) NativeBridge.nativeUsbConnected(false)
        runCatching { opened.close() }
        session = null
    }

    @Synchronized
    private fun nextPumpGeneration(): Int {
        pumpGeneration += 1
        return pumpGeneration
    }

    @Synchronized
    private fun nextRecoveryGeneration(): Int {
        recoveryGeneration += 1
        return recoveryGeneration
    }

    private fun requestPermission(candidate: UsbCandidate) {
        val intent = Intent(ACTION_USB_PERMISSION).setPackage(context.packageName)
        val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            PendingIntent.FLAG_MUTABLE
        } else {
            0
        }
        val pending =
            PendingIntent.getBroadcast(context, 0, intent, flags)
        when (candidate) {
            is UsbCandidate.Accessory -> usbManager.requestPermission(candidate.accessory, pending)
            is UsbCandidate.Bulk -> usbManager.requestPermission(candidate.device, pending)
            is UsbCandidate.Serial -> usbManager.requestPermission(candidate.device, pending)
        }
    }

    private fun findCandidate(): UsbCandidate? =
        findAccessoryCandidate()
            ?: findBulkCandidate()
            ?: findSerialDriver()?.let { UsbCandidate.Serial(it) }

    private fun findAccessoryCandidate(): UsbCandidate.Accessory? =
        usbManager.accessoryList
            ?.asSequence()
            ?.filter(::isPrnsAccessory)
            ?.map { UsbCandidate.Accessory(it) }
            ?.firstOrNull()

    private fun isPrnsAccessory(accessory: UsbAccessory): Boolean =
        accessory.manufacturer == NativeBridge.nativeUsbAccessoryManufacturer() &&
            accessory.model == NativeBridge.nativeUsbAccessoryModel()

    private fun findBulkCandidate(): UsbCandidate.Bulk? {
        val vendorId = NativeBridge.nativeUsbAutoVendorId()
        val productId = NativeBridge.nativeUsbAutoProductId()
        return usbManager.deviceList.values
            .asSequence()
            .filter { it.vendorId == vendorId && it.productId == productId }
            .mapNotNull { device ->
                findBulkEndpoints(device)?.let { UsbCandidate.Bulk(device, it) }
            }
            .firstOrNull()
    }

    private fun findBulkEndpoints(device: UsbDevice): BulkEndpoints? {
        for (interfaceIndex in 0 until device.interfaceCount) {
            val usbInterface = device.getInterface(interfaceIndex)
            if (usbInterface.interfaceClass != UsbConstants.USB_CLASS_VENDOR_SPEC) {
                continue
            }
            val inEndpoint = findBulkEndpoint(usbInterface, UsbConstants.USB_DIR_IN)
            val outEndpoint = findBulkEndpoint(usbInterface, UsbConstants.USB_DIR_OUT)
            if (inEndpoint != null && outEndpoint != null) {
                return BulkEndpoints(usbInterface, inEndpoint, outEndpoint)
            }
        }
        return null
    }

    private fun findBulkEndpoint(usbInterface: UsbInterface, direction: Int): UsbEndpoint? {
        for (endpointIndex in 0 until usbInterface.endpointCount) {
            val endpoint = usbInterface.getEndpoint(endpointIndex)
            if (
                endpoint.type == UsbConstants.USB_ENDPOINT_XFER_BULK &&
                endpoint.direction == direction
            ) {
                return endpoint
            }
        }
        return null
    }

    private fun prober(): UsbSerialProber {
        val table = UsbSerialProber.getDefaultProbeTable()
        table.addProduct(ESP_VENDOR_ID, ESP_PRODUCT_ID, CdcAcmSerialDriver::class.java)
        table.addProduct(
            NativeBridge.nativeUsbAutoVendorId(),
            NativeBridge.nativeUsbAutoProductId(),
            CdcAcmSerialDriver::class.java,
        )
        return UsbSerialProber(table)
    }

    private fun findSerialDriver(): UsbSerialDriver? =
        prober().findAllDrivers(usbManager).firstOrNull()

    private sealed class UsbCandidate {
        abstract val description: String

        abstract fun hasPermission(usbManager: UsbManager): Boolean

        data class Bulk(
            val device: UsbDevice,
            val endpoints: BulkEndpoints,
        ) : UsbCandidate() {
            override val description: String = "Prns vendor bulk USB Auto"

            override fun hasPermission(usbManager: UsbManager): Boolean =
                usbManager.hasPermission(device)
        }

        data class Serial(
            val driver: UsbSerialDriver,
        ) : UsbCandidate() {
            override val description: String = "CDC serial USB Auto"
            val device: UsbDevice = driver.device

            override fun hasPermission(usbManager: UsbManager): Boolean =
                usbManager.hasPermission(device)
        }

        data class Accessory(
            val accessory: UsbAccessory,
        ) : UsbCandidate() {
            override val description: String = "Android accessory USB Auto"

            override fun hasPermission(usbManager: UsbManager): Boolean =
                usbManager.hasPermission(accessory)
        }
    }

    private data class BulkEndpoints(
        val usbInterface: UsbInterface,
        val inEndpoint: UsbEndpoint,
        val outEndpoint: UsbEndpoint,
    )

    private interface UsbSession {
        val description: String

        fun read(buffer: ByteArray, timeoutMs: Int): Int

        fun write(buffer: ByteArray, len: Int, timeoutMs: Int)

        fun close()
    }

    private class SerialSession(
        private val port: UsbSerialPort,
    ) : UsbSession {
        override val description: String = "CDC serial USB Auto"

        override fun read(buffer: ByteArray, timeoutMs: Int): Int =
            port.read(buffer, timeoutMs).coerceAtLeast(0)

        override fun write(buffer: ByteArray, len: Int, timeoutMs: Int) {
            port.write(buffer.copyOf(len), timeoutMs)
        }

        override fun close() {
            port.close()
        }
    }

    private class BulkSession(
        private val connection: UsbDeviceConnection,
        private val endpoints: BulkEndpoints,
    ) : UsbSession {
        override val description: String = "Prns vendor bulk USB Auto"

        override fun read(buffer: ByteArray, timeoutMs: Int): Int =
            connection.bulkTransfer(endpoints.inEndpoint, buffer, buffer.size, timeoutMs)
                .coerceAtLeast(0)

        override fun write(buffer: ByteArray, len: Int, timeoutMs: Int) {
            val written = connection.bulkTransfer(
                endpoints.outEndpoint,
                buffer,
                len,
                timeoutMs,
            )
            if (written != len) {
                throw IllegalStateException("bulk write wrote $written/$len bytes")
            }
        }

        override fun close() {
            runCatching { connection.releaseInterface(endpoints.usbInterface) }
            connection.close()
        }
    }

    private class AccessorySession(
        private val descriptor: ParcelFileDescriptor,
    ) : UsbSession {
        private val input = FileInputStream(descriptor.fileDescriptor)
        private val output = FileOutputStream(descriptor.fileDescriptor)

        override val description: String = "Android accessory USB Auto"

        override fun read(buffer: ByteArray, timeoutMs: Int): Int {
            val n = input.read(buffer)
            if (n < 0) throw IOException("accessory stream closed")
            return n
        }

        override fun write(buffer: ByteArray, len: Int, timeoutMs: Int) {
            output.write(buffer, 0, len)
            output.flush()
        }

        override fun close() {
            runCatching { input.close() }
            runCatching { output.close() }
            descriptor.close()
        }
    }

    companion object {
        private const val TAG = "HopspotUsb"
        private const val ACTION_USB_PERMISSION = "org.personal.hopspot.USB_PERMISSION"
        private const val BAUD = 115200
        private const val RX_CAPACITY = 16 * 1024
        private const val TX_CAPACITY = 4 * 1024
        private const val READ_TIMEOUT_MS = 100
        private const val WRITE_TIMEOUT_MS = 200
        private const val IDLE_SLEEP_MS = 2L
        private const val SCAN_INTERVAL_MS = 1000L
        private const val RECONNECT_GRACE_MS = 3000L
        private const val STARTUP_RX_WATCHDOG_MS = 3500L
        private const val STARTUP_RX_WATCHDOG_MIN_TX_BYTES = 1L
        private const val ESP_VENDOR_ID = 0x303A
        private const val ESP_PRODUCT_ID = 0x1001
    }
}
