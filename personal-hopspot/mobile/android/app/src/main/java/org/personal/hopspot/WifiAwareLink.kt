package org.personal.hopspot

import android.Manifest
import android.annotation.SuppressLint
import android.annotation.TargetApi
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.aware.AttachCallback
import android.net.wifi.aware.DiscoverySession
import android.net.wifi.aware.DiscoverySessionCallback
import android.net.wifi.aware.PeerHandle
import android.net.wifi.aware.PublishConfig
import android.net.wifi.aware.PublishDiscoverySession
import android.net.wifi.aware.SubscribeConfig
import android.net.wifi.aware.SubscribeDiscoverySession
import android.net.wifi.aware.WifiAwareManager
import android.net.wifi.aware.WifiAwareNetworkInfo
import android.net.wifi.aware.WifiAwareNetworkSpecifier
import android.net.wifi.aware.WifiAwareSession
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.net.Inet6Address
import java.net.NetworkInterface
import java.nio.ByteBuffer

@SuppressLint("MissingPermission")
@TargetApi(Build.VERSION_CODES.O)
class WifiAwareLink(context: Context) {
    private val appContext = context.applicationContext
    private val manager =
        appContext.getSystemService(Context.WIFI_AWARE_SERVICE) as? WifiAwareManager
    private val connectivity =
        appContext.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
    private val handler = Handler(Looper.getMainLooper())

    private val serviceName = NativeBridge.nativeWifiAwareServiceName()
    private val passphrase = NativeBridge.nativeWifiAwarePassphrase()
    private val localToken = NativeBridge.nativeWifiAwareLocalToken()
    private val localTokenBytes = ByteBuffer.allocate(4).putInt(localToken).array()

    private var receiver: BroadcastReceiver? = null
    private var session: WifiAwareSession? = null
    private var publishSession: PublishDiscoverySession? = null
    private var subscribeSession: SubscribeDiscoverySession? = null
    private var attaching = false
    private var discovering = false
    private var messageId = 0

    private val discoveredPeers = HashMap<Int, PeerHandle>()
    private val requestingPeers = HashMap<Int, PeerHandle>()
    private val pendingInitiators = HashSet<Int>()
    private val callbacks = HashMap<Pair<Int, Boolean>, ConnectivityManager.NetworkCallback>()

    fun start() {
        val manager = manager
        if (manager == null) {
            Log.i(TAG, "Wi-Fi Aware service unavailable on this device")
            NativeBridge.nativeWifiAwareAvailability(NativeBridge.WIFI_AWARE_DISABLED)
            return
        }
        registerReceiver()
        reportAvailability()
        maybeAttach()
        handler.post(pollLoop)
    }

    fun stop() {
        handler.removeCallbacksAndMessages(null)
        receiver?.let { runCatching { appContext.unregisterReceiver(it) } }
        receiver = null
        val connectivity = connectivity
        if (connectivity != null) {
            for (callback in callbacks.values) {
                runCatching { connectivity.unregisterNetworkCallback(callback) }
            }
        }
        callbacks.clear()
        discoveredPeers.clear()
        requestingPeers.clear()
        pendingInitiators.clear()
        publishSession?.close()
        subscribeSession?.close()
        publishSession = null
        subscribeSession = null
        session?.close()
        session = null
        discovering = false
    }

    private fun hasPermission(): Boolean {
        val needed =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                Manifest.permission.NEARBY_WIFI_DEVICES
            } else {
                Manifest.permission.ACCESS_FINE_LOCATION
            }
        return appContext.checkSelfPermission(needed) == PackageManager.PERMISSION_GRANTED
    }

    private fun reportAvailability() {
        val manager = manager
        val code =
            when {
                !hasPermission() -> NativeBridge.WIFI_AWARE_NO_PERMISSION
                manager == null || !manager.isAvailable -> NativeBridge.WIFI_AWARE_DISABLED
                else -> NativeBridge.WIFI_AWARE_AVAILABLE
            }
        NativeBridge.nativeWifiAwareAvailability(code)
    }

    private fun registerReceiver() {
        val filter = IntentFilter(WifiAwareManager.ACTION_WIFI_AWARE_STATE_CHANGED)
        val listener = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                reportAvailability()
                if (manager?.isAvailable == true) {
                    maybeAttach()
                } else {
                    onAwareLost()
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

    private fun onAwareLost() {
        publishSession = null
        subscribeSession = null
        session = null
        discovering = false
        discoveredPeers.clear()
        requestingPeers.clear()
        pendingInitiators.clear()
    }

    private fun maybeAttach() {
        val manager = manager ?: return
        if (session != null || attaching || !manager.isAvailable || !hasPermission()) {
            return
        }
        attaching = true
        manager.attach(object : AttachCallback() {
            override fun onAttached(newSession: WifiAwareSession) {
                attaching = false
                session = newSession
                Log.i(TAG, "Wi-Fi Aware attached (token=$localToken)")
            }

            override fun onAttachFailed() {
                attaching = false
                Log.w(TAG, "Wi-Fi Aware attach failed")
            }
        }, handler)
    }

    private val pollLoop = object : Runnable {
        override fun run() {
            pumpDesiredState()
            handler.postDelayed(this, POLL_INTERVAL_MS)
        }
    }

    private fun pumpDesiredState() {
        if (!hasPermission()) {
            return
        }
        maybeAttach()
        val wantDiscovery = NativeBridge.nativeWifiAwareDesiredDiscovery()
        if (wantDiscovery) {
            startDiscovery()
        } else {
            stopDiscovery()
        }

        var request = NativeBridge.nativeWifiAwareTakeRequest()
        while (request >= 0) {
            requestDataPath(request.toInt(), (request shr 32) and 1L == 1L)
            request = NativeBridge.nativeWifiAwareTakeRequest()
        }

        var abandon = NativeBridge.nativeWifiAwareTakeAbandon()
        while (abandon >= 0) {
            abandonDataPath(abandon.toInt(), (abandon shr 32) and 1L == 1L)
            abandon = NativeBridge.nativeWifiAwareTakeAbandon()
        }
    }

    private fun startDiscovery() {
        val session = session ?: return
        if (discovering) {
            return
        }
        discovering = true
        val publishConfig = PublishConfig.Builder()
            .setServiceName(serviceName)
            .setServiceSpecificInfo(localTokenBytes)
            .build()
        session.publish(publishConfig, object : DiscoverySessionCallback() {
            override fun onPublishStarted(discovery: PublishDiscoverySession) {
                publishSession = discovery
                Log.i(TAG, "Wi-Fi Aware publish started")
            }

            override fun onMessageReceived(peerHandle: PeerHandle, message: ByteArray) {
                val token = readToken(message) ?: return
                requestingPeers[token] = peerHandle
                Log.i(TAG, "Wi-Fi Aware inbound request from token=$token")
                NativeBridge.nativeWifiAwareNdpRequested(token)
            }
        }, handler)

        val subscribeConfig = SubscribeConfig.Builder()
            .setServiceName(serviceName)
            .build()
        session.subscribe(subscribeConfig, object : DiscoverySessionCallback() {
            override fun onSubscribeStarted(discovery: SubscribeDiscoverySession) {
                subscribeSession = discovery
                Log.i(TAG, "Wi-Fi Aware subscribe started")
            }

            override fun onServiceDiscovered(
                peerHandle: PeerHandle,
                serviceSpecificInfo: ByteArray,
                matchFilter: MutableList<ByteArray>,
            ) {
                val token = readToken(serviceSpecificInfo) ?: return
                discoveredPeers[token] = peerHandle
                Log.i(TAG, "Wi-Fi Aware discovered peer token=$token")
                NativeBridge.nativeWifiAwarePeerDiscovered(token)
            }

            override fun onMessageReceived(peerHandle: PeerHandle, message: ByteArray) {
                val token = readToken(message) ?: return
                if (pendingInitiators.remove(token)) {
                    discoveredPeers[token] = peerHandle
                    val discovery = subscribeSession ?: return
                    Log.i(TAG, "Wi-Fi Aware responder ready; initiating peer=$token")
                    openNetwork(token, true, discovery, peerHandle)
                }
            }
        }, handler)
    }

    private fun stopDiscovery() {
        if (!discovering) {
            return
        }
        discovering = false
        publishSession?.close()
        subscribeSession?.close()
        publishSession = null
        subscribeSession = null
    }

    private fun requestDataPath(peer: Int, isInitiator: Boolean) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return
        }
        if (isInitiator) {
            val discovery = subscribeSession ?: return
            val handle = discoveredPeers[peer] ?: return
            pendingInitiators.add(peer)
            discovery.sendMessage(handle, nextMessageId(), localTokenBytes)
            Log.i(TAG, "Wi-Fi Aware initiating handshake with peer=$peer")
        } else {
            val discovery = publishSession ?: return
            val handle = requestingPeers[peer] ?: return
            openNetwork(peer, false, discovery, handle)
            discovery.sendMessage(handle, nextMessageId(), localTokenBytes)
        }
    }

    @TargetApi(Build.VERSION_CODES.Q)
    private fun openNetwork(
        peer: Int,
        isInitiator: Boolean,
        discovery: DiscoverySession,
        handle: PeerHandle,
    ) {
        val connectivity = connectivity ?: return
        Log.i(TAG, "Wi-Fi Aware requesting network peer=$peer initiator=$isInitiator")
        val specifier = WifiAwareNetworkSpecifier.Builder(discovery, handle)
            .setPskPassphrase(passphrase)
            .build()
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI_AWARE)
            .setNetworkSpecifier(specifier)
            .build()
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
                onDataPathAvailable(peer, isInitiator, network, caps)
            }

            override fun onLost(network: Network) {
                Log.i(TAG, "Wi-Fi Aware data path lost peer=$peer initiator=$isInitiator")
                NativeBridge.nativeWifiAwareDataPathDown(peer, isInitiator)
            }

            override fun onUnavailable() {
                Log.w(TAG, "Wi-Fi Aware NDP unavailable peer=$peer initiator=$isInitiator")
                NativeBridge.nativeWifiAwareNdpFailed(peer, isInitiator)
            }
        }
        val key = Pair(peer, isInitiator)
        callbacks.remove(key)?.let { runCatching { connectivity.unregisterNetworkCallback(it) } }
        callbacks[key] = callback
        connectivity.requestNetwork(request, callback)
    }

    @TargetApi(Build.VERSION_CODES.Q)
    private fun onDataPathAvailable(
        peer: Int,
        isInitiator: Boolean,
        network: Network,
        caps: NetworkCapabilities,
    ) {
        val connectivity = connectivity ?: return
        val info = caps.transportInfo as? WifiAwareNetworkInfo ?: return
        val interfaceName = connectivity.getLinkProperties(network)?.interfaceName ?: return
        val nif = NetworkInterface.getByName(interfaceName) ?: return
        val address =
            if (isInitiator) {
                info.peerIpv6Addr?.address ?: return
            } else {
                linkLocalAddress(nif) ?: return
            }
        if (address.size != 16) {
            return
        }
        val buffer = ByteBuffer.allocateDirect(16)
        buffer.put(address)
        Log.i(
            TAG,
            "Wi-Fi Aware data path UP peer=$peer initiator=$isInitiator iface=$interfaceName scope=${nif.index}",
        )
        NativeBridge.nativeWifiAwareDataPathUp(peer, isInitiator, buffer, nif.index)
    }

    private fun abandonDataPath(peer: Int, isInitiator: Boolean) {
        val connectivity = connectivity ?: return
        callbacks.remove(Pair(peer, isInitiator))?.let {
            runCatching { connectivity.unregisterNetworkCallback(it) }
        }
    }

    private fun nextMessageId(): Int {
        messageId += 1
        return messageId
    }

    private companion object {
        private const val TAG = "HopspotWifiAware"
        private const val POLL_INTERVAL_MS = 1000L

        private fun readToken(bytes: ByteArray?): Int? {
            if (bytes == null || bytes.size < 4) {
                return null
            }
            return ByteBuffer.wrap(bytes).int
        }

        private fun linkLocalAddress(nif: NetworkInterface): ByteArray? {
            val addresses = nif.inetAddresses
            while (addresses.hasMoreElements()) {
                val candidate = addresses.nextElement()
                if (candidate is Inet6Address && candidate.isLinkLocalAddress) {
                    return candidate.address
                }
            }
            return null
        }
    }
}
