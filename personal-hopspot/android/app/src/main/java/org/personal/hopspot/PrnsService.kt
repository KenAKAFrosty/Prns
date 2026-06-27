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
    private var bleLink: BleLink? = null

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
        startForegroundNow()
        renderHandle = NativeBridge.nativeInit()
        startPlatformLinks()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopSelf()
            return START_NOT_STICKY
        }
        startForegroundNow()
        startPlatformLinks()
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder =
        if (intent?.action == ACTION_CLIENT) {
            clientMessenger.binder
        } else {
            localBinder
        }

    override fun onDestroy() {
        stopPlatformLinks()
        clientMessengers.clear()
        if (renderHandle != 0L) {
            NativeBridge.nativeFree(renderHandle)
            renderHandle = 0L
        }
        stopForegroundCompat()
        super.onDestroy()
    }

    @Synchronized
    fun refreshPlatformLinks() {
        startPlatformLinks()
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
    fun setBattery(percent: Int, charging: Boolean) {
        if (renderHandle != 0L) {
            NativeBridge.nativeSetBattery(renderHandle, percent, charging)
        }
    }

    fun announce() {
        NativeBridge.nativeAnnounce()
    }

    @Synchronized
    private fun startPlatformLinks() {
        if (wifiAutoLink == null) {
            wifiAutoLink = WifiAutoLink(applicationContext).also { it.start() }
        }
        if (usbLink == null) {
            usbLink = UsbLink(applicationContext).also { it.start() }
        }
        if (bleLink == null) {
            if (hasBlePermissions()) {
                bleLink = BleLink(applicationContext).also { it.start() }
            } else {
                Log.i(TAG, "BLE permissions not granted; BLE link will start after permission refresh")
            }
        }
    }

    @Synchronized
    private fun stopPlatformLinks() {
        bleLink?.stop()
        bleLink = null
        usbLink?.stop()
        usbLink = null
        wifiAutoLink?.stop()
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

    private fun startForegroundNow() {
        val notification = buildNotification()
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(
                    NOTIFICATION_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
                )
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
        } catch (e: Exception) {
            Log.e(TAG, "failed to promote PrnsService to foreground", e)
            stopSelf()
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
            .setContentText("Local RNS node is running")
            .setContentIntent(openIntent)
            .setOngoing(true)
            .setPriority(Notification.PRIORITY_LOW)
            .setShowWhen(false)
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Stop", stopIntent)
            .build()
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
        val reply = Message.obtain(null, MSG_STATUS).apply {
            data = Bundle().apply {
                putBoolean(KEY_RUNNING, true)
                putInt(KEY_LOCAL_PORT, LOCAL_RNS_PORT)
            }
        }
        try {
            replyTo.send(reply)
        } catch (e: RemoteException) {
            clientMessengers.remove(replyTo)
        }
    }

    companion object {
        const val ACTION_START = "org.personal.hopspot.action.START_PRNS"
        const val ACTION_STOP = "org.personal.hopspot.action.STOP_PRNS"
        const val ACTION_CLIENT = "org.personal.hopspot.action.BIND_PRNS_CLIENT"

        const val MSG_REGISTER_CLIENT = 1
        const val MSG_UNREGISTER_CLIENT = 2
        const val MSG_ANNOUNCE = 3
        const val MSG_QUERY_STATUS = 4
        const val MSG_STATUS = 5

        const val KEY_RUNNING = "running"
        const val KEY_LOCAL_PORT = "local_port"

        private const val TAG = "PrnsService"
        private const val NOTIFICATION_ID = 42
        private const val NOTIFICATION_CHANNEL = "personal_rns_node"
        private const val LOCAL_RNS_PORT = 37428

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
