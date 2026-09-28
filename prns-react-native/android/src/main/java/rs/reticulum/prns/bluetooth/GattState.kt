// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.bluetooth

import android.bluetooth.BluetoothGatt
import java.util.UUID

internal const val ERROR_GATT_WRITE_REQUEST_BUSY = 201

internal enum class OutboundAdmission { Accepted, Busy, Terminal }

internal data class GattWriteSubmission(val admission: OutboundAdmission, val status: Int? = null)

/** Every client write occupies Android's operation lane, including writes without ATT response. */
internal fun submitGattClientWrite(
    state: GattState,
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

internal fun completeGattClientWrite(
    state: GattState,
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

/** Resolve an OS callback only to the object that owns that physical client. */
internal fun <T : Any> gattCallbackOwner(owner: T?, callback: Any, current: (T) -> Any?): T? =
    owner?.takeIf { current(it) === callback }

/** Expiration must terminate the physical owner before its operation lane can be reused. */
internal fun expireGattOperation(
    state: GattState,
    nowMillis: Long,
    timeoutMillis: Long,
    closePhysicalOwner: () -> Unit,
): Boolean {
    if (!state.expireOutbound(nowMillis, timeoutMillis)) return false
    closePhysicalOwner()
    return true
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

/** One link's operation ownership, bounded startup, and bounded outbound progress. */
internal class GattState(private val nowMillis: () -> Long = { System.nanoTime() / 1_000_000 }) {
    private var pending: PendingGattOperation? = null
    private var outboundAtMillis: Long? = null
    private var servicesRequested = false
    private var startupAtMillis: Long? = null
    private var ready = false
    private var closed = false

    @Synchronized
    fun begin(operation: PendingGattOperation): Boolean {
        if (closed || pending != null) return false
        pending = operation
        if (operation.kind == GattOperationKind.ClientWrite || operation.kind == GattOperationKind.ServerNotify) {
            // Admission retries do not constitute progress or restart the deadline.
            if (outboundAtMillis == null) outboundAtMillis = nowMillis()
        }
        return true
    }

    @Synchronized
    fun complete(kind: GattOperationKind, characteristic: UUID? = null): Boolean {
        val operation = pending ?: return false
        if (closed || operation.kind != kind ||
            characteristic != null && operation.characteristic != characteristic
        ) return false
        pending = null
        if (kind == GattOperationKind.ClientWrite || kind == GattOperationKind.ServerNotify) {
            outboundAtMillis = null
        }
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

    /** A missing completion or persistent platform Busy is terminal, never a lane release. */
    @Synchronized
    fun expireOutbound(nowMillis: Long, timeoutMillis: Long): Boolean {
        val started = outboundAtMillis ?: return false
        if (closed || nowMillis - started < timeoutMillis) return false
        close()
        return true
    }

    @Synchronized
    fun close() {
        closed = true
        pending = null
        outboundAtMillis = null
    }
}
