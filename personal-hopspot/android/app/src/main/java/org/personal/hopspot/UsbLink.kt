package org.personal.hopspot

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbManager
import android.util.Log
import androidx.core.content.ContextCompat
import com.hoho.android.usbserial.driver.CdcAcmSerialDriver
import com.hoho.android.usbserial.driver.UsbSerialDriver
import com.hoho.android.usbserial.driver.UsbSerialPort
import com.hoho.android.usbserial.driver.UsbSerialProber
import com.hoho.android.usbserial.util.SerialInputOutputManager
import java.nio.ByteBuffer

class UsbLink(private val context: Context) {
    private val usbManager = context.getSystemService(Context.USB_SERVICE) as UsbManager
    private val rxBuffer = ByteBuffer.allocateDirect(RX_CAPACITY)

    private var port: UsbSerialPort? = null
    private var ioManager: SerialInputOutputManager? = null

    @Volatile
    private var running = false

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
            }
        }
    }

    fun start() {
        val filter = IntentFilter().apply {
            addAction(ACTION_USB_PERMISSION)
            addAction(UsbManager.ACTION_USB_DEVICE_ATTACHED)
            addAction(UsbManager.ACTION_USB_DEVICE_DETACHED)
        }
        ContextCompat.registerReceiver(
            context,
            receiver,
            filter,
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        Log.i(TAG, "start; scanning for a serial device")
        scanning = true
        Thread {
            while (scanning) {
                if (port == null) connect()
                Thread.sleep(SCAN_INTERVAL_MS)
            }
        }.start()
    }

    fun stop() {
        scanning = false
        disconnect()
        runCatching { context.unregisterReceiver(receiver) }
    }

    private fun connect() {
        if (port != null) return
        val driver = prober().findAllDrivers(usbManager).firstOrNull() ?: return
        if (!usbManager.hasPermission(driver.device)) {
            if (!permissionPending) {
                permissionPending = true
                Log.i(
                    TAG,
                    "found device vid=${driver.device.vendorId} pid=${driver.device.productId}; requesting permission",
                )
                requestPermission(driver.device)
            }
            return
        }
        Log.i(TAG, "opening device, ports=${driver.ports.size}")
        open(driver)
    }

    private fun open(driver: UsbSerialDriver) {
        val connection = usbManager.openDevice(driver.device)
        if (connection == null) {
            Log.w(TAG, "openDevice returned null")
            return
        }
        val serialPort = driver.ports.firstOrNull()
        if (serialPort == null) {
            Log.w(TAG, "driver exposes no ports")
            return
        }
        try {
            serialPort.open(connection)
            serialPort.setParameters(BAUD, 8, UsbSerialPort.STOPBITS_1, UsbSerialPort.PARITY_NONE)
            runCatching {
                serialPort.setRTS(false)
                serialPort.setDTR(false)
            }.onFailure { Log.w(TAG, "control lines: $it") }
        } catch (e: Exception) {
            Log.w(TAG, "open failed: $e")
            runCatching { serialPort.close() }
            return
        }
        port = serialPort
        rxTotal = 0
        txTotal = 0
        NativeBridge.nativeUsbConnected(true)
        Log.i(TAG, "port open; nativeUsbConnected(true)")
        startPumps(serialPort)
    }

    private fun startPumps(serialPort: UsbSerialPort) {
        val io = SerialInputOutputManager(
            serialPort,
            object : SerialInputOutputManager.Listener {
                override fun onNewData(data: ByteArray) {
                    val now = System.currentTimeMillis()
                    if (now - lastRxLogMs > 700) {
                        lastRxLogMs = now
                        val head = data.take(28).joinToString(" ") { "%02x".format(it) }
                        Log.i(TAG, "RX ${data.size}B total=${rxTotal + data.size}: $head")
                    }
                    rxTotal += data.size
                    val n = minOf(data.size, rxBuffer.capacity())
                    rxBuffer.clear()
                    rxBuffer.put(data, 0, n)
                    NativeBridge.nativeUsbRx(rxBuffer, n)
                }

                override fun onRunError(e: Exception) {
                    Log.w(TAG, "io error: $e")
                    disconnect()
                }
            },
        )
        io.start()
        ioManager = io

        running = true
        Thread {
            val txBuffer = ByteBuffer.allocateDirect(TX_CAPACITY)
            val scratch = ByteArray(TX_CAPACITY)
            while (running) {
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
                    runCatching { port?.write(scratch.copyOf(n), WRITE_TIMEOUT_MS) }
                        .onFailure { Log.w(TAG, "write: $it") }
                } else {
                    Thread.sleep(IDLE_SLEEP_MS)
                }
            }
        }.start()
    }

    private fun disconnect() {
        if (port == null) return
        Log.i(TAG, "disconnect (rx=$rxTotal tx=$txTotal)")
        running = false
        NativeBridge.nativeUsbConnected(false)
        ioManager?.stop()
        ioManager = null
        runCatching { port?.close() }
        port = null
    }

    private fun requestPermission(device: UsbDevice) {
        val intent = Intent(ACTION_USB_PERMISSION).setPackage(context.packageName)
        val pending =
            PendingIntent.getBroadcast(context, 0, intent, PendingIntent.FLAG_MUTABLE)
        usbManager.requestPermission(device, pending)
    }

    private fun prober(): UsbSerialProber {
        val table = UsbSerialProber.getDefaultProbeTable()
        table.addProduct(ESP_VENDOR_ID, ESP_PRODUCT_ID, CdcAcmSerialDriver::class.java)
        return UsbSerialProber(table)
    }

    companion object {
        private const val TAG = "HopspotUsb"
        private const val ACTION_USB_PERMISSION = "org.personal.hopspot.USB_PERMISSION"
        private const val BAUD = 115200
        private const val RX_CAPACITY = 16 * 1024
        private const val TX_CAPACITY = 4 * 1024
        private const val WRITE_TIMEOUT_MS = 200
        private const val IDLE_SLEEP_MS = 2L
        private const val SCAN_INTERVAL_MS = 1000L
        private const val ESP_VENDOR_ID = 0x303A
        private const val ESP_PRODUCT_ID = 0x1001
    }
}
