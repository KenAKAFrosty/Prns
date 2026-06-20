package org.personal.hopspot

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Rect
import android.os.Build
import android.os.Bundle
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.View
import java.nio.ByteBuffer

class MainActivity : Activity() {
    private var handle: Long = 0L
    private var usbLink: UsbLink? = null
    private var wifiAutoLink: WifiAutoLink? = null
    private var bleLink: BleLink? = null
    private var hopspotView: HopspotView? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        handle = NativeBridge.nativeInit()
        wifiAutoLink = WifiAutoLink(this).also { it.start() }
        usbLink = UsbLink(this).also { it.start() }
        bleLink = BleLink(this)
        ensureBlePermissionsThenStart()
        hopspotView = HopspotView(this, handle).also { setContentView(it) }
    }

    override fun onDestroy() {
        super.onDestroy()
        hopspotView?.stop()
        hopspotView = null
        usbLink?.stop()
        usbLink = null
        bleLink?.stop()
        bleLink = null
        wifiAutoLink?.stop()
        wifiAutoLink = null
        if (handle != 0L) {
            NativeBridge.nativeFree(handle)
            handle = 0L
        }
    }

    private fun ensureBlePermissionsThenStart() {
        val needed = blePermissions().filter {
            checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED
        }
        if (needed.isEmpty()) {
            bleLink?.start()
        } else {
            requestPermissions(needed.toTypedArray(), BLE_PERMISSION_REQUEST)
        }
    }

    private fun blePermissions(): List<String> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            listOf(
                Manifest.permission.BLUETOOTH_SCAN,
                Manifest.permission.BLUETOOTH_ADVERTISE,
                Manifest.permission.BLUETOOTH_CONNECT,
            )
        } else {
            listOf(Manifest.permission.ACCESS_FINE_LOCATION)
        }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != BLE_PERMISSION_REQUEST) {
            return
        }
        val granted = grantResults.isNotEmpty() &&
            grantResults.all { it == PackageManager.PERMISSION_GRANTED }
        if (granted) {
            bleLink?.start()
        }
    }

    private companion object {
        private const val BLE_PERMISSION_REQUEST = 1
    }
}

private class HopspotView(
    context: android.content.Context,
    private val handle: Long,
) : View(context) {
    private val bitmap = Bitmap.createBitmap(
        NativeBridge.PANEL_WIDTH,
        NativeBridge.PANEL_HEIGHT,
        Bitmap.Config.ARGB_8888,
    )
    private val buffer = ByteBuffer.allocateDirect(NativeBridge.ARGB_BYTES)
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
                act(NativeBridge.nativePostInput(handle, NativeBridge.INPUT_SHORT_PRESS))
                invalidate()
                return true
            }

            override fun onLongPress(e: MotionEvent) {
                act(NativeBridge.nativePostInput(handle, NativeBridge.INPUT_LONG_PRESS))
                invalidate()
            }

            private fun act(action: Int) {
                if (action == NativeBridge.ACTION_ANNOUNCE) {
                    NativeBridge.nativeAnnounce()
                }
            }
        },
    )
    private val ticker = object : Runnable {
        override fun run() {
            invalidate()
            postDelayed(this, FRAME_DELAY_MS)
        }
    }

    init {
        setBackgroundColor(android.graphics.Color.BLACK)
        post(ticker)
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        NativeBridge.nativeRender(handle, buffer)
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

    fun stop() {
        removeCallbacks(ticker)
    }

    private companion object {
        private const val FRAME_DELAY_MS = 33L
    }
}
