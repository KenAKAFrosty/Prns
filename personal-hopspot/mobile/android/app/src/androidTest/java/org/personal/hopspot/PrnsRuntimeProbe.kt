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
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

class PrnsRuntimeProbe : Instrumentation() {
    private var requireRestoredRoute = false

    override fun onCreate(arguments: Bundle?) {
        requireRestoredRoute = arguments?.getString(ARG_REQUIRE_RESTORED_ROUTE) == "true"
        super.onCreate(arguments)
        start()
    }

    override fun onStart() {
        super.onStart()
        val results = Bundle()
        try {
            val outcome = runProbe()
            val identities = outcome.identities
            val platform = outcome.platform
            results.putString(RESULT_RPC_IDENTITY_SHA256, identities.rpcIdentitySha256)
            results.putString(RESULT_NODE_IDENTITY_SHA256, identities.nodeIdentitySha256)
            results.putString(
                RESULT_BLUETOOTH_IDENTITY_SHA256,
                identities.bluetoothIdentitySha256,
            )
            results.putString(
                RESULT_DELIVERY_DESTINATION_SHA256,
                identities.deliveryDestinationSha256,
            )
            results.putString(
                RESULT_NODE_PAGE_DESTINATION_SHA256,
                identities.nodePageDestinationSha256,
            )
            results.putString(RESULT_BLE_LINK_STARTED, platform.bleLinkStarted.toString())
            results.putString(
                RESULT_WIFI_AWARE_LINK_STARTED,
                platform.wifiAwareLinkStarted.toString(),
            )
            results.putString(
                RESULT_WIFI_DIRECT_LINK_STARTED,
                platform.wifiDirectLinkStarted.toString(),
            )
            results.putString(RESULT_WIFI_AWARE_FAILURE, platform.wifiAwareFailure)
            results.putString(RESULT_WIFI_DIRECT_FAILURE, platform.wifiDirectFailure)
            results.putString(
                RESULT_ROUTE_COUNT_BEFORE_RESTART,
                outcome.routeCountBeforeRestart.toString(),
            )
            results.putString(
                RESULT_ROUTE_COUNT_AFTER_RESTART,
                outcome.routeCountAfterRestart.toString(),
            )
            results.putString(
                RESULT_RESTORED_ROUTE_COUNT,
                outcome.persistence.restoredRouteCount.toString(),
            )
            results.putString(
                RESULT_RESTORED_DESTINATION_IDENTITY_COUNT,
                outcome.persistence.restoredDestinationIdentityCount.toString(),
            )
            results.putString(
                RESULT_RESTORED_TUNNEL_COUNT,
                outcome.persistence.restoredTunnelCount.toString(),
            )
            results.putString(
                RESULT_RESTORED_RATCHET_COUNT,
                outcome.persistence.restoredRatchetCount.toString(),
            )
            results.putString(
                RESULT_REFUSED_RESTORE_COUNT,
                outcome.persistence.refusedRestoreCount.toString(),
            )
            results.putString(
                RESULT_DROPPED_RESTORE_COUNT,
                outcome.persistence.droppedRestoreCount.toString(),
            )
            results.putString(
                RESULT_SUCCESSFUL_FLUSH_COUNT,
                outcome.persistence.successfulFlushCount.toString(),
            )
            results.putString("status", "ok")
            finish(Activity.RESULT_OK, results)
        } catch (error: Throwable) {
            results.putString("status", "fail")
            results.putString("error", error.stackTraceToString())
            finish(Activity.RESULT_CANCELED, results)
        }
    }

    private fun runProbe(): ProbeOutcome {
        val client = context
        val service = ComponentName(TARGET_PACKAGE, "$TARGET_PACKAGE.PrnsService")
        try {
            startService(client, service)
            Thread.sleep(FOREGROUND_SETTLE_MS)
            sendHome()
            Thread.sleep(BACKGROUND_SETTLE_MS)

            val first = bindAndQuery(client, service)
            requireStatusBundle(first, "first bind")

            client.unbindAndRebind(service).also { second ->
                requireStatusBundle(second, "background rebind")
            }

            val services = activeServiceDump()
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

            stopService(client, service)
            startService(client, service)
            Thread.sleep(FOREGROUND_SETTLE_MS)
            val restarted = bindAndQuery(client, service)
            requireStatusBundle(restarted, "restart bind")
            val firstEvidence = EvidenceSnapshot.from(first)
            val restartedEvidence = EvidenceSnapshot.from(restarted)
            require(restartedEvidence == firstEvidence) {
                "identity changed across service restart"
            }
            val firstPlatform = PlatformSnapshot.from(first)
            val restartedPlatform = PlatformSnapshot.from(restarted)
            require(restartedPlatform == firstPlatform) {
                "platform state changed across service restart"
            }
            val persistence = PersistenceSnapshot.from(restarted)
            require(persistence.restoredRatchetCount >= 1) {
                "ratcheted destination state did not restore"
            }
            require(persistence.refusedRestoreCount == 0) {
                "runtime state restore refused ${persistence.refusedRestoreCount} rows"
            }
            val firstRouteCount = first.getInt(KEY_ROUTE_COUNT)
            val restartedRouteCount = restarted.getInt(KEY_ROUTE_COUNT)
            if (requireRestoredRoute) {
                require(firstRouteCount > 0) {
                    "routing persistence evidence requires a learned route before restart"
                }
                require(persistence.restoredRouteCount > 0) {
                    "no routing-table row restored after restart"
                }
            }
            return ProbeOutcome(
                firstEvidence,
                firstPlatform,
                firstRouteCount,
                restartedRouteCount,
                persistence,
            )
        } finally {
            client.stopService(Intent().setComponent(service))
        }
    }

    private fun startService(context: Context, service: ComponentName) {
        val intent = Intent(ACTION_START).setComponent(service)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.startForegroundService(intent)
        } else {
            context.startService(intent)
        }
    }

    private fun stopService(context: Context, service: ComponentName) {
        val intent = Intent(ACTION_STOP).setComponent(service)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.startForegroundService(intent)
        } else {
            context.startService(intent)
        }
        val stopped = (1..STOP_ATTEMPTS).any {
            if (!activeServiceDump().contains("PrnsService")) {
                true
            } else {
                Thread.sleep(STOP_POLL_MS)
                false
            }
        }
        require(stopped) { "PrnsService did not stop" }
    }

    private fun activeServiceDump(): String =
        shellOutput("dumpsys activity services $TARGET_PACKAGE")
            .substringAfter("active services:", "")

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
        require(status.getBoolean(KEY_PERSISTENCE_ACTIVE)) {
            "$label runtime persistence was not active"
        }
        require(status.getLong(KEY_SUCCESSFUL_FLUSH_COUNT) >= 1) {
            "$label runtime state had not landed an initial flush"
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
        for (key in IDENTITY_HEX_KEYS) {
            val value = status.getString(key).orEmpty()
            require(value.length == 32 && value.all { it in '0'..'9' || it in 'a'..'f' }) {
                "$label malformed $key"
            }
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

    private data class ProbeOutcome(
        val identities: EvidenceSnapshot,
        val platform: PlatformSnapshot,
        val routeCountBeforeRestart: Int,
        val routeCountAfterRestart: Int,
        val persistence: PersistenceSnapshot,
    )

    private data class PersistenceSnapshot(
        val restoredRouteCount: Int,
        val restoredDestinationIdentityCount: Int,
        val restoredTunnelCount: Int,
        val restoredRatchetCount: Int,
        val refusedRestoreCount: Int,
        val droppedRestoreCount: Int,
        val successfulFlushCount: Long,
    ) {
        companion object {
            fun from(status: Bundle): PersistenceSnapshot =
                PersistenceSnapshot(
                    restoredRouteCount = status.getInt(KEY_RESTORED_ROUTE_COUNT),
                    restoredDestinationIdentityCount =
                        status.getInt(KEY_RESTORED_DESTINATION_IDENTITY_COUNT),
                    restoredTunnelCount = status.getInt(KEY_RESTORED_TUNNEL_COUNT),
                    restoredRatchetCount = status.getInt(KEY_RESTORED_RATCHET_COUNT),
                    refusedRestoreCount = status.getInt(KEY_REFUSED_RESTORE_COUNT),
                    droppedRestoreCount = status.getInt(KEY_DROPPED_RESTORE_COUNT),
                    successfulFlushCount = status.getLong(KEY_SUCCESSFUL_FLUSH_COUNT),
                )
        }
    }

    private data class PlatformSnapshot(
        val bleLinkStarted: Boolean,
        val wifiAwareLinkStarted: Boolean,
        val wifiDirectLinkStarted: Boolean,
        val wifiAwareFailure: String,
        val wifiDirectFailure: String,
    ) {
        companion object {
            fun from(status: Bundle): PlatformSnapshot =
                PlatformSnapshot(
                    bleLinkStarted = status.getBoolean(KEY_BLE_LINK_STARTED),
                    wifiAwareLinkStarted = status.getBoolean(KEY_WIFI_AWARE_LINK_STARTED),
                    wifiDirectLinkStarted = status.getBoolean(KEY_WIFI_DIRECT_LINK_STARTED),
                    wifiAwareFailure = status.getString(KEY_WIFI_AWARE_FAILURE).orEmpty(),
                    wifiDirectFailure = status.getString(KEY_WIFI_DIRECT_FAILURE).orEmpty(),
                )
        }
    }

    private data class EvidenceSnapshot(
        val rpcIdentitySha256: String,
        val nodeIdentitySha256: String,
        val bluetoothIdentitySha256: String,
        val deliveryDestinationSha256: String,
        val nodePageDestinationSha256: String,
    ) {
        companion object {
            fun from(status: Bundle): EvidenceSnapshot =
                EvidenceSnapshot(
                    rpcIdentitySha256 =
                        fingerprint(KEY_RPC_KEY_HEX, status.getString(KEY_RPC_KEY_HEX).orEmpty()),
                    nodeIdentitySha256 =
                        fingerprint(
                            KEY_NODE_IDENTITY_HASH_HEX,
                            status.getString(KEY_NODE_IDENTITY_HASH_HEX).orEmpty(),
                        ),
                    bluetoothIdentitySha256 =
                        fingerprint(
                            KEY_BLE_IDENTITY_HEX,
                            status.getString(KEY_BLE_IDENTITY_HEX).orEmpty(),
                        ),
                    deliveryDestinationSha256 =
                        fingerprint(
                            KEY_DELIVERY_DESTINATION_HEX,
                            status.getString(KEY_DELIVERY_DESTINATION_HEX).orEmpty(),
                        ),
                    nodePageDestinationSha256 =
                        fingerprint(
                            KEY_NODE_PAGE_DESTINATION_HEX,
                            status.getString(KEY_NODE_PAGE_DESTINATION_HEX).orEmpty(),
                        ),
                )

            private fun fingerprint(domain: String, value: String): String {
                val digest = MessageDigest.getInstance("SHA-256")
                digest.update(domain.toByteArray(Charsets.UTF_8))
                digest.update(FINGERPRINT_DOMAIN_SEPARATOR)
                return digest.digest(value.toByteArray(Charsets.UTF_8)).toHex()
            }

            private fun ByteArray.toHex(): String {
                val out = CharArray(size * 2)
                forEachIndexed { index, byte ->
                    val value = byte.toInt() and 0xff
                    out[index * 2] = HEX_ALPHABET[value ushr 4]
                    out[index * 2 + 1] = HEX_ALPHABET[value and 0x0f]
                }
                return String(out)
            }
        }
    }

    private companion object {
        const val TARGET_PACKAGE = "org.personal.hopspot"
        const val ACTION_START = "org.personal.hopspot.action.START_PRNS"
        const val ACTION_STOP = "org.personal.hopspot.action.STOP_PRNS"
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
        const val STATE_RUNNING = "running"
        const val INSTANCE_ROLE_SERVER = "server"
        const val RESULT_RPC_IDENTITY_SHA256 = "rpc_identity_sha256"
        const val RESULT_NODE_IDENTITY_SHA256 = "node_identity_sha256"
        const val RESULT_BLUETOOTH_IDENTITY_SHA256 = "bluetooth_identity_sha256"
        const val RESULT_DELIVERY_DESTINATION_SHA256 = "delivery_destination_sha256"
        const val RESULT_NODE_PAGE_DESTINATION_SHA256 = "node_page_destination_sha256"
        const val RESULT_BLE_LINK_STARTED = "ble_link_started"
        const val RESULT_WIFI_AWARE_LINK_STARTED = "wifi_aware_link_started"
        const val RESULT_WIFI_DIRECT_LINK_STARTED = "wifi_direct_link_started"
        const val RESULT_WIFI_AWARE_FAILURE = "wifi_aware_failure"
        const val RESULT_WIFI_DIRECT_FAILURE = "wifi_direct_failure"
        const val RESULT_ROUTE_COUNT_BEFORE_RESTART = "route_count_before_restart"
        const val RESULT_ROUTE_COUNT_AFTER_RESTART = "route_count_after_restart"
        const val RESULT_RESTORED_ROUTE_COUNT = "restored_route_count"
        const val RESULT_RESTORED_DESTINATION_IDENTITY_COUNT =
            "restored_destination_identity_count"
        const val RESULT_RESTORED_TUNNEL_COUNT = "restored_tunnel_count"
        const val RESULT_RESTORED_RATCHET_COUNT = "restored_ratchet_count"
        const val RESULT_REFUSED_RESTORE_COUNT = "refused_restore_count"
        const val RESULT_DROPPED_RESTORE_COUNT = "dropped_restore_count"
        const val RESULT_SUCCESSFUL_FLUSH_COUNT = "successful_flush_count"
        const val ARG_REQUIRE_RESTORED_ROUTE = "require_restored_route"
        const val FINGERPRINT_DOMAIN_SEPARATOR: Byte = 0
        const val HEX_ALPHABET = "0123456789abcdef"
        const val LOCAL_RNS_PORT = 37428
        const val RPC_PORT = 37429
        const val FOREGROUND_SETTLE_MS = 1_500L
        const val BACKGROUND_SETTLE_MS = 1_500L
        const val STOP_ATTEMPTS = 50
        const val STOP_POLL_MS = 100L
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
            KEY_NODE_IDENTITY_HASH_HEX,
            KEY_BLE_IDENTITY_HEX,
            KEY_DELIVERY_DESTINATION_HEX,
            KEY_NODE_PAGE_DESTINATION_HEX,
            KEY_BLE_LINK_STARTED,
            KEY_WIFI_AWARE_LINK_STARTED,
            KEY_WIFI_DIRECT_LINK_STARTED,
            KEY_WIFI_AWARE_FAILURE,
            KEY_WIFI_DIRECT_FAILURE,
            KEY_PERSISTENCE_ACTIVE,
            KEY_RESTORED_ROUTE_COUNT,
            KEY_RESTORED_DESTINATION_IDENTITY_COUNT,
            KEY_RESTORED_TUNNEL_COUNT,
            KEY_RESTORED_RATCHET_COUNT,
            KEY_REFUSED_RESTORE_COUNT,
            KEY_DROPPED_RESTORE_COUNT,
            KEY_SUCCESSFUL_FLUSH_COUNT,
            KEY_SERVICE_UPTIME_MS,
            KEY_RUNTIME_UPTIME_MS,
            KEY_CLIENT_COUNT,
            KEY_INTERFACE_COUNT,
            KEY_ONLINE_INTERFACE_COUNT,
            KEY_LOCAL_CLIENT_COUNT,
            KEY_ROUTE_COUNT,
            KEY_LINK_COUNT,
            KEY_TRANSPORTED_LINK_COUNT,
            KEY_RESTORED_ROUTE_COUNT,
            KEY_RESTORED_DESTINATION_IDENTITY_COUNT,
            KEY_RESTORED_TUNNEL_COUNT,
            KEY_RESTORED_RATCHET_COUNT,
            KEY_REFUSED_RESTORE_COUNT,
            KEY_DROPPED_RESTORE_COUNT,
            KEY_RX_BYTES,
            KEY_TX_BYTES,
            KEY_RX_BPS,
            KEY_TX_BPS,
            KEY_SUCCESSFUL_FLUSH_COUNT,
        )
        val IDENTITY_HEX_KEYS = listOf(
            KEY_NODE_IDENTITY_HASH_HEX,
            KEY_BLE_IDENTITY_HEX,
            KEY_DELIVERY_DESTINATION_HEX,
            KEY_NODE_PAGE_DESTINATION_HEX,
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
