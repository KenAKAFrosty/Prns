package org.personal.hopspot

import android.graphics.Bitmap
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import java.nio.ByteBuffer

class MainActivity : ComponentActivity() {
    private var handle: Long = 0L
    private var usbLink: UsbLink? = null
    private var wifiAutoLink: WifiAutoLink? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        handle = NativeBridge.nativeInit()
        wifiAutoLink = WifiAutoLink(this).also { it.start() }
        usbLink = UsbLink(this).also { it.start() }
        setContent { HopspotScreen(handle) }
    }

    override fun onDestroy() {
        super.onDestroy()
        usbLink?.stop()
        usbLink = null
        wifiAutoLink?.stop()
        wifiAutoLink = null
        if (handle != 0L) {
            NativeBridge.nativeFree(handle)
            handle = 0L
        }
    }
}

@Composable
private fun HopspotScreen(handle: Long) {
    val bitmap = remember {
        Bitmap.createBitmap(
            NativeBridge.PANEL_WIDTH,
            NativeBridge.PANEL_HEIGHT,
            Bitmap.Config.ARGB_8888,
        )
    }
    val buffer = remember { ByteBuffer.allocateDirect(NativeBridge.ARGB_BYTES) }
    var image by remember { mutableStateOf<ImageBitmap?>(null) }

    LaunchedEffect(handle) {
        while (true) {
            withFrameNanos { }
            NativeBridge.nativeRender(handle, buffer)
            buffer.rewind()
            bitmap.copyPixelsFromBuffer(buffer)
            buffer.rewind()
            image = bitmap.copy(Bitmap.Config.ARGB_8888, false).asImageBitmap()
        }
    }

    val current = image
    if (current != null) {
        Image(
            bitmap = current,
            contentDescription = null,
            modifier = Modifier
                .fillMaxSize()
                .background(Color.Black)
                .pointerInput(handle) {
                    detectTapGestures(
                        onTap = {
                            NativeBridge.nativePostInput(handle, NativeBridge.INPUT_SHORT_PRESS)
                        },
                        onLongPress = {
                            NativeBridge.nativePostInput(handle, NativeBridge.INPUT_LONG_PRESS)
                        },
                    )
                },
            contentScale = ContentScale.Fit,
            filterQuality = FilterQuality.None,
        )
    }
}
