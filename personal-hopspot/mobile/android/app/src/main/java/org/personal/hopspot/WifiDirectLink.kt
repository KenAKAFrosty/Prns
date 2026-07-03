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
    private var discovering = false

    private val serviceType = NativeBridge.nativeWifiDirectServiceType()
    private val instanceName = NativeBridge.nativeWifiDirectDeviceMarker()

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
        discovering = false
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
                val owner = info.groupOwnerAddress?.address ?: return@requestConnectionInfo
                if (owner.size == 4) {
                    val buffer = ByteBuffer.allocateDirect(4)
                    buffer.put(owner)
                    NativeBridge.nativeWifiDirectGroupFormed(info.isGroupOwner, buffer)
                }
            } else {
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
        manager.addLocalService(channel, info, null)
    }

    private fun setupServiceDiscovery(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
    ) {
        manager.setDnsSdResponseListeners(
            channel,
            { instance, registrationType, device ->
                if (registrationType.startsWith(serviceType) && instance.startsWith(instanceName)) {
                    pushSighting(device)
                }
            },
            null,
        )
        val request = WifiP2pDnsSdServiceRequest.newInstance(serviceType)
        manager.addServiceRequest(channel, request, null)
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
            manager.discoverServices(channel, null)
            discovering = true
        } else if (discovering) {
            manager.stopPeerDiscovery(channel, null)
            discovering = false
        }

        val target = ByteBuffer.allocateDirect(6)
        if (NativeBridge.nativeWifiDirectNextFormTarget(target)) {
            val octets = ByteArray(6)
            target.rewind()
            target.get(octets)
            connectTo(manager, channel, octets)
        }

        if (NativeBridge.nativeWifiDirectTakeRemoveGroup()) {
            manager.removeGroup(channel, null)
        }
    }

    private fun connectTo(
        manager: WifiP2pManager,
        channel: WifiP2pManager.Channel,
        octets: ByteArray,
    ) {
        val config = WifiP2pConfig().apply {
            deviceAddress = macString(octets)
        }
        manager.connect(channel, config, object : WifiP2pManager.ActionListener {
            override fun onSuccess() {
                Log.i(TAG, "Wi-Fi Direct connect requested to ${config.deviceAddress}")
            }

            override fun onFailure(reason: Int) {
                Log.w(TAG, "Wi-Fi Direct connect failed reason=$reason")
            }
        })
    }

    private companion object {
        private const val TAG = "HopspotWifiDirect"
        private const val POLL_INTERVAL_MS = 1000L

        private fun macString(octets: ByteArray): String =
            octets.joinToString(":") { "%02x".format(it) }

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
