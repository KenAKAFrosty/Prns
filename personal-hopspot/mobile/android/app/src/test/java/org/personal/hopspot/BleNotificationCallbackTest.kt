package org.personal.hopspot

import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test
import org.mockito.Mockito.CALLS_REAL_METHODS
import org.mockito.Mockito.mock
import org.mockito.Mockito.`when`

@Suppress("DEPRECATION")
class BleNotificationCallbackTest {
    private class RecordingCallback : BleNotificationCallback() {
        var calls = 0
        var receivedGatt: BluetoothGatt? = null
        var receivedCharacteristic: BluetoothGattCharacteristic? = null
        var receivedValue: ByteArray? = null

        override fun onNotification(
            gatt: BluetoothGatt,
            characteristic: BluetoothGattCharacteristic,
            value: ByteArray,
        ) {
            calls++
            receivedGatt = gatt
            receivedCharacteristic = characteristic
            receivedValue = value
        }
    }

    // Bypass the Android SDK stub constructor, but execute every callback and
    // recording method normally. No device or native library is loaded.
    private fun callback(): RecordingCallback =
        mock(RecordingCallback::class.java, CALLS_REAL_METHODS)

    @Test
    fun legacyNotificationReachesTheTransportHandlerOnce() {
        val gatt = mock(BluetoothGatt::class.java)
        val characteristic = mock(BluetoothGattCharacteristic::class.java)
        val payload = byteArrayOf(0, 1, 0x7f, 0xff.toByte())
        `when`(characteristic.value).thenReturn(payload)
        val callback = callback()

        callback.onCharacteristicChanged(gatt, characteristic)

        assertEquals(1, callback.calls)
        assertSame(gatt, callback.receivedGatt)
        assertSame(characteristic, callback.receivedCharacteristic)
        assertArrayEquals(payload, callback.receivedValue)
    }

    @Test
    fun missingLegacyValueIsDeliveredAsEmpty() {
        val callback = callback()

        callback.onCharacteristicChanged(
            mock(BluetoothGatt::class.java),
            mock(BluetoothGattCharacteristic::class.java),
        )

        assertEquals(1, callback.calls)
        assertArrayEquals(byteArrayOf(), callback.receivedValue)
    }

    @Test
    fun modernNotificationUsesItsSuppliedValueExactlyOnce() {
        val callback = callback()
        val gatt = mock(BluetoothGatt::class.java)
        val characteristic = mock(BluetoothGattCharacteristic::class.java)
        `when`(characteristic.value).thenReturn(byteArrayOf(9))
        val payload = byteArrayOf(3, 4)

        callback.onCharacteristicChanged(gatt, characteristic, payload)

        assertEquals(1, callback.calls)
        assertSame(gatt, callback.receivedGatt)
        assertSame(characteristic, callback.receivedCharacteristic)
        assertArrayEquals(payload, callback.receivedValue)
    }
}
