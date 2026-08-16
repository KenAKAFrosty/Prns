package org.personal.hopspot

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.util.Log
import java.net.Inet4Address
import java.net.Inet6Address
import java.nio.ByteBuffer
import java.util.ArrayDeque

class WifiAutoLink(context: Context) {
    private val appContext = context.applicationContext
    private val lock =
        (appContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager)
            ?.createMulticastLock("PersonalHopspotAutoWifi")
            ?.apply { setReferenceCounted(false) }
    private val nsdManager = appContext.getSystemService(Context.NSD_SERVICE) as? NsdManager

    private var registrationListener: NsdManager.RegistrationListener? = null
    private var discoveryListener: NsdManager.DiscoveryListener? = null
    private var registeredName: String? = null

    private val resolveQueue = ArrayDeque<NsdServiceInfo>()
    private var resolving = false

    fun start() {
        acquireLock()
        startMdns()
    }

    fun stop() {
        stopMdns()
        releaseLock()
    }

    private fun acquireLock() {
        val current = lock
        if (current == null) {
            Log.i(TAG, "wifi multicast lock unavailable")
            return
        }
        try {
            current.acquire()
            Log.i(TAG, "wifi multicast lock acquired")
        } catch (e: RuntimeException) {
            Log.w(TAG, "wifi multicast lock unavailable", e)
        }
    }

    private fun releaseLock() {
        val current = lock ?: return
        if (current.isHeld) {
            current.release()
            Log.i(TAG, "wifi multicast lock released")
        }
    }

    private fun startMdns() {
        val manager = nsdManager
        if (manager == null) {
            Log.i(TAG, "nsd unavailable; multicast only")
            return
        }
        val port = NativeBridge.nativeMdnsServicePort()
        registerService(manager, port)
        discoverServices(manager)
    }

    private fun stopMdns() {
        val manager = nsdManager ?: return
        registrationListener?.let { listener ->
            try {
                manager.unregisterService(listener)
            } catch (e: IllegalArgumentException) {
                Log.d(TAG, "nsd unregister: $e")
            }
        }
        registrationListener = null
        discoveryListener?.let { listener ->
            try {
                manager.stopServiceDiscovery(listener)
            } catch (e: IllegalArgumentException) {
                Log.d(TAG, "nsd stop discovery: $e")
            }
        }
        discoveryListener = null
        synchronized(resolveQueue) {
            resolveQueue.clear()
            resolving = false
        }
    }

    private fun registerService(manager: NsdManager, port: Int) {
        val info = NsdServiceInfo().apply {
            serviceName = SERVICE_NAME
            serviceType = SERVICE_TYPE
            this.port = port
        }
        val listener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(registered: NsdServiceInfo) {
                registeredName = registered.serviceName
                Log.i(TAG, "nsd registered ${registered.serviceName} on :$port")
            }

            override fun onRegistrationFailed(info: NsdServiceInfo, errorCode: Int) {
                Log.w(TAG, "nsd register failed code=$errorCode")
            }

            override fun onServiceUnregistered(info: NsdServiceInfo) {}

            override fun onUnregistrationFailed(info: NsdServiceInfo, errorCode: Int) {}
        }
        registrationListener = listener
        manager.registerService(info, NsdManager.PROTOCOL_DNS_SD, listener)
    }

    private fun discoverServices(manager: NsdManager) {
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {
                Log.i(TAG, "nsd discovery started")
            }

            override fun onServiceFound(info: NsdServiceInfo) {
                if (info.serviceName == registeredName) {
                    return
                }
                enqueueResolve(manager, info)
            }

            override fun onServiceLost(info: NsdServiceInfo) {}

            override fun onDiscoveryStopped(serviceType: String) {}

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "nsd discovery start failed code=$errorCode")
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
        }
        discoveryListener = listener
        manager.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
    }

    private fun enqueueResolve(manager: NsdManager, info: NsdServiceInfo) {
        synchronized(resolveQueue) {
            resolveQueue.addLast(info)
        }
        pumpResolve(manager)
    }

    @Suppress("DEPRECATION")
    private fun pumpResolve(manager: NsdManager) {
        val next = synchronized(resolveQueue) {
            if (resolving) {
                return
            }
            val head = resolveQueue.pollFirst() ?: return
            resolving = true
            head
        }
        manager.resolveService(next, object : NsdManager.ResolveListener {
            override fun onServiceResolved(info: NsdServiceInfo) {
                pushSighting(info)
                advance(manager)
            }

            override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) {
                Log.d(TAG, "nsd resolve failed code=$errorCode")
                advance(manager)
            }
        })
    }

    private fun advance(manager: NsdManager) {
        synchronized(resolveQueue) {
            resolving = false
        }
        pumpResolve(manager)
    }

    @Suppress("DEPRECATION")
    private fun pushSighting(info: NsdServiceInfo) {
        val host = info.host ?: return
        val octets = when (host) {
            is Inet4Address -> host.address
            is Inet6Address -> host.address
            else -> return
        }
        if (octets.size != 4 && octets.size != 16) {
            return
        }
        val direct = ByteBuffer.allocateDirect(octets.size)
        direct.put(octets)
        NativeBridge.nativeWifiSighting(direct, info.port)
    }

    private companion object {
        private const val TAG = "HopspotWifi"
        private const val SERVICE_TYPE = "_reticulum._udp"
        private const val SERVICE_NAME = "PersonalHopspot"
    }
}
