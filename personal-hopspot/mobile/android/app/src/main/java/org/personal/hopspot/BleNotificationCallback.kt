package org.personal.hopspot

import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic

/** Delivers both pre-API-33 and current notifications through one transport handler. */
internal abstract class BleNotificationCallback : BluetoothGattCallback() {
    protected abstract fun onNotification(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        value: ByteArray,
    )

    @Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
    final override fun onCharacteristicChanged(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
    ) {
        onNotification(gatt, characteristic, characteristic.value ?: ByteArray(0))
    }

    final override fun onCharacteristicChanged(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        value: ByteArray,
    ) {
        // Do not call the superclass: its modern implementation forwards to the
        // legacy overload, which would deliver this notification a second time.
        onNotification(gatt, characteristic, value)
    }
}
