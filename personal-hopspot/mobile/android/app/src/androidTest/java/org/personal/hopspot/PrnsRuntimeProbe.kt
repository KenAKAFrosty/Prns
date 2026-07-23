package org.personal.hopspot

import android.app.Activity
import android.app.Instrumentation
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Binder
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.Message
import android.os.Messenger
import java.io.FileInputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

class PrnsRuntimeProbe : Instrumentation() {
    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        start()
    }

    override fun onStart() {
        super.onStart()
        val results = Bundle()
        try {
            runProbe()
            results.putString("status", "ok")
            finish(Activity.RESULT_OK, results)
        } catch (error: Throwable) {
            results.putString("status", "fail")
            results.putString("error", error.stackTraceToString())
            finish(Activity.RESULT_CANCELED, results)
        }
    }

    private fun runProbe() {
        val client = context
        val service = ComponentName(TARGET_PACKAGE, "$TARGET_PACKAGE.PrnsService")
        val startIntent = Intent(ACTION_START).setComponent(service)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            client.startForegroundService(startIntent)
        } else {
            client.startService(startIntent)
        }

        Thread.sleep(FOREGROUND_SETTLE_MS)
        sendHome()
        Thread.sleep(BACKGROUND_SETTLE_MS)

        val first = bindAndQuery(client, service)
        requireStatusBundle(first, "first bind")

        client.unbindAndRebind(service).also { second ->
            requireStatusBundle(second, "background rebind")
        }

        val services = shellOutput("dumpsys activity services $TARGET_PACKAGE")
        require(services.contains("PrnsService")) {
            "activity service dump did not show PrnsService while probe was active"
        }

        val notifications = shellOutput("dumpsys notification --noredact")
        require(
            notifications.contains("Personal RNS") ||
                notifications.contains("personal_rns_node"),
        ) {
            "notification dump did not show the Personal RNS foreground notification"
        }
    }

    private fun requireStatusBundle(status: Bundle, label: String) {
        for (key in REQUIRED_STATUS_KEYS) {
            require(status.containsKey(key)) { "$label status missing $key" }
        }
        require(status.getString(KEY_STATE) == STATE_RUNNING) {
            "$label state=${status.getString(KEY_STATE)}"
        }
        require(status.getBoolean(KEY_RUNNING)) { "$label service did not report running" }
        require(status.getBoolean(KEY_FOREGROUND)) { "$label service did not report foreground" }
        require(status.getInt(KEY_LAST_FAILURE_CODE) == 0) {
            "$label failure code=${status.getInt(KEY_LAST_FAILURE_CODE)}"
        }
        require(status.getString(KEY_LAST_FAILURE) == "none") {
            "$label failure=${status.getString(KEY_LAST_FAILURE)}"
        }
        require(status.getString(KEY_INSTANCE_ROLE) == INSTANCE_ROLE_SERVER) {
            "$label instance role=${status.getString(KEY_INSTANCE_ROLE)}"
        }
        require(status.getInt(KEY_LOCAL_PORT) == LOCAL_RNS_PORT) {
            "$label local port ${status.getInt(KEY_LOCAL_PORT)}"
        }
        require(status.getInt(KEY_RPC_PORT) == RPC_PORT) {
            "$label rpc port ${status.getInt(KEY_RPC_PORT)}"
        }
        val rpcKeyHex = status.getString(KEY_RPC_KEY_HEX).orEmpty()
        require(rpcKeyHex.length == 64 && rpcKeyHex.all { it in '0'..'9' || it in 'a'..'f' }) {
            "$label malformed rpc key"
        }
        require(status.getLong(KEY_SERVICE_UPTIME_MS) > 0) {
            "$label service uptime ${status.getLong(KEY_SERVICE_UPTIME_MS)}"
        }
        require(status.getLong(KEY_RUNTIME_UPTIME_MS) >= 0) {
            "$label runtime uptime ${status.getLong(KEY_RUNTIME_UPTIME_MS)}"
        }
        require(status.getInt(KEY_CLIENT_COUNT) >= 1) {
            "$label client count ${status.getInt(KEY_CLIENT_COUNT)}"
        }
        require(status.getInt(KEY_INTERFACE_COUNT) >= 1) {
            "$label interface count ${status.getInt(KEY_INTERFACE_COUNT)}"
        }
        require(status.getInt(KEY_ONLINE_INTERFACE_COUNT) <= status.getInt(KEY_INTERFACE_COUNT)) {
            "$label online interfaces exceed total interfaces"
        }
        for (key in NON_NEGATIVE_INT_KEYS) {
            require(status.getInt(key) >= 0) { "$label $key=${status.getInt(key)}" }
        }
        for (key in NON_NEGATIVE_LONG_KEYS) {
            require(status.getLong(key) >= 0) { "$label $key=${status.getLong(key)}" }
        }
    }

    private fun Context.unbindAndRebind(service: ComponentName): Bundle {
        Thread.sleep(BACKGROUND_SETTLE_MS)
        return bindAndQuery(this, service)
    }

    private fun bindAndQuery(context: Context, service: ComponentName): Bundle {
        val handlerThread = HandlerThread("prns-runtime-probe-client").also { it.start() }
        val status = AtomicReference<Bundle>()
        val statusLatch = CountDownLatch(1)
        val replyMessenger = Messenger(
            Handler(handlerThread.looper) { message ->
                if (message.what == MSG_STATUS) {
                    status.set(Bundle(message.data))
                    statusLatch.countDown()
                    true
                } else {
                    false
                }
            },
        )

        val connected = CountDownLatch(1)
        val remote = AtomicReference<Messenger>()
        val connection = object : ServiceConnection {
            override fun onServiceConnected(name: ComponentName, binder: IBinder) {
                remote.set(Messenger(binder))
                connected.countDown()
            }

            override fun onServiceDisconnected(name: ComponentName) {
                remote.set(null)
            }
        }

        val intent = Intent(ACTION_CLIENT).setComponent(service)
        require(context.bindService(intent, connection, Context.BIND_AUTO_CREATE)) {
            "bindService returned false"
        }

        try {
            require(connected.await(5, TimeUnit.SECONDS)) { "timed out binding to PrnsService" }
            val client = remote.get() ?: error("PrnsService binder was null")
            val register = Message.obtain(null, MSG_REGISTER_CLIENT).apply {
                replyTo = replyMessenger
            }
            client.send(register)
            require(statusLatch.await(5, TimeUnit.SECONDS)) {
                "timed out waiting for PrnsService status"
            }
            return status.get() ?: error("PrnsService sent no status bundle")
        } finally {
            context.unbindService(connection)
            handlerThread.quitSafely()
        }
    }

    private fun sendHome() {
        runCatching {
            uiAutomation.executeShellCommand("input keyevent KEYCODE_HOME").use { descriptor ->
                Binder.flushPendingCommands()
            }
        }
    }

    private fun shellOutput(command: String): String {
        val descriptor = uiAutomation.executeShellCommand(command)
        return try {
            FileInputStream(descriptor.fileDescriptor).bufferedReader().use { it.readText() }
        } finally {
            descriptor.close()
        }
    }

    private companion object {
        const val TARGET_PACKAGE = "org.personal.hopspot"
        const val ACTION_START = "org.personal.hopspot.action.START_PRNS"
        const val ACTION_CLIENT = "org.personal.hopspot.action.BIND_PRNS_CLIENT"
        const val MSG_REGISTER_CLIENT = 1
        const val MSG_STATUS = 5
        const val KEY_STATE = "state"
        const val KEY_RUNNING = "running"
        const val KEY_FOREGROUND = "foreground"
        const val KEY_INSTANCE_ROLE = "instance_role"
        const val KEY_LOCAL_PORT = "local_port"
        const val KEY_RPC_PORT = "rpc_port"
        const val KEY_RPC_KEY_HEX = "rpc_key_hex"
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
        const val STATE_RUNNING = "running"
        const val INSTANCE_ROLE_SERVER = "server"
        const val LOCAL_RNS_PORT = 37428
        const val RPC_PORT = 37429
        const val FOREGROUND_SETTLE_MS = 1_500L
        const val BACKGROUND_SETTLE_MS = 1_500L
        val REQUIRED_STATUS_KEYS = listOf(
            KEY_STATE,
            KEY_RUNNING,
            KEY_FOREGROUND,
            KEY_LAST_FAILURE_CODE,
            KEY_LAST_FAILURE,
            KEY_INSTANCE_ROLE,
            KEY_LOCAL_PORT,
            KEY_RPC_PORT,
            KEY_RPC_KEY_HEX,
            KEY_SERVICE_UPTIME_MS,
            KEY_RUNTIME_UPTIME_MS,
            KEY_CLIENT_COUNT,
            KEY_INTERFACE_COUNT,
            KEY_ONLINE_INTERFACE_COUNT,
            KEY_LOCAL_CLIENT_COUNT,
            KEY_ROUTE_COUNT,
            KEY_LINK_COUNT,
            KEY_TRANSPORTED_LINK_COUNT,
            KEY_RX_BYTES,
            KEY_TX_BYTES,
            KEY_RX_BPS,
            KEY_TX_BPS,
        )
        val NON_NEGATIVE_INT_KEYS = listOf(
            KEY_LOCAL_CLIENT_COUNT,
            KEY_ROUTE_COUNT,
            KEY_LINK_COUNT,
            KEY_TRANSPORTED_LINK_COUNT,
        )
        val NON_NEGATIVE_LONG_KEYS = listOf(
            KEY_RUNTIME_UPTIME_MS,
            KEY_RX_BYTES,
            KEY_TX_BYTES,
            KEY_RX_BPS,
            KEY_TX_BPS,
        )
    }
}
