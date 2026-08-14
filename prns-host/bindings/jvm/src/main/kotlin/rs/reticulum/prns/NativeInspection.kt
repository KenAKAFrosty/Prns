package rs.reticulum.prns

import com.sun.jna.Pointer
import com.sun.jna.Structure

@Structure.FieldOrder(
    "structSize",
    "backend",
    "capabilities",
    "capabilityCount",
    "interfaceKinds",
    "interfaceKindCount",
)
internal open class NativeBackendInfo : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var backend: Int = 0

    @JvmField
    var capabilities: Pointer? = null

    @JvmField
    var capabilityCount: SizeT = SizeT()

    @JvmField
    var interfaceKinds: Pointer? = null

    @JvmField
    var interfaceKindCount: SizeT = SizeT()

    class ByValue : NativeBackendInfo(), Structure.ByValue
}

@Structure.FieldOrder(
    "structSize",
    "interfaceId",
    "hasName",
    "name",
    "hasKind",
    "kind",
    "health",
    "hasFailureDetail",
    "failureDetail",
    "rxBytes",
    "txBytes",
    "hasRxBps",
    "rxBps",
    "hasTxBps",
    "txBps",
    "routeCount",
    "linkCount",
    "transportedLinkCount",
)
internal class NativeInterfaceSnapshot(pointer: Pointer? = null) : Structure(pointer) {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var interfaceId: NativeByteView.ByValue = NativeByteView.ByValue()

    @JvmField
    var hasName: Byte = 0

    @JvmField
    var name: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var hasKind: Byte = 0

    @JvmField
    var kind: Int = 0

    @JvmField
    var health: Int = 0

    @JvmField
    var hasFailureDetail: Byte = 0

    @JvmField
    var failureDetail: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var rxBytes: Long = 0

    @JvmField
    var txBytes: Long = 0

    @JvmField
    var hasRxBps: Byte = 0

    @JvmField
    var rxBps: Long = 0

    @JvmField
    var hasTxBps: Byte = 0

    @JvmField
    var txBps: Long = 0

    @JvmField
    var routeCount: Int = 0

    @JvmField
    var linkCount: Int = 0

    @JvmField
    var transportedLinkCount: Int = 0
}

@Structure.FieldOrder(
    "structSize",
    "destination",
    "hops",
    "hasViaIdentity",
    "viaIdentity",
    "interfaceId",
    "learnedAtMillis",
    "lastRouteActivityAtMillis",
    "expiresAtMillis",
)
internal class NativeRouteSnapshot(pointer: Pointer? = null) : Structure(pointer) {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var destination: NativeByteView.ByValue = NativeByteView.ByValue()

    @JvmField
    var hops: Byte = 0

    @JvmField
    var hasViaIdentity: Byte = 0

    @JvmField
    var viaIdentity: NativeByteView.ByValue = NativeByteView.ByValue()

    @JvmField
    var interfaceId: NativeByteView.ByValue = NativeByteView.ByValue()

    @JvmField
    var learnedAtMillis: Long = 0

    @JvmField
    var lastRouteActivityAtMillis: Long = 0

    @JvmField
    var expiresAtMillis: Long = 0
}

@Structure.FieldOrder("structSize", "destination", "identity")
internal class NativeDestinationIdentitySnapshot(pointer: Pointer? = null) : Structure(pointer) {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var destination: NativeByteView.ByValue = NativeByteView.ByValue()

    @JvmField
    var identity: NativeByteView.ByValue = NativeByteView.ByValue()
}

@Structure.FieldOrder(
    "structSize",
    "running",
    "uptimeMillis",
    "interfaceCount",
    "onlineInterfaceCount",
    "routeCount",
    "linkCount",
    "transportedLinkCount",
    "rxBytes",
    "txBytes",
    "rxBps",
    "txBps",
)
internal open class NativeRuntimeHealthSnapshot : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var running: Byte = 0

    @JvmField
    var uptimeMillis: Long = 0

    @JvmField
    var interfaceCount: Int = 0

    @JvmField
    var onlineInterfaceCount: Int = 0

    @JvmField
    var routeCount: Int = 0

    @JvmField
    var linkCount: Int = 0

    @JvmField
    var transportedLinkCount: Int = 0

    @JvmField
    var rxBytes: Long = 0

    @JvmField
    var txBytes: Long = 0

    @JvmField
    var rxBps: Long = 0

    @JvmField
    var txBps: Long = 0

    class ByValue : NativeRuntimeHealthSnapshot(), Structure.ByValue
}

@Structure.FieldOrder(
    "structSize",
    "persistent",
    "restored",
    "hasLastFlushCause",
    "lastFlushCause",
    "hasLastFailureDetail",
    "lastFailureDetail",
)
internal open class NativePersistenceSnapshot : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var persistent: Byte = 0

    @JvmField
    var restored: Byte = 0

    @JvmField
    var hasLastFlushCause: Byte = 0

    @JvmField
    var lastFlushCause: Int = 0

    @JvmField
    var hasLastFailureDetail: Byte = 0

    @JvmField
    var lastFailureDetail: NativeStringView.ByValue = NativeStringView.ByValue()

    class ByValue : NativePersistenceSnapshot(), Structure.ByValue
}

@Structure.FieldOrder(
    "structSize",
    "revision",
    "backend",
    "interfaces",
    "interfaceCount",
    "routes",
    "routeCount",
    "activeLinkCount",
    "destinationIdentities",
    "destinationIdentityCount",
    "runtime",
    "persistence",
)
internal class NativeHostSnapshot : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var revision: Long = 0

    @JvmField
    var backend: NativeBackendInfo.ByValue = NativeBackendInfo.ByValue()

    @JvmField
    var interfaces: Pointer? = null

    @JvmField
    var interfaceCount: SizeT = SizeT()

    @JvmField
    var routes: Pointer? = null

    @JvmField
    var routeCount: SizeT = SizeT()

    @JvmField
    var activeLinkCount: Int = 0

    @JvmField
    var destinationIdentities: Pointer? = null

    @JvmField
    var destinationIdentityCount: SizeT = SizeT()

    @JvmField
    var runtime: NativeRuntimeHealthSnapshot.ByValue = NativeRuntimeHealthSnapshot.ByValue()

    @JvmField
    var persistence: NativePersistenceSnapshot.ByValue = NativePersistenceSnapshot.ByValue()
}

internal fun NativeBackendInfo.decode(): BackendInfo = BackendInfo(
    requireNotNull(BackendKind.fromRawValue(backend)),
    pointerValues(capabilities, capabilityCount).map {
        requireNotNull(Capability.fromRawValue(it))
    },
    pointerValues(interfaceKinds, interfaceKindCount).map {
        requireNotNull(InterfaceKind.fromRawValue(it))
    },
)

internal fun NativeHostSnapshot.decode(): HostSnapshot {
    val interfaceValues = structureValues(
        interfaces,
        interfaceCount,
        ::NativeInterfaceSnapshot,
    ).map { value ->
        InterfaceSnapshot(
            InterfaceId(copyBytes(value.interfaceId)),
            if (value.hasName != 0.toByte()) copyString(value.name) else null,
            if (value.hasKind != 0.toByte()) InterfaceKind.fromRawValue(value.kind) else null,
            requireNotNull(InterfaceHealth.fromRawValue(value.health)),
            if (value.hasFailureDetail != 0.toByte()) {
                copyString(value.failureDetail)
            } else {
                null
            },
            value.rxBytes.toULong(),
            value.txBytes.toULong(),
            if (value.hasRxBps != 0.toByte()) value.rxBps else null,
            if (value.hasTxBps != 0.toByte()) value.txBps else null,
            value.routeCount.toLong(),
            value.linkCount.toLong(),
            value.transportedLinkCount.toLong(),
        )
    }
    val routeValues = structureValues(routes, routeCount, ::NativeRouteSnapshot).map { value ->
        RouteSnapshot(
            DestinationHash(copyBytes(value.destination)),
            value.hops.toInt() and 0xff,
            if (value.hasViaIdentity != 0.toByte()) {
                IdentityHash(copyBytes(value.viaIdentity))
            } else {
                null
            },
            InterfaceId(copyBytes(value.interfaceId)),
            value.learnedAtMillis,
            value.lastRouteActivityAtMillis,
            value.expiresAtMillis,
        )
    }
    val identityValues = structureValues(
        destinationIdentities,
        destinationIdentityCount,
        ::NativeDestinationIdentitySnapshot,
    ).map { value ->
        DestinationIdentitySnapshot(
            DestinationHash(copyBytes(value.destination)),
            IdentityHash(copyBytes(value.identity)),
        )
    }
    return HostSnapshot(
        revision.toULong(),
        backend.decode(),
        interfaceValues,
        routeValues,
        activeLinkCount.toLong(),
        identityValues,
        RuntimeHealthSnapshot(
            runtime.running != 0.toByte(),
            runtime.uptimeMillis,
            runtime.interfaceCount.toLong(),
            runtime.onlineInterfaceCount.toLong(),
            runtime.routeCount.toLong(),
            runtime.linkCount.toLong(),
            runtime.transportedLinkCount.toLong(),
            runtime.rxBytes.toULong(),
            runtime.txBytes.toULong(),
            runtime.rxBps,
            runtime.txBps,
        ),
        PersistenceSnapshot(
            persistence.persistent != 0.toByte(),
            persistence.restored != 0.toByte(),
            if (persistence.hasLastFlushCause != 0.toByte()) {
                PersistenceFlushCause.fromRawValue(persistence.lastFlushCause)
            } else {
                null
            },
            if (persistence.hasLastFailureDetail != 0.toByte()) {
                copyString(persistence.lastFailureDetail)
            } else {
                null
            },
        ),
    )
}

private fun pointerValues(pointer: Pointer?, count: SizeT): List<Int> {
    val length = count.toLong().toInt()
    if (length == 0) {
        return emptyList()
    }
    return requireNotNull(pointer).getIntArray(0, length).toList()
}

private fun <Value : Structure> structureValues(
    pointer: Pointer?,
    count: SizeT,
    construct: (Pointer?) -> Value,
): List<Value> {
    val length = count.toLong().toInt()
    if (length == 0) {
        return emptyList()
    }
    val first = construct(requireNotNull(pointer))
    return first.toArray(length).map { raw ->
        @Suppress("UNCHECKED_CAST")
        val value = raw as Value
        value.read()
        value
    }
}
