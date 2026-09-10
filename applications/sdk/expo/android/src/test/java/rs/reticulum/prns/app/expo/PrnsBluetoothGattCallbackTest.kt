// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.app.expo

import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.mockito.Mockito.mock
import org.mockito.Mockito.`when`

@Suppress("DEPRECATION")
class PrnsBluetoothGattCallbackTest {
    private class RecordingCallback : PrnsBluetoothGattCallback() {
        var calls = 0
        var receivedGatt: BluetoothGatt? = null
        var receivedCharacteristic: BluetoothGattCharacteristic? = null
        var receivedValue: ByteArray? = null

        override fun onCharacteristicChanged(
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

    @Test
    fun android10NotificationReachesTheSameTransportHandler() {
        val gatt = mock(BluetoothGatt::class.java)
        val characteristic = mock(BluetoothGattCharacteristic::class.java)
        val payload = byteArrayOf(0, 1, 0x7f, 0xff.toByte())
        `when`(characteristic.value).thenReturn(payload)
        val callback = RecordingCallback()

        callback.onCharacteristicChanged(gatt, characteristic)

        assertEquals(1, callback.calls)
        assertSame(gatt, callback.receivedGatt)
        assertSame(characteristic, callback.receivedCharacteristic)
        assertArrayEquals(payload, callback.receivedValue)
    }

    @Test
    fun missingLegacyValueIsDeliveredAsEmptyWithoutCrashing() {
        val callback = RecordingCallback()

        callback.onCharacteristicChanged(
            mock(BluetoothGatt::class.java),
            mock(BluetoothGattCharacteristic::class.java),
        )

        assertEquals(1, callback.calls)
        assertArrayEquals(byteArrayOf(), callback.receivedValue)
    }

    @Test
    fun modernNotificationUsesItsSuppliedValueNotTheMutableCharacteristic() {
        val callback = RecordingCallback()
        val characteristic = mock(BluetoothGattCharacteristic::class.java)
        `when`(characteristic.value).thenReturn(byteArrayOf(9))
        val payload = byteArrayOf(3, 4)

        callback.onCharacteristicChanged(mock(BluetoothGatt::class.java), characteristic, payload)

        assertEquals(1, callback.calls)
        assertArrayEquals(payload, callback.receivedValue)
    }

    @Test
    fun visibleActivityDoesNotRequireBackgroundLocationForDiscovery() {
        assertTrue(prnsBluetoothDiscoveryAllowed(29, true, false))
        assertTrue(prnsBluetoothDiscoveryAllowed(30, true, false))
    }

    @Test
    fun backgroundDiscoveryIsPausedWithoutTheSeparateLegacyGrant() {
        assertFalse(prnsBluetoothDiscoveryAllowed(29, false, false))
        assertFalse(prnsBluetoothDiscoveryAllowed(30, false, false))
    }

    @Test
    fun backgroundGrantAllowsLegacyServiceDiscovery() {
        assertTrue(prnsBluetoothDiscoveryAllowed(29, false, true))
        assertTrue(prnsBluetoothDiscoveryAllowed(30, false, true))
    }

    @Test
    fun nearbyDevicePermissionPlatformsDoNotNeedBackgroundLocation() {
        assertTrue(prnsBluetoothDiscoveryAllowed(31, false, false))
        assertTrue(prnsBluetoothDiscoveryAllowed(36, false, false))
    }

    @Test
    fun publishedPsmNotifiesTheOwnerOnceAfterNativePublication() {
        val events = mutableListOf<Pair<String, Int>>()

        prnsPublishBluetoothPsm(
            37,
            { events.add("native" to it) },
            { events.add("owner" to it) },
        )

        assertEquals(listOf("native" to 37, "owner" to 37), events)
    }

    @Test(expected = IllegalArgumentException::class)
    fun invalidPsmCannotReachTheNativeSessionOrOwner() {
        prnsPublishBluetoothPsm(0, { throw AssertionError("published invalid PSM") }, {
            throw AssertionError("reported invalid PSM")
        })
    }
}
