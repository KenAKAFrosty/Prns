package org.personal.hopspot

import java.nio.ByteBuffer

object NativeBridge {
    const val INPUT_SHORT_PRESS = 0
    const val INPUT_LONG_PRESS = 1

    const val ACTION_NONE = 0
    const val ACTION_ANNOUNCE = 1

    const val PANEL_WIDTH = 64
    const val PANEL_HEIGHT = 128
    const val ARGB_BYTES = PANEL_WIDTH * PANEL_HEIGHT * 4

    init {
        System.loadLibrary("personal_hopspot_android")
    }

    external fun nativeInit(): Long

    external fun nativeFree(handle: Long)

    external fun nativePostInput(handle: Long, code: Int): Int

    external fun nativeRender(handle: Long, buffer: ByteBuffer)

    external fun nativeUsbConnected(connected: Boolean)

    external fun nativeUsbRx(buffer: ByteBuffer, len: Int)

    external fun nativeUsbTx(buffer: ByteBuffer): Int

    external fun nativeBleSetPsm(psm: Int)

    external fun nativeBleSighting(address: ByteBuffer, rssi: Int)

    external fun nativeBleLinkUp(connId: Int, address: ByteBuffer, rssi: Int, dialed: Boolean)

    external fun nativeBleControlIn(connId: Int, buffer: ByteBuffer, len: Int)

    external fun nativeBleControlOut(connId: Int, buffer: ByteBuffer): Int

    external fun nativeBleL2capIn(connId: Int, buffer: ByteBuffer, len: Int)

    external fun nativeBleL2capOut(connId: Int, buffer: ByteBuffer): Int

    external fun nativeBleDataIn(connId: Int, buffer: ByteBuffer, len: Int)

    external fun nativeBleDataOut(connId: Int, buffer: ByteBuffer): Int

    external fun nativeBleL2capUp(connId: Int)

    external fun nativeBleDisconnected(connId: Int)

    external fun nativeBleNextDial(buffer: ByteBuffer): Boolean

    external fun nativeBleNextL2capOpen(buffer: ByteBuffer): Boolean
}
