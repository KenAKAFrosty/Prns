package com.personal.hopspot

import android.content.Context
import android.net.wifi.WifiManager
import android.util.Log

class WifiAutoLink(context: Context) {
    private val lock =
        (context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager)
            ?.createMulticastLock("PersonalHopspotAutoWifi")
            ?.apply { setReferenceCounted(false) }

    fun start() {
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

    fun stop() {
        val current = lock ?: return
        if (current.isHeld) {
            current.release()
            Log.i(TAG, "wifi multicast lock released")
        }
    }

    private companion object {
        private const val TAG = "HopspotWifi"
    }
}
