package org.personal.hopspot

import android.Manifest
import android.annotation.SuppressLint
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.wifi.p2p.WifiP2pConfig
import android.net.wifi.p2p.WifiP2pDevice
import android.net.wifi.p2p.WifiP2pInfo
import android.net.wifi.p2p.WifiP2pManager
import android.net.wifi.p2p.nsd.WifiP2pDnsSdServiceInfo
import android.net.wifi.p2p.nsd.WifiP2pDnsSdServiceRequest
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import java.nio.ByteBuffer

@SuppressLint("MissingPermission")
class WifiDirectLink(context: Context) {
    private val appContext = context.applicationContext
    private val manager =
        appContext.getSystemService(Context.WIFI_P2P_SERVICE) as? WifiP2pManager
    private val handler = Handler(Looper.getMainLooper())

    private var channel: WifiP2pManager.Channel? = null
    private var receiver: BroadcastReceiver? = null
    private var p2pEnabled = false
    private var discoveryActive = false
    private var discoverPending = false
    private var running = false
    private var channelGeneration = 0

    private val serviceType = NativeBridge.nativeWifiDirectServiceType()
    private val legacyInstanceName = NativeBridge.nativeWifiDirectDeviceMarker()
    private val nativeInstanceName = NativeBridge.nativeWifiDirectNativeServiceInstance()
    private val supplicantInstanceName =
        NativeBridge.nativeWifiDirectSupplicantServiceInstance()
    private var forming = false
    private var inGroup = false
    private var formationDeadlineElapsedMs = 0L

    fun start() {
        if (running) {
            return
        }
        running = true
        val manager = manager
        if (manager == null) {
            Log.i(TAG, "Wi-Fi P2P service unavailable on this device")
            NativeBridge.nativeWifiDirectAvailability(NativeBridge.WIFI_DIRECT_DISABLED)
            return
        }
        registerReceiver()
        openChannel(manager)
        handler.post(pollLoop)
    }

    fun stop() {
        val wasForming = forming
        val wasInGroup = inGroup
        running = false
        channelGeneration += 1
        handler.removeCallbacksAndMessages(null)
        receiver?.let { runCatching { appContext.unregisterReceiver(it) } }
        receiver = null
        val manager = manager
        val activeChannel = channel
        channel = null
        if (manager != null && activeChannel != null) {
            manager.clearLocalServices(activeChannel, null)
            manager.clearServiceRequests(activeChannel, null)
            manager.stopPeerDiscovery(activeChannel, null)
            manager.cancelConnect(activeChannel, null)
            manager.removeGroup(activeChannel, null)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
                activeChannel.close()
            }
        }
        channel = null
        discoveryActive = false
        discoverPending = false
        forming = false
        inGroup = false
        if (wasForming) {
            NativeBridge.nativeWifiDirectFormationFailed()
        }
        if (wasInGroup) {
            NativeBridge.nativeWifiDirectGroupLost()
        }
        NativeBridge.nativeWifiDirectAvailability(NativeBridge.WIFI_DIRECT_DISABLED)
    }

    private fun openChannel(manager: WifiP2pManager) {
        channelGeneration += 1
        val generation = channelGeneration
        val opened = manager.initialize(appContext, Looper.getMainLooper()) {
            onChannelDisconnected(manager, generation)
        }
        channel = opened
        reportAvailability()
        advertiseService(manager, opened)
        setupServiceDiscovery(manager, opened)
    }

    private fun onChannelDisconnected(manager: WifiP2pManager, generation: Int) {
        if (!running || generation != channelGeneration) {
            return
        }
        Log.w(TAG, "Wi-Fi P2P channel disconnected; reopening")
        channel = null
        discoveryActive = false
        discoverPending = false
        forming = false
        if (inGroup) {
            inGroup = false
            NativeBridge.nativeWifiDirectGroupLost()
        }
        NativeBridge.nativeWifiDirectAvailability(NativeBridge.WIFI_DIRECT_DISABLED)
        handler.postDelayed(
            {
                if (running && generation == channelGeneration && channel == null) {
                    openChannel(manager)
                }
            },
            CHANNEL_REOPEN_DELAY_MS,
        )
    }

    private fun hasPermission(): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
            return true
        }
        val needed =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                Manifest.permission.NEARBY_WIFI_DEVICES
            } else {
                Manifest.permission.ACCESS_FINE_LOCATION
            }
        return appContext.checkSelfPermission(needed) == PackageManager.PERMISSION_GRANTED
    }

    private fun reportAvailability() {
        val code =
            when {
                !hasPermission() -> NativeBridge.WIFI_DIRECT_NO_PERMISSION
                !p2pEnabled -> NativeBridge.WIFI_DIRECT_DISABLED
                else -> NativeBridge.WIFI_DIRECT_AVAILABLE
            }
        NativeBridge.nativeWifiDirectAvailability(code)
    }

    private fun registerReceiver() {
        val filter = IntentFilter().apply {
            addAction(WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_THIS_DEVICE_CHANGED_ACTION)
            addAction(WifiP2pManager.WIFI_P2P_DISCOVERY_CHANGED_ACTION)
        }
        val listener = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                when (intent.action) {
                    WifiP2pManager.WIFI_P2P_STATE_CHANGED_ACTION -> {
                        val state =
                            intent.getIntExtra(WifiP2pManager.EXTRA_WIFI_STATE, -1)
                        p2pEnabled = state == WifiP2pManager.WIFI_P2P_STATE_ENABLED
                        reportAvailability()
                    }
                    WifiP2pManager.WIFI_P2P_CONNECTION_CHANGED_ACTION ->
                        onConnectionChanged()
                    WifiP2pManager.WIFI_P2P_DISCOVERY_CHANGED_ACTION -> {
                        val state =
                            intent.getIntExtra(
                                WifiP2pManager.EXTRA_DISCOVERY_STATE,
                                WifiP2pManager.WIFI_P2P_DISCOVERY_STOPPED,
                            )
                        discoveryActive = state == WifiP2pManager.WIFI_P2P_DISCOVERY_STARTED
                    }
                    WifiP2pManager.WIFI_P2P_THIS_DEVICE_CHANGED_ACTION -> {
                        val device =
                            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                                intent.getParcelableExtra(
                                    WifiP2pManager.EXTRA_WIFI_P2P_DEVICE,
                                    WifiP2pDevice::class.java,
                                )
                            } else {
                                @Suppress("DEPRECATION")
                                intent.getParcelableExtra(WifiP2pManager.EXTRA_WIFI_P2P_DEVICE)
                            }
                        device?.deviceName?.let {
                            NativeBridge.nativeWifiDirectSetLocalNameHash(it.hashCode())
                        }
                    }
                }
            }
        }
        receiver = listener
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            appContext.registerReceiver(listener, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            appContext.registerReceiver(listener, filter)
        }
    }

    private fun onConnectionChanged() {
        val manager = manager ?: return
        val channel = channel ?: return
        if (!hasPermission()) {
            return
        }
        manager.requestConnectionInfo(channel) { info: WifiP2pInfo ->
            if (info.groupFormed) {
                inGroup = true
                forming = false
                val owner = info.groupOwnerAddress?.address ?: return@requestConnectionInfo
                if (owner.size == 4) {
                    val buffer = ByteBuffer.allocateDirect(4)
                    buffer.put(owner)
                    NativeBridge.nativeWifiDirectGroupFormed(info.isGroupOwner, buffer)
                }
            } else if (inGroup) {
                inGroup = false
                forming = false
                NativeBridge.nativeWifiDirectGroupLost()
            }
        }
    }

    private fun advertiseService(manager: WifiP2pManager, channel: WifiP2pManager.Channel) {
        if (!hasPermission()) {
            return
        }
        val record = mapOf("role" to "prns")
        val info = WifiP2pDnsSdServiceInfo.newInstance(nativeInstanceName, serviceType, record)
        manager.addLocalService(channel, info, actionListener("addLocalService"))
    }

    private fun setupServiceDiscovery(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
    ) {
        manager.setDnsSdResponseListeners(
            channel,
            { instance, registrationType, device ->
                if (registrationType.startsWith(serviceType)) {
                    when {
                        instance == supplicantInstanceName -> pushSighting(device, true)
                        instance == nativeInstanceName -> pushSighting(device, false)
                        instance == legacyInstanceName ->
                            pushSighting(
                                device,
                                device.deviceName?.startsWith(legacyInstanceName) == true,
                            )
                    }
                }
            },
            null,
        )
        val request = WifiP2pDnsSdServiceRequest.newInstance(serviceType)
        manager.addServiceRequest(channel, request, actionListener("addServiceRequest"))
    }

    private fun pushSighting(device: WifiP2pDevice, peerIsSupplicant: Boolean) {
        val octets = macOctets(device.deviceAddress) ?: return
        val buffer = ByteBuffer.allocateDirect(6)
        buffer.put(octets)
        NativeBridge.nativeWifiDirectSighting(
            buffer,
            peerIsSupplicant,
            (device.deviceName ?: "").hashCode(),
        )
    }

    private val pollLoop = object : Runnable {
        override fun run() {
            pumpDesiredState()
            handler.postDelayed(this, POLL_INTERVAL_MS)
        }
    }

    private fun pumpDesiredState() {
        val manager = manager ?: return
        val channel = channel ?: return
        if (!hasPermission() || !p2pEnabled) {
            return
        }
        if (forming && SystemClock.elapsedRealtime() > formationDeadlineElapsedMs) {
            forming = false
            manager.cancelConnect(channel, null)
            NativeBridge.nativeWifiDirectFormationFailed()
        }
        NativeBridge.nativeWifiDirectTakeFormationRequest()
            ?.let(::decodeFormationRequest)
            ?.let(::formGroup)

        val wantDiscovery = NativeBridge.nativeWifiDirectDesiredDiscovery()
        if (wantDiscovery) {
            if (!forming && !discoveryActive && !discoverPending) {
                discoverPending = true
                manager.discoverServices(channel, object : WifiP2pManager.ActionListener {
                    override fun onSuccess() {
                        discoverPending = false
                    }

                    override fun onFailure(reason: Int) {
                        discoverPending = false
                        Log.w(TAG, "Wi-Fi Direct service discovery failed reason=$reason")
                    }
                })
            }
        } else if (!forming && (discoveryActive || discoverPending)) {
            discoverPending = false
            manager.stopPeerDiscovery(channel, actionListener("stopPeerDiscovery"))
        }

        if (NativeBridge.nativeWifiDirectTakeRemoveGroup()) {
            forming = false
            manager.cancelConnect(channel, null)
            manager.removeGroup(channel, actionListener("removeGroup"))
        }
    }

    private fun actionListener(op: String): WifiP2pManager.ActionListener =
        object : WifiP2pManager.ActionListener {
            override fun onSuccess() {}

            override fun onFailure(reason: Int) {
                Log.w(TAG, "Wi-Fi Direct $op failed reason=$reason")
            }
        }

    private fun formGroup(request: FormationRequest) {
        val manager = manager ?: return
        val channel = channel ?: return
        if (forming || inGroup || !hasPermission() || !p2pEnabled) {
            return
        }
        forming = true
        formationDeadlineElapsedMs = SystemClock.elapsedRealtime() + FORMATION_TIMEOUT_MS
        val config = WifiP2pConfig().apply {
            deviceAddress = request.peer
            groupOwnerIntent = request.intent
        }
        manager.connect(channel, config, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.i(TAG, "Wi-Fi Direct group formation started")
            }

            override fun onFailure(reason: Int) {
                forming = false
                NativeBridge.nativeWifiDirectFormationFailed()
                Log.w(TAG, "Wi-Fi Direct connect failed reason=$reason")
            }
        })
    }

    private fun decodeFormationRequest(encoded: ByteArray): FormationRequest? {
        if (encoded.size != FORMATION_REQUEST_BYTES) {
            return null
        }
        val intent = encoded[6].toInt() and 0xff
        if (intent !in 0..15) {
            return null
        }
        val peer = encoded.take(6).joinToString(":") { "%02x".format(it.toInt() and 0xff) }
        return FormationRequest(peer, intent)
    }

    private data class FormationRequest(val peer: String, val intent: Int)

    private companion object {
        private const val TAG = "HopspotWifiDirect"
        private const val POLL_INTERVAL_MS = 1000L
        private const val FORMATION_TIMEOUT_MS = 30_000L
        private const val CHANNEL_REOPEN_DELAY_MS = 1_000L
        private const val FORMATION_REQUEST_BYTES = 7

        private fun macOctets(address: String?): ByteArray? {
            val parts = address?.split(":") ?: return null
            if (parts.size != 6) {
                return null
            }
            val octets = ByteArray(6)
            for (i in 0 until 6) {
                octets[i] = parts[i].toIntOrNull(16)?.toByte() ?: return null
            }
            return octets
        }
    }
}
