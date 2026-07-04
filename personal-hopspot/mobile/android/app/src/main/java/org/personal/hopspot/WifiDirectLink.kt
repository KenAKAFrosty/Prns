package org.personal.hopspot

import android.Manifest
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
import android.util.Log
import java.nio.ByteBuffer

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

    private val serviceType = NativeBridge.nativeWifiDirectServiceType()
    private val instanceName = NativeBridge.nativeWifiDirectDeviceMarker()
    private val groupSsidPrefix = NativeBridge.nativeWifiDirectGroupSsidPrefix()
    private val groupSsid = groupSsidPrefix + randomSuffix()
    private val groupPassphrase = NativeBridge.nativeWifiDirectGroupPassphrase()
    private var hosting = false
    private var joining = false

    fun start() {
        val manager = manager
        if (manager == null) {
            Log.i(TAG, "Wi-Fi P2P service unavailable on this device")
            NativeBridge.nativeWifiDirectAvailability(NativeBridge.WIFI_DIRECT_DISABLED)
            return
        }
        val channel = manager.initialize(appContext, Looper.getMainLooper()) {
            Log.w(TAG, "Wi-Fi P2P channel disconnected")
        }
        this.channel = channel
        reportAvailability()
        registerReceiver()
        advertiseService(manager, channel)
        setupServiceDiscovery(manager, channel)
        handler.post(pollLoop)
    }

    fun stop() {
        handler.removeCallbacksAndMessages(null)
        receiver?.let { runCatching { appContext.unregisterReceiver(it) } }
        receiver = null
        val manager = manager
        val activeChannel = channel
        if (manager != null && activeChannel != null) {
            manager.clearLocalServices(activeChannel, null)
            manager.clearServiceRequests(activeChannel, null)
            manager.stopPeerDiscovery(activeChannel, null)
            manager.removeGroup(activeChannel, null)
        }
        channel = null
        discoveryActive = false
        discoverPending = false
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
                if (info.isGroupOwner) {
                    advertiseGroupOffer(manager, channel)
                }
                val owner = info.groupOwnerAddress?.address ?: return@requestConnectionInfo
                if (owner.size == 4) {
                    val buffer = ByteBuffer.allocateDirect(4)
                    buffer.put(owner)
                    NativeBridge.nativeWifiDirectGroupFormed(info.isGroupOwner, buffer)
                }
            } else {
                hosting = false
                joining = false
                NativeBridge.nativeWifiDirectGroupLost()
            }
        }
    }

    private fun advertiseService(manager: WifiP2pManager, channel: WifiP2pManager.Channel) {
        if (!hasPermission()) {
            return
        }
        val record = mapOf("role" to "prns")
        val info = WifiP2pDnsSdServiceInfo.newInstance(instanceName, serviceType, record)
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
                        instance.startsWith(groupSsidPrefix) -> joinGroup(instance)
                        instance.startsWith(instanceName) -> pushSighting(device)
                    }
                }
            },
            null,
        )
        val request = WifiP2pDnsSdServiceRequest.newInstance(serviceType)
        manager.addServiceRequest(channel, request, actionListener("addServiceRequest"))
    }

    private fun pushSighting(device: WifiP2pDevice) {
        val octets = macOctets(device.deviceAddress) ?: return
        val buffer = ByteBuffer.allocateDirect(6)
        buffer.put(octets)
        NativeBridge.nativeWifiDirectSighting(buffer)
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
        val wantDiscovery = NativeBridge.nativeWifiDirectDesiredDiscovery()
        if (wantDiscovery) {
            if (!discoveryActive && !discoverPending) {
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
        } else if (discoveryActive || discoverPending) {
            discoverPending = false
            manager.stopPeerDiscovery(channel, actionListener("stopPeerDiscovery"))
        }

        if (NativeBridge.nativeWifiDirectTakeHostRequest()) {
            hostGroup()
        }

        if (NativeBridge.nativeWifiDirectTakeRemoveGroup()) {
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

    private fun hostGroup() {
        val manager = manager ?: return
        val channel = channel ?: return
        if (hosting || !hasPermission() || !p2pEnabled) {
            return
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return
        }
        hosting = true
        val config = WifiP2pConfig.Builder()
            .setNetworkName(groupSsid)
            .setPassphrase(groupPassphrase)
            .build()
        manager.createGroup(channel, config, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.i(TAG, "Wi-Fi Direct hosting group $groupSsid")
            }

            override fun onFailure(reason: Int) {
                hosting = false
                Log.w(TAG, "Wi-Fi Direct createGroup failed reason=$reason")
            }
        })
    }

    private fun joinGroup(ssid: String) {
        val manager = manager ?: return
        val channel = channel ?: return
        if (hosting || joining || !hasPermission() || !p2pEnabled) {
            return
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return
        }
        joining = true
        val config = WifiP2pConfig.Builder()
            .setNetworkName(ssid)
            .setPassphrase(groupPassphrase)
            .build()
        manager.connect(channel, config, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.i(TAG, "Wi-Fi Direct joining group $ssid")
            }

            override fun onFailure(reason: Int) {
                joining = false
                Log.w(TAG, "Wi-Fi Direct connect(join) failed reason=$reason")
            }
        })
    }

    private fun advertiseGroupOffer(manager: WifiP2pManager, channel: WifiP2pManager.Channel) {
        val record = mapOf("role" to "prns")
        val info = WifiP2pDnsSdServiceInfo.newInstance(groupSsid, serviceType, record)
        manager.clearLocalServices(channel, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                manager.addLocalService(channel, info, actionListener("advertiseGroupOffer"))
                Log.i(TAG, "Wi-Fi Direct advertising group offer $groupSsid")
            }

            override fun onFailure(reason: Int) {
                Log.w(TAG, "Wi-Fi Direct clearLocalServices failed reason=$reason")
            }
        })
    }

    private companion object {
        private const val TAG = "HopspotWifiDirect"
        private const val POLL_INTERVAL_MS = 1000L

        private fun randomSuffix(): String {
            val hex = "0123456789abcdef"
            return (1..6).map { hex.random() }.joinToString("")
        }

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
