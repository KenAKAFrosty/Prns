package io.reticulum.prns

import com.sun.jna.Pointer
import com.sun.jna.ptr.PointerByReference
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

sealed interface StreamClaim<out Stream>

data class StreamClaimed<out Stream>(val stream: Stream) : StreamClaim<Stream>

data object StreamAlreadyClaimed : StreamClaim<Nothing>

class Host(options: HostOptions) : AutoCloseable {
    private val stateLock = ReentrantLock()
    private var pointer: Pointer?
    val identityHash: IdentityHash
    val destinationHashes: List<DestinationHash>

    init {
        verifyNativeContract()
        val nativePointer = NativeArena().use { arena ->
            val nativeOptions = arena.hostOptions(options)
            val output = PointerByReference()
            checkedStatus(
                NativeApi.library.prns_host_create(nativeOptions, output),
                "createHost",
            )
            requireNotNull(output.value)
        }
        try {
            identityHash = readIdentityHash(nativePointer)
            destinationHashes = readDestinationHashes(nativePointer)
            pointer = nativePointer
        } catch (failure: Throwable) {
            NativeApi.library.prns_host_release(nativePointer)
            throw failure
        }
    }

    private fun <Value> withPointer(block: (Pointer) -> Value): Value =
        stateLock.withLock {
            block(
                pointer
                    ?: throw StatusException("host", Status.STOPPED),
            )
        }

    fun execute(command: HostCommand): Command = withPointer { host ->
        NativeArena().use { arena ->
            val output = PointerByReference()
            val status = when (command) {
                is HostCommandAnnounce -> {
                    val destination = arena.bytes(command.destination.copyBytes())
                    val interfaceId = command.`interface`?.let {
                        arena.bytesReference(it.copyBytes())
                    }
                    NativeApi.library.prns_host_announce(
                        host,
                        destination,
                        interfaceId,
                        output,
                    )
                }
                is HostCommandSendSinglePacket -> {
                    NativeApi.library.prns_host_send_single_packet(
                        host,
                        arena.bytes(command.destination.copyBytes()),
                        arena.bytes(command.payload.copyBytes()),
                        output,
                    )
                }
                is HostCommandCloseLink -> {
                    NativeApi.library.prns_host_close_link(
                        host,
                        arena.bytes(command.linkId.copyBytes()),
                        output,
                    )
                }
                is HostCommandAttachTcpServer -> {
                    val bitrate = command.bitrate.native()
                    NativeApi.library.prns_host_attach_tcp_server(
                        host,
                        arena.string(command.bind),
                        bitrate.first,
                        bitrate.second,
                        output,
                    )
                }
                is HostCommandAttachTcpClient -> {
                    val bitrate = command.bitrate.native()
                    NativeApi.library.prns_host_attach_tcp_client(
                        host,
                        arena.string(command.target),
                        bitrate.first,
                        bitrate.second,
                        output,
                    )
                }
                is HostCommandAttachUdp -> {
                    val bitrate = command.bitrate.native()
                    NativeApi.library.prns_host_attach_udp(
                        host,
                        arena.string(command.local),
                        arena.string(command.peer),
                        bitrate.first,
                        bitrate.second,
                        output,
                    )
                }
                is HostCommandDetachInterface -> {
                    NativeApi.library.prns_host_detach_interface(
                        host,
                        arena.bytes(command.`interface`.copyBytes()),
                        output,
                    )
                }
                is HostCommandEstablishLink -> {
                    NativeApi.library.prns_host_establish_link(
                        host,
                        arena.bytes(command.destination.copyBytes()),
                        output,
                    )
                }
                is HostCommandRequestPath -> {
                    NativeApi.library.prns_host_request_path(
                        host,
                        arena.bytes(command.destination.copyBytes()),
                        output,
                    )
                }
                is HostCommandIdentify -> {
                    NativeApi.library.prns_host_identify(
                        host,
                        arena.bytes(command.linkId.copyBytes()),
                        arena.bytes(command.identity.copyBytes()),
                        output,
                    )
                }
                is HostCommandSendLinkPacket -> {
                    NativeApi.library.prns_host_send_link_packet(
                        host,
                        arena.bytes(command.linkId.copyBytes()),
                        arena.bytes(command.payload.copyBytes()),
                        output,
                    )
                }
                is HostCommandRequest -> {
                    val timeout = command.timeout.native()
                    NativeApi.library.prns_host_request(
                        host,
                        arena.bytes(command.linkId.copyBytes()),
                        arena.bytes(command.pathHash.copyBytes()),
                        arena.bytes(command.payload.copyBytes()),
                        timeout.first,
                        timeout.second,
                        output,
                    )
                }
                is HostCommandRespond -> {
                    NativeApi.library.prns_host_respond(
                        host,
                        arena.bytes(command.linkId.copyBytes()),
                        arena.bytes(command.requestId.copyBytes()),
                        command.requestRttMillis,
                        arena.bytes(command.payload.copyBytes()),
                        output,
                    )
                }
                is HostCommandSendResource -> {
                    val metadata = command.packedMetadata?.let {
                        arena.bytesReference(it.copyBytes())
                    }
                    NativeApi.library.prns_host_send_resource(
                        host,
                        arena.bytes(command.linkId.copyBytes()),
                        arena.bytes(command.payload.copyBytes()),
                        metadata,
                        command.compression.native(),
                        output,
                    )
                }
                is HostCommandSetLinkResourceStrategy -> {
                    val strategy = command.strategy.native()
                    NativeApi.library.prns_host_set_link_resource_strategy(
                        host,
                        arena.bytes(command.linkId.copyBytes()),
                        strategy.kind,
                        strategy.maximumUncompressedBytes,
                        strategy.acceptCompressed,
                        output,
                    )
                }
                is HostCommandSetDestinationResourceStrategy -> {
                    val strategy = command.strategy.native()
                    NativeApi.library.prns_host_set_destination_resource_strategy(
                        host,
                        arena.bytes(command.destination.copyBytes()),
                        strategy.kind,
                        strategy.maximumUncompressedBytes,
                        strategy.acceptCompressed,
                        output,
                    )
                }
                is HostCommandSendChannelMessage -> {
                    require(command.messageType in 0..0xffff) {
                        "messageType must fit in 16 bits"
                    }
                    NativeApi.library.prns_host_send_channel_message(
                        host,
                        arena.bytes(command.linkId.copyBytes()),
                        command.messageType.toShort(),
                        arena.bytes(command.payload.copyBytes()),
                        output,
                    )
                }
                is HostCommandAllowRequester -> {
                    NativeApi.library.prns_host_allow_requester(
                        host,
                        arena.bytes(command.destination.copyBytes()),
                        arena.bytes(command.pathHash.copyBytes()),
                        arena.bytes(command.identity.copyBytes()),
                        output,
                    )
                }
            }
            checkedStatus(status, "executeCommand")
            Command(requireNotNull(output.value))
        }
    }

    fun claimApplicationEvents(): StreamClaim<EventFlow<ApplicationEvent>> =
        claimEvents("claimApplicationEvents") { host, output ->
            NativeApi.library.prns_host_claim_application_events(host, output)
        }.map { pointer ->
            EventFlow(pointer, ::decodeApplicationEvent)
        }

    fun claimDiagnostics(): StreamClaim<EventFlow<DiagnosticEvent>> =
        claimEvents("claimDiagnostics") { host, output ->
            NativeApi.library.prns_host_claim_diagnostics(host, output)
        }.map { pointer ->
            EventFlow(pointer, ::decodeDiagnosticEvent)
        }

    private fun claimEvents(
        operation: String,
        claim: (Pointer, PointerByReference) -> Int,
    ): StreamClaim<Pointer> = withPointer { host ->
        val output = PointerByReference()
        val status = Status.fromRawValue(claim(host, output)) ?: Status.BACKEND_FAILED
        when (status) {
            Status.OK -> StreamClaimed(requireNotNull(output.value))
            Status.ALREADY_CLAIMED -> StreamAlreadyClaimed
            else -> throw StatusException(operation, status)
        }
    }

    fun stop() {
        withPointer { host ->
            val status = Status.fromRawValue(NativeApi.library.prns_host_stop(host))
                ?: Status.BACKEND_FAILED
            if (status != Status.OK && status != Status.STOPPED) {
                throw StatusException("stopHost", status)
            }
        }
    }

    override fun close() {
        val nativePointer = stateLock.withLock {
            val current = pointer
            pointer = null
            current
        }
        nativePointer?.let(NativeApi.library::prns_host_release)
    }

    private fun readIdentityHash(host: Pointer): IdentityHash {
        val view = NativeByteView()
        checkedStatus(
            NativeApi.library.prns_host_identity_hash(host, view),
            "identityHash",
        )
        view.read()
        return IdentityHash(copyBytes(view))
    }

    private fun readDestinationHashes(host: Pointer): List<DestinationHash> {
        val count = NativeApi.library.prns_host_destination_count(host).toLong()
        return (0L until count).map { index ->
            val view = NativeByteView()
            checkedStatus(
                NativeApi.library.prns_host_destination_hash(
                    host,
                    SizeT(index),
                    view,
                ),
                "destinationHash",
            )
            view.read()
            DestinationHash(copyBytes(view))
        }
    }
}

private fun Bitrate.native(): Pair<Int, Long> = when (this) {
    BitrateAuto -> BitrateKind.AUTO.rawValue to 0L
    is BitrateBitsPerSecond -> BitrateKind.BITS_PER_SECOND.rawValue to value
}

private fun ResponseTimeout.native(): Pair<Int, Long> = when (this) {
    ResponseTimeoutLinkDefault -> ResponseTimeoutKind.LINK_DEFAULT.rawValue to 0L
    is ResponseTimeoutExact -> ResponseTimeoutKind.EXACT.rawValue to millis
}

private fun ResourceCompression.native(): Int = when (this) {
    ResourceCompressionAuto -> ResourceCompressionKind.AUTO.rawValue
    ResourceCompressionNever -> ResourceCompressionKind.NEVER.rawValue
}

private data class NativeResourceStrategy(
    val kind: Int,
    val maximumUncompressedBytes: Long,
    val acceptCompressed: Byte,
)

private fun ResourceStrategy.native(): NativeResourceStrategy = when (this) {
    ResourceStrategyRefuse -> NativeResourceStrategy(
        ResourceStrategyKind.REFUSE.rawValue,
        0L,
        0,
    )
    is ResourceStrategyAccept -> NativeResourceStrategy(
        ResourceStrategyKind.ACCEPT.rawValue,
        maximumUncompressedBytes,
        if (acceptCompressed) 1 else 0,
    )
}

private fun <Input, Output> StreamClaim<Input>.map(
    transform: (Input) -> Output,
): StreamClaim<Output> = when (this) {
    is StreamClaimed -> StreamClaimed(transform(stream))
    StreamAlreadyClaimed -> StreamAlreadyClaimed
}
