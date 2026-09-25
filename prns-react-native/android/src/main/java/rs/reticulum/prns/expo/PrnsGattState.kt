// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.expo

import android.bluetooth.BluetoothGatt
import java.util.UUID

internal const val ERROR_GATT_WRITE_REQUEST_BUSY = 201

internal enum class OutboundAdmission { Accepted, Busy, Terminal }

internal data class GattWriteSubmission(val admission: OutboundAdmission, val status: Int? = null)

/** Every client write occupies Android's operation lane, including writes without ATT response. */
internal fun prnsSubmitGattClientWrite(
    state: PrnsGattState,
    characteristic: UUID,
    write: () -> Int,
): GattWriteSubmission {
    val operation = PendingGattOperation(GattOperationKind.ClientWrite, characteristic)
    if (!state.begin(operation)) return GattWriteSubmission(OutboundAdmission.Busy)
    val status = try {
        write()
    } catch (error: Exception) {
        state.cancel(operation)
        throw error
    }
    if (status == BluetoothGatt.GATT_SUCCESS) {
        return GattWriteSubmission(OutboundAdmission.Accepted, status)
    }
    state.cancel(operation)
    return GattWriteSubmission(
        if (status == ERROR_GATT_WRITE_REQUEST_BUSY) OutboundAdmission.Busy else OutboundAdmission.Terminal,
        status,
    )
}

internal data class GattWriteCompletion(val releasedPending: Boolean, val shouldClose: Boolean)

internal fun prnsCompleteGattClientWrite(
    state: PrnsGattState,
    characteristic: UUID,
    status: Int,
): GattWriteCompletion = synchronized(state) {
    val released = state.complete(GattOperationKind.ClientWrite, characteristic)
    // An error is still terminal when no matching operation was tracked.
    // A mismatched successful callback must not release another operation.
    val failed = status != BluetoothGatt.GATT_SUCCESS
    if (failed) state.close()
    GattWriteCompletion(released, failed)
}

internal enum class GattOperationKind {
    Mtu,
    ServiceDiscovery,
    CharacteristicRead,
    DescriptorWrite,
    ClientWrite,
    ServerNotify,
}

internal data class PendingGattOperation(
    val kind: GattOperationKind,
    val characteristic: UUID?,
)

/** One link's actual operation ownership and bounded client startup. */
internal class PrnsGattState {
    private var pending: PendingGattOperation? = null
    private var servicesRequested = false
    private var startupAtMillis: Long? = null
    private var ready = false
    private var closed = false

    @Synchronized
    fun begin(operation: PendingGattOperation): Boolean {
        if (closed || pending != null) return false
        pending = operation
        return true
    }

    @Synchronized
    fun complete(kind: GattOperationKind, characteristic: UUID? = null): Boolean {
        val operation = pending ?: return false
        if (closed || operation.kind != kind ||
            characteristic != null && operation.characteristic != characteristic
        ) return false
        pending = null
        return true
    }

    // Only for a platform request rejected synchronously. This cannot cancel an
    // Android operation that has already started; its matching callback owns it.
    @Synchronized
    fun cancel(operation: PendingGattOperation): Boolean {
        if (closed || pending != operation) return false
        pending = null
        return true
    }

    @Synchronized
    fun beginStartup(nowMillis: Long): Boolean {
        if (closed || startupAtMillis != null) return false
        startupAtMillis = nowMillis
        return true
    }

    @Synchronized
    fun beginServiceDiscovery(): Boolean {
        if (closed || servicesRequested || pending != null) return false
        servicesRequested = true
        pending = PendingGattOperation(GattOperationKind.ServiceDiscovery, null)
        return true
    }

    @Synchronized
    fun markReady(): Boolean {
        if (closed || ready) return false
        ready = true
        return true
    }

    @Synchronized
    fun expireStartup(nowMillis: Long, timeoutMillis: Long): Boolean {
        val started = startupAtMillis ?: return false
        if (closed || ready || nowMillis - started < timeoutMillis) return false
        close()
        return true
    }

    @Synchronized
    fun close() {
        closed = true
        pending = null
    }
}
