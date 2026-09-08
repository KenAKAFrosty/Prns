package org.personal.hopspot

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

/** Owns the link's single outstanding GATT operation and bounded startup lifecycle. */
internal class GattState {
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

    // Use only for a synchronously rejected platform request. A local timeout does not cancel
    // an accepted Android operation; its matching callback or terminal link close owns release.
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
        val startedAt = startupAtMillis ?: return false
        if (closed || ready || nowMillis - startedAt < timeoutMillis) return false
        // A deadline is terminal, not permission to overlap an outstanding Android operation.
        close()
        return true
    }

    @Synchronized
    fun close() {
        closed = true
        pending = null
    }
}
