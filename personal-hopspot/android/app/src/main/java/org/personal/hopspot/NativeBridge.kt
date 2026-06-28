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

    external fun nativeAnnounce()

    external fun nativeRuntimeHealth(): LongArray?

    external fun nativeRpcKeyHex(): String?

    external fun nativeRender(handle: Long, buffer: ByteBuffer)

    external fun nativeSetBattery(handle: Long, percent: Int, charging: Boolean)

    external fun nativeUsbConnected(connected: Boolean)

    external fun nativeUsbAutoVendorId(): Int

    external fun nativeUsbAutoProductId(): Int

    external fun nativeUsbAccessoryManufacturer(): String

    external fun nativeUsbAccessoryModel(): String

    external fun nativeUsbAccessoryDescription(): String

    external fun nativeUsbAccessoryVersion(): String

    external fun nativeUsbAccessoryUri(): String

    external fun nativeUsbAccessorySerial(): String

    external fun nativeUsbRx(buffer: ByteBuffer, len: Int)

    external fun nativeUsbTx(buffer: ByteBuffer): Int

    external fun nativeRendezvousPort(): Int

    external fun nativeWifiSighting(address: ByteBuffer, port: Int)

    external fun nativeBleSetPsm(psm: Int)

    external fun nativeBleSighting(address: ByteBuffer, rssi: Int)

    external fun nativeBleDialFailed(address: ByteBuffer)

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

    fun runtimeHealth(): PrnsRuntimeHealth =
        PrnsRuntimeHealth.fromNative(nativeRuntimeHealth())
}

data class PrnsRuntimeHealth(
    val runtimeUptimeMs: Long,
    val interfaceCount: Int,
    val onlineInterfaceCount: Int,
    val localClientCount: Int,
    val routeCount: Int,
    val linkCount: Int,
    val transportedLinkCount: Int,
    val rxBytes: Long,
    val txBytes: Long,
    val rxBps: Long,
    val txBps: Long,
) {
    companion object {
        private const val FIELD_COUNT = 11

        fun fromNative(values: LongArray?): PrnsRuntimeHealth {
            if (values == null || values.size < FIELD_COUNT) {
                return PrnsRuntimeHealth(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
            }
            return PrnsRuntimeHealth(
                runtimeUptimeMs = values[0].coerceAtLeast(0),
                interfaceCount = values[1].toNonNegativeInt(),
                onlineInterfaceCount = values[2].toNonNegativeInt(),
                localClientCount = values[3].toNonNegativeInt(),
                routeCount = values[4].toNonNegativeInt(),
                linkCount = values[5].toNonNegativeInt(),
                transportedLinkCount = values[6].toNonNegativeInt(),
                rxBytes = values[7].coerceAtLeast(0),
                txBytes = values[8].coerceAtLeast(0),
                rxBps = values[9].coerceAtLeast(0),
                txBps = values[10].coerceAtLeast(0),
            )
        }

        private fun Long.toNonNegativeInt(): Int =
            coerceIn(0, Int.MAX_VALUE.toLong()).toInt()
    }
}
