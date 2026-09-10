// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The Prns Authors

package rs.reticulum.prns.app.expo

import java.util.UUID

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
