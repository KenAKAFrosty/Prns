package org.personal.hopspot

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Binder
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Message
import android.os.Messenger
import android.os.RemoteException
import android.os.SystemClock
import android.util.Log
import java.nio.ByteBuffer
import java.util.concurrent.CopyOnWriteArraySet

class PrnsService : Service() {
    inner class LocalBinder : Binder() {
        val service: PrnsService
            get() = this@PrnsService
    }

    private val localBinder = LocalBinder()
    private val clientMessengers = CopyOnWriteArraySet<Messenger>()
    private val clientMessenger = Messenger(ClientHandler())

    private var renderHandle: Long = 0L
    private var usbLink: UsbLink? = null
    private var wifiAutoLink: WifiAutoLink? = null
    private var wifiDirectLink: WifiDirectLink? = null
    private var wifiAwareLink: WifiAwareLink? = null
    private var bleLink: BleLink? = null
    private var serviceStartedAtElapsedMs: Long = 0L
    private var lastServiceError: String? = null
    private var isForeground = false

    override fun onCreate() {
        super.onCreate()
        serviceStartedAtElapsedMs = SystemClock.elapsedRealtime()
        createNotificationChannel()
        if (!startForegroundNow()) {
            return
        }
        renderHandle = NativeBridge.nativeInit()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopSelf()
            return START_NOT_STICKY
        }
        if (!startForegroundNow()) {
            return START_NOT_STICKY
        }
        ensureEngineStarted()
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder {
        if (isForeground) {
            ensureEngineStarted()
        }
        return if (intent?.action == ACTION_CLIENT) {
            clientMessenger.binder
        } else {
            localBinder
        }
    }

    override fun onDestroy() {
        stopPlatformLinks()
        clientMessengers.clear()
        val stopFailure = NativeBridge.nativeStopEngine()
        if (stopFailure != 0) {
            lastServiceError = "engine_stop:$stopFailure"
            Log.e(TAG, "native engine shutdown failed with code $stopFailure")
        }
        if (renderHandle != 0L) {
            NativeBridge.nativeFree(renderHandle)
            renderHandle = 0L
        }
        stopForegroundCompat()
        super.onDestroy()
    }

    @Synchronized
    fun refreshPlatformLinks() {
        stopPlatformLinks()
        ensureEngineStarted()
    }

    @Synchronized
    fun postInput(code: Int): Int =
        if (renderHandle != 0L) {
            NativeBridge.nativePostInput(renderHandle, code)
        } else {
            NativeBridge.ACTION_NONE
        }

    @Synchronized
    fun render(buffer: ByteBuffer) {
        if (renderHandle != 0L) {
            NativeBridge.nativeRender(renderHandle, buffer)
        }
    }

    @Synchronized
    fun setBattery(percent: Int, externallyPowered: Boolean) {
        if (renderHandle != 0L) {
            NativeBridge.nativeSetBattery(renderHandle, percent, externallyPowered)
        }
    }

    fun announce() {
        NativeBridge.nativeAnnounce()
    }

    @Synchronized
    private fun ensureEngineStarted() {
        when (NativeBridge.engineState()) {
            PrnsEngineState.RUNNING -> {
                startPlatformLinks()
                refreshNotification()
                return
            }
            PrnsEngineState.STARTING -> return
            PrnsEngineState.STOPPED -> Unit
            PrnsEngineState.FAILED -> {
                val cleanupFailure = NativeBridge.nativeStopEngine()
                if (cleanupFailure != 0) {
                    lastServiceError = "engine_cleanup:$cleanupFailure"
                    refreshNotification()
                    return
                }
            }
        }
        val failure = NativeBridge.nativeStartEngine(filesDir.absolutePath)
        val state = NativeBridge.engineState()
        if (failure == 0 && state == PrnsEngineState.RUNNING) {
            lastServiceError = null
            startPlatformLinks()
        } else {
            val typedFailure = NativeBridge.engineFailure()
            lastServiceError = "engine_start:${typedFailure.name}:${typedFailure.code}"
            Log.e(TAG, "native engine startup failed: $lastServiceError")
        }
        refreshNotification()
    }

    @Synchronized
    private fun startPlatformLinks() {
        if (NativeBridge.engineState() != PrnsEngineState.RUNNING) {
            return
        }
        if (wifiAutoLink == null) {
            Log.i(TAG, "starting Wi-Fi Auto link")
            wifiAutoLink = try {
                WifiAutoLink(applicationContext).also { it.start() }
            } catch (error: RuntimeException) {
                Log.e(TAG, "Wi-Fi Auto link failed to start", error)
                null
            }
        }
        if (BuildConfig.EXPERIMENTAL_WIFI_DIRECT && wifiDirectLink == null) {
            Log.i(TAG, "starting Wi-Fi Direct link")
            wifiDirectLink = try {
                WifiDirectLink(applicationContext).also { it.start() }
            } catch (error: RuntimeException) {
                Log.e(TAG, "Wi-Fi Direct link failed to start", error)
                null
            }
        } else if (!BuildConfig.EXPERIMENTAL_WIFI_DIRECT) {
            NativeBridge.nativeWifiDirectAvailability(
                NativeBridge.WIFI_DIRECT_EXPERIMENTAL_DISABLED,
            )
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && wifiAwareLink == null) {
            Log.i(TAG, "starting Wi-Fi Aware link")
            wifiAwareLink = try {
                WifiAwareLink(applicationContext).also { it.start() }
            } catch (error: RuntimeException) {
                Log.e(TAG, "Wi-Fi Aware link failed to start", error)
                null
            }
        }
        if (usbLink == null) {
            Log.i(TAG, "starting USB Auto link")
            usbLink = try {
                UsbLink(applicationContext).also { it.start() }
            } catch (e: Exception) {
                Log.e(TAG, "USB Auto link failed to start", e)
                null
            }
        }
        if (bleLink == null) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                Log.i(TAG, "Bluetooth LE Auto requires Android 10 or newer")
            } else if (hasBlePermissions()) {
                Log.i(TAG, "starting Bluetooth LE Auto link")
                bleLink = try {
                    BleLink(applicationContext).also { it.start() }
                } catch (error: RuntimeException) {
                    Log.e(TAG, "Bluetooth LE Auto link failed to start", error)
                    null
                }
            } else {
                Log.i(
                    TAG,
                    "Bluetooth permissions not granted; Bluetooth LE link will start after permission refresh",
                )
            }
        }
    }

    @Synchronized
    private fun stopPlatformLinks() {
        runCatching { bleLink?.stop() }
            .onFailure { Log.w(TAG, "Bluetooth LE Auto link failed to stop", it) }
        bleLink = null
        runCatching { usbLink?.stop() }
            .onFailure { Log.w(TAG, "USB Auto link failed to stop", it) }
        usbLink = null
        runCatching { wifiAwareLink?.stop() }
            .onFailure { Log.w(TAG, "Wi-Fi Aware link failed to stop", it) }
        wifiAwareLink = null
        runCatching { wifiDirectLink?.stop() }
            .onFailure { Log.w(TAG, "Wi-Fi Direct link failed to stop", it) }
        wifiDirectLink = null
        runCatching { wifiAutoLink?.stop() }
            .onFailure { Log.w(TAG, "Wi-Fi Auto link failed to stop", it) }
        wifiAutoLink = null
    }

    private fun hasBlePermissions(): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
            return true
        }
        val permissions =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                listOf(
                    Manifest.permission.BLUETOOTH_SCAN,
                    Manifest.permission.BLUETOOTH_ADVERTISE,
                    Manifest.permission.BLUETOOTH_CONNECT,
                )
            } else {
                listOf(Manifest.permission.ACCESS_FINE_LOCATION)
            }
        return permissions.all { checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }
    }

    private fun startForegroundNow(): Boolean {
        val notification = buildNotification()
        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(
                    NOTIFICATION_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
                )
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
            lastServiceError = null
            isForeground = true
            true
        } catch (e: Exception) {
            lastServiceError = "foreground:${e.javaClass.simpleName}"
            isForeground = false
            Log.e(TAG, "failed to promote PrnsService to foreground", e)
            stopSelf()
            false
        }
    }

    private fun buildNotification(): Notification {
        val openIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            immutablePendingIntentFlags(),
        )
        val stopIntent = PendingIntent.getService(
            this,
            1,
            Intent(this, PrnsService::class.java).setAction(ACTION_STOP),
            immutablePendingIntentFlags(),
        )
        val builder =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Notification.Builder(this, NOTIFICATION_CHANNEL)
            } else {
                @Suppress("DEPRECATION")
                Notification.Builder(this)
            }
        @Suppress("DEPRECATION")
        return builder
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle("Personal RNS")
            .setContentText(notificationText())
            .setContentIntent(openIntent)
            .setOngoing(true)
            .setPriority(Notification.PRIORITY_LOW)
            .setShowWhen(false)
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Stop", stopIntent)
            .build()
    }

    private fun notificationText(): String =
        when (NativeBridge.engineState()) {
            PrnsEngineState.STOPPED -> "Local RNS node is stopped"
            PrnsEngineState.STARTING -> "Local RNS node is starting"
            PrnsEngineState.RUNNING -> "Local RNS node is running"
            PrnsEngineState.FAILED -> "Local RNS node failed to start"
        }

    private fun refreshNotification() {
        if (!isForeground) {
            return
        }
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        manager.notify(NOTIFICATION_ID, buildNotification())
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return
        }
        val manager = getSystemService(NotificationManager::class.java)
        val channel = NotificationChannel(
            NOTIFICATION_CHANNEL,
            "Personal RNS",
            NotificationManager.IMPORTANCE_LOW,
        )
        channel.description = "Keeps the local Personal RNS node available"
        manager.createNotificationChannel(channel)
    }

    @Suppress("DEPRECATION")
    private fun stopForegroundCompat() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            stopForeground(true)
        }
        isForeground = false
    }

    private fun immutablePendingIntentFlags(): Int =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        } else {
            PendingIntent.FLAG_UPDATE_CURRENT
        }

    private inner class ClientHandler : Handler(Looper.getMainLooper()) {
        override fun handleMessage(msg: Message) {
            when (msg.what) {
                MSG_REGISTER_CLIENT -> {
                    msg.replyTo?.let { clientMessengers.add(it) }
                    replyStatus(msg.replyTo)
                }
                MSG_UNREGISTER_CLIENT -> {
                    msg.replyTo?.let { clientMessengers.remove(it) }
                }
                MSG_ANNOUNCE -> announce()
                MSG_QUERY_STATUS -> replyStatus(msg.replyTo)
                else -> super.handleMessage(msg)
            }
        }
    }

    private fun replyStatus(replyTo: Messenger?) {
        if (replyTo == null) {
            return
        }
        val health = NativeBridge.runtimeHealth()
        val persistence = NativeBridge.persistenceHealth()
        val engineState = NativeBridge.engineState()
        val engineFailure = NativeBridge.engineFailure()
        val reply = Message.obtain(null, MSG_STATUS).apply {
            data = Bundle().apply {
                putString(KEY_STATE, engineState.wireName)
                putBoolean(KEY_RUNNING, engineState == PrnsEngineState.RUNNING)
                putBoolean(KEY_FOREGROUND, isForeground)
                putInt(KEY_LAST_FAILURE_CODE, engineFailure.code)
                putString(KEY_LAST_FAILURE, engineFailure.name)
                putString(KEY_INSTANCE_ROLE, INSTANCE_ROLE_SERVER)
                putInt(KEY_LOCAL_PORT, LOCAL_RNS_PORT)
                putInt(KEY_RPC_PORT, RPC_PORT)
                NativeBridge.nativeRpcKeyHex()?.let { putString(KEY_RPC_KEY_HEX, it) }
                NativeBridge.nativeNodeIdentityHashHex()?.let {
                    putString(KEY_NODE_IDENTITY_HASH_HEX, it)
                }
                NativeBridge.nativeBleIdentityHex()?.let {
                    putString(KEY_BLE_IDENTITY_HEX, it)
                }
                NativeBridge.nativeDeliveryDestinationHex()?.let {
                    putString(KEY_DELIVERY_DESTINATION_HEX, it)
                }
                NativeBridge.nativeNodePageDestinationHex()?.let {
                    putString(KEY_NODE_PAGE_DESTINATION_HEX, it)
                }
                putBoolean(KEY_BLE_LINK_STARTED, bleLink != null)
                putBoolean(KEY_WIFI_AWARE_LINK_STARTED, wifiAwareLink != null)
                putBoolean(KEY_WIFI_DIRECT_LINK_STARTED, wifiDirectLink != null)
                putString(
                    KEY_WIFI_AWARE_FAILURE,
                    NativeBridge.nativeWifiAwareFailureReason() ?: TRANSPORT_FAILURE_NONE,
                )
                putString(
                    KEY_WIFI_DIRECT_FAILURE,
                    NativeBridge.nativeWifiDirectFailureReason() ?: TRANSPORT_FAILURE_NONE,
                )
                putBoolean(KEY_PERSISTENCE_ACTIVE, persistence != null)
                putInt(
                    KEY_RESTORED_ROUTE_COUNT,
                    persistence?.restoredRouteCount ?: 0,
                )
                putInt(
                    KEY_RESTORED_DESTINATION_IDENTITY_COUNT,
                    persistence?.restoredDestinationIdentityCount ?: 0,
                )
                putInt(
                    KEY_RESTORED_TUNNEL_COUNT,
                    persistence?.restoredTunnelCount ?: 0,
                )
                putInt(
                    KEY_RESTORED_RATCHET_COUNT,
                    persistence?.restoredRatchetCount ?: 0,
                )
                putInt(
                    KEY_REFUSED_RESTORE_COUNT,
                    persistence?.refusedRestoreCount ?: 0,
                )
                putInt(
                    KEY_DROPPED_RESTORE_COUNT,
                    persistence?.droppedRestoreCount ?: 0,
                )
                putLong(
                    KEY_SUCCESSFUL_FLUSH_COUNT,
                    persistence?.successfulFlushCount ?: 0,
                )
                putLong(KEY_SERVICE_UPTIME_MS, serviceUptimeMs())
                putLong(KEY_RUNTIME_UPTIME_MS, health.runtimeUptimeMs)
                putInt(KEY_CLIENT_COUNT, clientMessengers.size)
                putInt(KEY_INTERFACE_COUNT, health.interfaceCount)
                putInt(KEY_ONLINE_INTERFACE_COUNT, health.onlineInterfaceCount)
                putInt(KEY_LOCAL_CLIENT_COUNT, health.localClientCount)
                putInt(KEY_ROUTE_COUNT, health.routeCount)
                putInt(KEY_LINK_COUNT, health.linkCount)
                putInt(KEY_TRANSPORTED_LINK_COUNT, health.transportedLinkCount)
                putLong(KEY_RX_BYTES, health.rxBytes)
                putLong(KEY_TX_BYTES, health.txBytes)
                putLong(KEY_RX_BPS, health.rxBps)
                putLong(KEY_TX_BPS, health.txBps)
                lastServiceError?.let { putString(KEY_LAST_ERROR, it) }
            }
        }
        try {
            replyTo.send(reply)
        } catch (e: RemoteException) {
            clientMessengers.remove(replyTo)
        }
    }

    private fun serviceUptimeMs(): Long =
        (SystemClock.elapsedRealtime() - serviceStartedAtElapsedMs).coerceAtLeast(0)

    companion object {
        const val ACTION_START = "org.personal.hopspot.action.START_PRNS"
        const val ACTION_STOP = "org.personal.hopspot.action.STOP_PRNS"
        const val ACTION_CLIENT = "org.personal.hopspot.action.BIND_PRNS_CLIENT"

        const val MSG_REGISTER_CLIENT = 1
        const val MSG_UNREGISTER_CLIENT = 2
        const val MSG_ANNOUNCE = 3
        const val MSG_QUERY_STATUS = 4
        const val MSG_STATUS = 5

        const val KEY_STATE = "state"
        const val KEY_RUNNING = "running"
        const val KEY_FOREGROUND = "foreground"
        const val KEY_INSTANCE_ROLE = "instance_role"
        const val KEY_LOCAL_PORT = "local_port"
        const val KEY_RPC_PORT = "rpc_port"
        const val KEY_RPC_KEY_HEX = "rpc_key_hex"
        const val KEY_NODE_IDENTITY_HASH_HEX = "node_identity_hash_hex"
        const val KEY_BLE_IDENTITY_HEX = "ble_identity_hex"
        const val KEY_DELIVERY_DESTINATION_HEX = "delivery_destination_hex"
        const val KEY_NODE_PAGE_DESTINATION_HEX = "node_page_destination_hex"
        const val KEY_BLE_LINK_STARTED = "ble_link_started"
        const val KEY_WIFI_AWARE_LINK_STARTED = "wifi_aware_link_started"
        const val KEY_WIFI_DIRECT_LINK_STARTED = "wifi_direct_link_started"
        const val KEY_WIFI_AWARE_FAILURE = "wifi_aware_failure"
        const val KEY_WIFI_DIRECT_FAILURE = "wifi_direct_failure"
        const val KEY_PERSISTENCE_ACTIVE = "persistence_active"
        const val KEY_RESTORED_ROUTE_COUNT = "restored_route_count"
        const val KEY_RESTORED_DESTINATION_IDENTITY_COUNT =
            "restored_destination_identity_count"
        const val KEY_RESTORED_TUNNEL_COUNT = "restored_tunnel_count"
        const val KEY_RESTORED_RATCHET_COUNT = "restored_ratchet_count"
        const val KEY_REFUSED_RESTORE_COUNT = "refused_restore_count"
        const val KEY_DROPPED_RESTORE_COUNT = "dropped_restore_count"
        const val KEY_SUCCESSFUL_FLUSH_COUNT = "successful_flush_count"
        const val KEY_SERVICE_UPTIME_MS = "service_uptime_ms"
        const val KEY_RUNTIME_UPTIME_MS = "runtime_uptime_ms"
        const val KEY_CLIENT_COUNT = "client_count"
        const val KEY_INTERFACE_COUNT = "interface_count"
        const val KEY_ONLINE_INTERFACE_COUNT = "online_interface_count"
        const val KEY_LOCAL_CLIENT_COUNT = "local_client_count"
        const val KEY_ROUTE_COUNT = "route_count"
        const val KEY_LINK_COUNT = "link_count"
        const val KEY_TRANSPORTED_LINK_COUNT = "transported_link_count"
        const val KEY_RX_BYTES = "rx_bytes"
        const val KEY_TX_BYTES = "tx_bytes"
        const val KEY_RX_BPS = "rx_bps"
        const val KEY_TX_BPS = "tx_bps"
        const val KEY_LAST_FAILURE_CODE = "last_failure_code"
        const val KEY_LAST_FAILURE = "last_failure"
        const val KEY_LAST_ERROR = "last_error"
        const val INSTANCE_ROLE_SERVER = "server"
        const val TRANSPORT_FAILURE_NONE = "none"

        private const val TAG = "PrnsService"
        private const val NOTIFICATION_ID = 42
        private const val NOTIFICATION_CHANNEL = "personal_rns_node"
        private const val LOCAL_RNS_PORT = 37428
        private const val RPC_PORT = LOCAL_RNS_PORT + 1

        fun start(context: Context) {
            val intent = Intent(context, PrnsService::class.java).setAction(ACTION_START)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }
    }
}
