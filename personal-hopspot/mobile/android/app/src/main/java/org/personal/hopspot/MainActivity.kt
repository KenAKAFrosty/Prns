package org.personal.hopspot

import android.Manifest
import android.app.Activity
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.os.BatteryManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Rect
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import android.os.PersistableBundle
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.View
import android.widget.Toast
import java.nio.ByteBuffer

class MainActivity : Activity() {
    private var service: PrnsService? = null
    private var bound = false
    private var hopspotView: HopspotView? = null
    private var refreshLinksOnConnect = false
    private var lastPermissionState: List<Boolean>? = null

    private val serviceConnection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName, binder: IBinder) {
            val local = binder as? PrnsService.LocalBinder ?: return
            service = local.service
            hopspotView?.setService(local.service)
            if (refreshLinksOnConnect) {
                refreshLinksOnConnect = false
                local.service.refreshPlatformLinks()
            }
        }

        override fun onServiceDisconnected(name: ComponentName) {
            service = null
            hopspotView?.setService(null)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        hopspotView = HopspotView(this).also { setContentView(it) }
        startAndBindService()
        requestMissingPermissions()
    }

    override fun onDestroy() {
        super.onDestroy()
        hopspotView?.stop()
        hopspotView?.setService(null)
        hopspotView = null
        if (bound) {
            unbindService(serviceConnection)
            bound = false
        }
        service = null
    }

    override fun onResume() {
        super.onResume()
        val current = permissionState()
        val changed = lastPermissionState?.let { it != current } == true
        lastPermissionState = current
        if (changed) {
            refreshPlatformLinks()
        }
    }

    private fun requestMissingPermissions() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
            return
        }
        val needed = runtimePermissions().filter { permission ->
            checkSelfPermission(permission) != PackageManager.PERMISSION_GRANTED
        }
        if (needed.isNotEmpty()) {
            requestPermissions(needed.toTypedArray(), PRNS_PERMISSION_REQUEST)
        }
    }

    private fun runtimePermissions(): List<String> {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
            return emptyList()
        }
        val permissions = mutableListOf<String>()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            permissions += Manifest.permission.POST_NOTIFICATIONS
            permissions += Manifest.permission.NEARBY_WIFI_DEVICES
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            permissions += listOf(
                Manifest.permission.BLUETOOTH_SCAN,
                Manifest.permission.BLUETOOTH_ADVERTISE,
                Manifest.permission.BLUETOOTH_CONNECT,
            )
        }
        if (Build.VERSION.SDK_INT <= Build.VERSION_CODES.S_V2) {
            permissions += listOf(
                Manifest.permission.ACCESS_COARSE_LOCATION,
                Manifest.permission.ACCESS_FINE_LOCATION,
            )
        }
        return permissions
    }

    private fun startAndBindService() {
        PrnsService.start(this)
        if (!bound) {
            bound = bindService(
                Intent(this, PrnsService::class.java),
                serviceConnection,
                Context.BIND_AUTO_CREATE,
            )
        } else {
            service?.refreshPlatformLinks()
        }
    }

    private fun permissionState(): List<Boolean> {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
            return emptyList()
        }
        return runtimePermissions().map {
            checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
        }
    }

    private fun refreshPlatformLinks() {
        val current = service
        if (current == null) {
            refreshLinksOnConnect = true
        } else {
            current.refreshPlatformLinks()
        }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != PRNS_PERMISSION_REQUEST) {
            return
        }
        lastPermissionState = permissionState()
        refreshPlatformLinks()
    }

    private companion object {
        private const val PRNS_PERMISSION_REQUEST = 1
    }
}

private class HopspotView(
    context: android.content.Context,
) : View(context) {
    private val bitmap = Bitmap.createBitmap(
        NativeBridge.PANEL_WIDTH,
        NativeBridge.PANEL_HEIGHT,
        Bitmap.Config.ARGB_8888,
    )
    private val buffer = ByteBuffer.allocateDirect(NativeBridge.RGBA_BYTES)
    private val paint = Paint(Paint.FILTER_BITMAP_FLAG).apply {
        isFilterBitmap = false
        isDither = false
    }
    private val src = Rect(0, 0, NativeBridge.PANEL_WIDTH, NativeBridge.PANEL_HEIGHT)
    private val dst = Rect()
    private val detector = GestureDetector(
        context,
        object : GestureDetector.SimpleOnGestureListener() {
            override fun onDown(e: MotionEvent): Boolean = true

            override fun onSingleTapUp(e: MotionEvent): Boolean {
                act(service?.postInput(NativeBridge.INPUT_SHORT_PRESS) ?: NativeBridge.ACTION_NONE)
                invalidate()
                return true
            }

            override fun onLongPress(e: MotionEvent) {
                act(service?.postInput(NativeBridge.INPUT_LONG_PRESS) ?: NativeBridge.ACTION_NONE)
                invalidate()
            }

            private fun act(action: Int) {
                when (action) {
                    NativeBridge.ACTION_ANNOUNCE -> service?.announce()
                    NativeBridge.ACTION_COPY_SHARED_INSTANCE_CONFIG -> copySharedInstanceConfig()
                }
            }

            private fun copySharedInstanceConfig() {
                val config = NativeBridge.nativeSidebandJoinConfig()
                if (config.isNullOrBlank()) {
                    Toast.makeText(context, "Hopspot is not ready", Toast.LENGTH_SHORT).show()
                    return
                }
                val clip = ClipData.newPlainText("RNS config", config)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    clip.description.extras = PersistableBundle().apply {
                        putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true)
                    }
                }
                val clipboard =
                    context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                clipboard.setPrimaryClip(clip)
                Toast.makeText(context, "RNS config copied", Toast.LENGTH_SHORT).show()
            }
        },
    )
    private val ticker = object : Runnable {
        override fun run() {
            invalidate()
            postDelayed(this, NativeBridge.RENDER_INTERVAL_MILLIS)
        }
    }

    init {
        setBackgroundColor(android.graphics.Color.BLACK)
        post(ticker)
    }

    fun setService(service: PrnsService?) {
        this.service = service
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val current = service
        if (current == null) {
            canvas.drawColor(Color.BLACK)
            return
        }
        if (batteryThrottle == 0) {
            pushBattery(current)
        }
        batteryThrottle = (batteryThrottle + 1) % BATTERY_EVERY_FRAMES
        current.render(buffer)
        buffer.rewind()
        bitmap.copyPixelsFromBuffer(buffer)
        buffer.rewind()

        val scale = minOf(
            width.toFloat() / NativeBridge.PANEL_WIDTH.toFloat(),
            height.toFloat() / NativeBridge.PANEL_HEIGHT.toFloat(),
        )
        val outWidth = (NativeBridge.PANEL_WIDTH * scale).toInt()
        val outHeight = (NativeBridge.PANEL_HEIGHT * scale).toInt()
        val left = (width - outWidth) / 2
        val top = (height - outHeight) / 2
        dst.set(left, top, left + outWidth, top + outHeight)
        canvas.drawBitmap(bitmap, src, dst, paint)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        return detector.onTouchEvent(event) || super.onTouchEvent(event)
    }

    // Read the OS battery level and external-power presence from the sticky
    // ACTION_BATTERY_CHANGED intent and push
    // it to the native face. Throttled to ~1s; the sticky read needs no registered receiver and
    // works on every API level.
    private fun pushBattery(current: PrnsService) {
        val status = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
            ?: return
        val level = status.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
        val scale = status.getIntExtra(BatteryManager.EXTRA_SCALE, -1)
        if (level < 0 || scale <= 0) {
            return
        }
        val percent = level * 100 / scale
        val externallyPowered = status.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) != 0
        current.setBattery(percent, externallyPowered)
    }

    fun stop() {
        removeCallbacks(ticker)
    }

    private var batteryThrottle = 0
    private var service: PrnsService? = null

    private companion object {
        private const val BATTERY_EVERY_FRAMES = 30
    }
}
