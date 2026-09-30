// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.bluetooth

import android.bluetooth.BluetoothGatt
import java.util.UUID

internal enum class CapabilityProgress { Waiting, Ready, Rejected, Ignored, Expired }

/** One optional read on one physical client. The native codec owns the capability format. */
internal class GattCapabilityNegotiation(private val supports: (ByteArray) -> Boolean) {
    private enum class Phase { New, Reading, Resolved }
    private var phase = Phase.New
    private var characteristic: UUID? = null
    private var supported = false

    @Synchronized
    fun isSupported(): Boolean = phase == Phase.Resolved && supported

    /** Ready means there is no accepted Android read still occupying the operation lane. */
    @Synchronized
    fun start(state: GattState, uuid: UUID?, read: () -> Boolean): CapabilityProgress {
        if (phase != Phase.New) return CapabilityProgress.Ignored
        if (uuid == null) {
            phase = Phase.Resolved
            return CapabilityProgress.Ready
        }
        val operation = PendingGattOperation(GattOperationKind.CharacteristicRead, uuid)
        if (!state.begin(operation)) return CapabilityProgress.Rejected
        characteristic = uuid
        phase = Phase.Reading
        val accepted = try {
            read()
        } catch (error: Exception) {
            state.cancel(operation)
            throw error
        }
        if (accepted) return CapabilityProgress.Waiting
        // A synchronous rejection owns no callback. Do not overwrite a completion that
        // happened reentrantly during submission or release a different operation's lane.
        if (!state.cancel(operation)) return CapabilityProgress.Ignored
        phase = Phase.Resolved
        return CapabilityProgress.Ready
    }

    @Synchronized
    fun complete(
        state: GattState,
        uuid: UUID,
        value: ByteArray,
        status: Int,
        nowMillis: Long,
        startupTimeoutMillis: Long,
    ): CapabilityProgress {
        if (phase != Phase.Reading || uuid != characteristic) return CapabilityProgress.Ignored
        if (state.expireStartup(nowMillis, startupTimeoutMillis)) {
            phase = Phase.Resolved
            return CapabilityProgress.Expired
        }
        if (!state.complete(GattOperationKind.CharacteristicRead, uuid)) {
            return CapabilityProgress.Ignored
        }
        supported = status == BluetoothGatt.GATT_SUCCESS && supports(value)
        phase = Phase.Resolved
        return CapabilityProgress.Ready
    }
}
