package io.reticulum.prns

import com.sun.jna.IntegerType
import com.sun.jna.Library
import com.sun.jna.Memory
import com.sun.jna.Native
import com.sun.jna.Pointer
import com.sun.jna.Structure
import com.sun.jna.ptr.ByteByReference
import com.sun.jna.ptr.LongByReference
import com.sun.jna.ptr.PointerByReference
import java.nio.charset.StandardCharsets

internal class SizeT(value: Long = 0) : IntegerType(Native.SIZE_T_SIZE, value, true) {
    override fun toByte(): Byte = toLong().toByte()

    override fun toShort(): Short = toLong().toShort()
}

@Structure.FieldOrder("data", "length")
internal open class NativeByteView : Structure() {
    @JvmField
    var data: Pointer? = null

    @JvmField
    var length: SizeT = SizeT()

    class ByValue : NativeByteView(), Structure.ByValue

    class ByReference : NativeByteView(), Structure.ByReference
}

@Structure.FieldOrder("data", "length")
internal open class NativeStringView : Structure() {
    @JvmField
    var data: Pointer? = null

    @JvmField
    var length: SizeT = SizeT()

    class ByValue : NativeStringView(), Structure.ByValue
}

@Structure.FieldOrder("structSize", "abi", "schemaVersion", "productVersion")
internal class NativeContractInfo : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var abi: Int = 0

    @JvmField
    var schemaVersion: Int = 0

    @JvmField
    var productVersion: NativeStringView.ByValue = NativeStringView.ByValue()
}

@Structure.FieldOrder(
    "structSize",
    "pendingCommands",
    "applicationEvents",
    "retainedEventBytes",
    "diagnostics",
)
internal open class NativeLimits : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var pendingCommands: SizeT = SizeT()

    @JvmField
    var applicationEvents: SizeT = SizeT()

    @JvmField
    var retainedEventBytes: SizeT = SizeT()

    @JvmField
    var diagnostics: SizeT = SizeT()

    class ByValue : NativeLimits(), Structure.ByValue
}

@Structure.FieldOrder("structSize", "kind", "secret", "path")
internal open class NativeIdentityConfig : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var kind: Int = 0

    @JvmField
    var secret: NativeByteView.ByValue = NativeByteView.ByValue()

    @JvmField
    var path: NativeStringView.ByValue = NativeStringView.ByValue()

    class ByValue : NativeIdentityConfig(), Structure.ByValue
}

@Structure.FieldOrder("structSize", "appName", "aspects", "aspectCount")
internal open class NativeDestinationName : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var appName: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var aspects: Pointer? = null

    @JvmField
    var aspectCount: SizeT = SizeT()

    class ByValue : NativeDestinationName(), Structure.ByValue
}

@Structure.FieldOrder(
    "structSize",
    "kind",
    "name",
    "identityKind",
    "dedicatedIdentity",
    "announceAppData",
)
internal open class NativeDestinationConfig : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var kind: Int = 0

    @JvmField
    var name: NativeDestinationName.ByValue = NativeDestinationName.ByValue()

    @JvmField
    var identityKind: Int = 0

    @JvmField
    var dedicatedIdentity: NativeIdentityConfig.ByValue = NativeIdentityConfig.ByValue()

    @JvmField
    var announceAppData: NativeByteView.ByValue = NativeByteView.ByValue()
}

@Structure.FieldOrder(
    "structSize",
    "requiredAbi",
    "requiredProductVersion",
    "limits",
    "role",
    "identity",
    "destinations",
    "destinationCount",
    "requiredCapabilities",
    "requiredCapabilityCount",
)
internal class NativeHostOptions : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var requiredAbi: Int = 0

    @JvmField
    var requiredProductVersion: NativeStringView.ByValue = NativeStringView.ByValue()

    @JvmField
    var limits: NativeLimits.ByValue = NativeLimits.ByValue()

    @JvmField
    var role: Int = 0

    @JvmField
    var identity: NativeIdentityConfig.ByValue = NativeIdentityConfig.ByValue()

    @JvmField
    var destinations: Pointer? = null

    @JvmField
    var destinationCount: SizeT = SizeT()

    @JvmField
    var requiredCapabilities: Pointer? = null

    @JvmField
    var requiredCapabilityCount: SizeT = SizeT()
}

@Structure.FieldOrder("structSize", "outcome", "failure", "evidence", "rttMillis", "value", "detail")
internal class NativeCommandResult : Structure() {
    @JvmField
    var structSize: SizeT = SizeT()

    @JvmField
    var outcome: Int = 0

    @JvmField
    var failure: Int = 0

    @JvmField
    var evidence: Int = 0

    @JvmField
    var rttMillis: Long = 0

    @JvmField
    var value: NativeByteView.ByValue = NativeByteView.ByValue()

    @JvmField
    var detail: NativeStringView.ByValue = NativeStringView.ByValue()
}

internal interface PrnsNative : Library {
    fun prns_contract_info(info: NativeContractInfo): Int
    fun prns_host_create(options: NativeHostOptions, host: PointerByReference): Int
    fun prns_host_release(host: Pointer)
    fun prns_host_identity_hash(host: Pointer, hash: NativeByteView): Int
    fun prns_host_destination_count(host: Pointer): SizeT
    fun prns_host_destination_hash(host: Pointer, index: SizeT, hash: NativeByteView): Int
    fun prns_host_announce(
        host: Pointer,
        destination: NativeByteView.ByValue,
        interfaceId: NativeByteView.ByReference?,
        command: PointerByReference,
    ): Int
    fun prns_host_send_single_packet(
        host: Pointer,
        destination: NativeByteView.ByValue,
        payload: NativeByteView.ByValue,
        command: PointerByReference,
    ): Int
    fun prns_host_close_link(
        host: Pointer,
        linkId: NativeByteView.ByValue,
        command: PointerByReference,
    ): Int
    fun prns_host_attach_tcp_server(
        host: Pointer,
        bind: NativeStringView.ByValue,
        bitrateKind: Int,
        bitrateBps: Long,
        command: PointerByReference,
    ): Int
    fun prns_host_attach_tcp_client(
        host: Pointer,
        target: NativeStringView.ByValue,
        bitrateKind: Int,
        bitrateBps: Long,
        command: PointerByReference,
    ): Int
    fun prns_host_attach_udp(
        host: Pointer,
        local: NativeStringView.ByValue,
        peer: NativeStringView.ByValue,
        bitrateKind: Int,
        bitrateBps: Long,
        command: PointerByReference,
    ): Int
    fun prns_host_detach_interface(
        host: Pointer,
        interfaceId: NativeByteView.ByValue,
        command: PointerByReference,
    ): Int
    fun prns_host_stop(host: Pointer): Int
    fun prns_command_wait(command: Pointer, timeoutMillis: Int, result: NativeCommandResult): Int
    fun prns_command_interrupt_wait(command: Pointer)
    fun prns_command_release(command: Pointer)
    fun prns_host_claim_application_events(host: Pointer, stream: PointerByReference): Int
    fun prns_host_claim_diagnostics(host: Pointer, stream: PointerByReference): Int
    fun prns_event_stream_interrupt_wait(stream: Pointer)
    fun prns_event_stream_release(stream: Pointer)
    fun prns_event_stream_next(
        stream: Pointer,
        timeoutMillis: Int,
        event: PointerByReference,
    ): Int
    fun prns_event_release(event: Pointer)
    fun prns_event_kind(event: Pointer): Int
    fun prns_event_bytes(event: Pointer, field: Int, value: NativeByteView): Int
    fun prns_event_string(event: Pointer, field: Int, value: NativeStringView): Int
    fun prns_event_u64(event: Pointer, field: Int, value: LongByReference): Int
    fun prns_event_u128(
        event: Pointer,
        field: Int,
        low: LongByReference,
        high: LongByReference,
    ): Int
    fun prns_event_resource_stream(event: Pointer, stream: PointerByReference): Int
    fun prns_resource_stream_release(stream: Pointer)
    fun prns_resource_stream_next(
        stream: Pointer,
        maximumBytes: SizeT,
        chunk: NativeByteView,
        finished: ByteByReference,
    ): Int
}

internal object NativeApi {
    val library: PrnsNative by lazy {
        val path = System.getProperty("personal.rns.library")
            ?: System.getenv("PRNS_HOST_LIBRARY")
            ?: "prns_host"
        Native.load(path, PrnsNative::class.java)
    }
}

class StatusException(
    val operation: String,
    val status: Status,
) : RuntimeException("$operation failed with $status")

internal fun checkedStatus(rawValue: Int, operation: String) {
    val status = Status.fromRawValue(rawValue) ?: Status.BACKEND_FAILED
    if (status != Status.OK) {
        throw StatusException(operation, status)
    }
}

internal fun copyBytes(view: NativeByteView): ByteArray {
    val length = view.length.toLong()
    if (length == 0L) {
        return ByteArray(0)
    }
    return requireNotNull(view.data).getByteArray(0, length.toInt())
}

internal fun copyString(view: NativeStringView): String {
    val length = view.length.toLong()
    if (length == 0L) {
        return ""
    }
    return String(
        requireNotNull(view.data).getByteArray(0, length.toInt()),
        StandardCharsets.UTF_8,
    )
}

internal class NativeArena : AutoCloseable {
    private val allocations = mutableListOf<Memory>()
    private val structures = mutableListOf<Structure>()

    fun bytes(value: ByteArray): NativeByteView.ByValue {
        val result = NativeByteView.ByValue()
        if (value.isNotEmpty()) {
            val memory = Memory(value.size.toLong())
            memory.write(0, value, 0, value.size)
            allocations += memory
            result.data = memory
            result.length = SizeT(value.size.toLong())
        }
        result.write()
        return result
    }

    fun bytesReference(value: ByteArray): NativeByteView.ByReference {
        val view = bytes(value)
        return NativeByteView.ByReference().also {
            it.data = view.data
            it.length = view.length
            it.write()
            structures += it
        }
    }

    fun string(value: String): NativeStringView.ByValue {
        val bytes = value.toByteArray(StandardCharsets.UTF_8)
        val result = NativeStringView.ByValue()
        if (bytes.isNotEmpty()) {
            val memory = Memory(bytes.size.toLong())
            memory.write(0, bytes, 0, bytes.size)
            allocations += memory
            result.data = memory
            result.length = SizeT(bytes.size.toLong())
        }
        result.write()
        return result
    }

    fun identity(value: IdentityConfig): NativeIdentityConfig.ByValue {
        val result = NativeIdentityConfig.ByValue()
        result.structSize = SizeT(result.size().toLong())
        when (value) {
            is IdentityConfigExisting -> {
                val secret = value.secret.copyBytes()
                try {
                    result.kind = IdentityConfigKind.EXISTING.rawValue
                    result.secret = bytes(secret)
                } finally {
                    secret.fill(0)
                }
            }
            IdentityConfigGenerateEphemeral -> {
                result.kind = IdentityConfigKind.GENERATE_EPHEMERAL.rawValue
            }
            is IdentityConfigLoadOrCreate -> {
                result.kind = IdentityConfigKind.LOAD_OR_CREATE.rawValue
                result.path = string(value.path)
            }
        }
        result.write()
        return result
    }

    private fun destinationName(value: DestinationName): NativeDestinationName.ByValue {
        val result = NativeDestinationName.ByValue()
        result.structSize = SizeT(result.size().toLong())
        result.appName = string(value.appName)
        if (value.aspects.isNotEmpty()) {
            val first = NativeStringView()
            val array = first.toArray(value.aspects.size)
            value.aspects.forEachIndexed { index, aspect ->
                val item = array[index] as NativeStringView
                val native = string(aspect)
                item.data = native.data
                item.length = native.length
                item.write()
                structures += item
            }
            result.aspects = first.pointer
            result.aspectCount = SizeT(value.aspects.size.toLong())
            structures += first
        }
        result.write()
        return result
    }

    private fun destination(value: DestinationConfig): NativeDestinationConfig {
        val result = NativeDestinationConfig()
        result.structSize = SizeT(result.size().toLong())
        when (value) {
            is DestinationConfigPlain -> {
                result.kind = DestinationConfigKind.PLAIN.rawValue
                result.name = destinationName(value.name)
            }
            is DestinationConfigSingle -> {
                result.kind = DestinationConfigKind.SINGLE.rawValue
                result.name = destinationName(value.name)
                when (val identity = value.identity) {
                    DestinationIdentityConfigHostIdentity -> {
                        result.identityKind =
                            DestinationIdentityConfigKind.HOST_IDENTITY.rawValue
                    }
                    is DestinationIdentityConfigDedicatedIdentity -> {
                        result.identityKind =
                            DestinationIdentityConfigKind.DEDICATED_IDENTITY.rawValue
                        result.dedicatedIdentity = identity(identity.identity)
                    }
                }
                value.announceAppData?.let {
                    result.announceAppData = bytes(it.copyBytes())
                }
            }
        }
        result.write()
        return result
    }

    fun hostOptions(value: HostOptions): NativeHostOptions {
        val result = NativeHostOptions()
        result.structSize = SizeT(result.size().toLong())
        result.requiredAbi = HostContract.ABI
        result.requiredProductVersion = string(HostContract.PRODUCT_VERSION)
        result.limits = NativeLimits.ByValue().also {
            it.structSize = SizeT(it.size().toLong())
            it.pendingCommands = SizeT(value.limits.pendingCommands)
            it.applicationEvents = SizeT(value.limits.applicationEvents)
            it.retainedEventBytes = SizeT(value.limits.retainedEventBytes)
            it.diagnostics = SizeT(value.limits.diagnostics)
            it.write()
        }
        result.role = value.role.rawValue
        result.identity = identity(value.identity)
        if (value.destinations.isNotEmpty()) {
            val first = NativeDestinationConfig()
            val array = first.toArray(value.destinations.size)
            value.destinations.forEachIndexed { index, destination ->
                val target = array[index] as NativeDestinationConfig
                val native = destination(destination)
                target.structSize = native.structSize
                target.kind = native.kind
                target.name = native.name
                target.identityKind = native.identityKind
                target.dedicatedIdentity = native.dedicatedIdentity
                target.announceAppData = native.announceAppData
                target.write()
                structures += target
            }
            result.destinations = first.pointer
            result.destinationCount = SizeT(value.destinations.size.toLong())
            structures += first
        }
        if (value.requiredCapabilities.isNotEmpty()) {
            val capabilities = value.requiredCapabilities.sortedBy(Capability::rawValue)
            val memory = Memory(Int.SIZE_BYTES.toLong() * capabilities.size)
            capabilities.forEachIndexed { index, capability ->
                memory.setInt(Int.SIZE_BYTES.toLong() * index, capability.rawValue)
            }
            allocations += memory
            result.requiredCapabilities = memory
            result.requiredCapabilityCount = SizeT(capabilities.size.toLong())
        }
        result.write()
        return result
    }

    override fun close() {
        allocations.asReversed().forEach {
            it.clear()
            it.close()
        }
        structures.clear()
        allocations.clear()
    }
}

internal fun verifyNativeContract() {
    val info = NativeContractInfo()
    info.structSize = SizeT(info.size().toLong())
    info.write()
    checkedStatus(NativeApi.library.prns_contract_info(info), "contractInfo")
    info.read()
    if (
        info.abi != HostContract.ABI ||
        info.schemaVersion != HostContract.SCHEMA_VERSION ||
        copyString(info.productVersion) != HostContract.PRODUCT_VERSION
    ) {
        throw StatusException("contractInfo", Status.CONTRACT_MISMATCH)
    }
}
