package rs.reticulum.prns

import com.sun.jna.Pointer
import com.sun.jna.ptr.LongByReference
import com.sun.jna.ptr.PointerByReference
import java.util.concurrent.CompletionStage
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

    val backendInfo: BackendInfo
        get() = withPointer {
            val value = NativeBackendInfo()
            value.structSize = SizeT(value.size().toLong())
            value.write()
            checkedStatus(NativeApi.library.prns_backend_info(value), "backendInfo")
            value.read()
            value.decode()
        }

    init {
        verifyNativeContract()
        val nativePointer =
                NativeArena().use { arena ->
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
                        pointer ?: throw StatusException("host", Status.STOPPED),
                )
            }

    fun execute(command: HostCommand): Command = withPointer { host ->
        NativeArena().use { arena ->
            val output = PointerByReference()
            val status =
                    when (command) {
                        is HostCommandAnnounce -> {
                            val destination = arena.bytes(command.destination.copyBytes())
                            val interfaceId =
                                    command.`interface`?.let {
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
                        is HostCommandAttachInterface -> {
                            NativeApi.library.prns_host_attach_interface(
                                    host,
                                    arena.interfaceConfig(command.config),
                                    interfaceRouting(command.routing),
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
                            val maximumResponseBytes =
                                    command.maximumResponseBytes?.let {
                                        require(it in 0..HostContract.SAFE_UINT_MAX) {
                                            "maximumResponseBytes must be an unsigned safe integer"
                                        }
                                        LongByReference(it)
                                    }
                            NativeApi.library.prns_host_request(
                                    host,
                                    arena.bytes(command.linkId.copyBytes()),
                                    arena.bytes(command.pathHash.copyBytes()),
                                    arena.bytes(command.payload.copyBytes()),
                                    timeout.first,
                                    timeout.second,
                                    maximumResponseBytes,
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
                            val metadata =
                                    command.packedMetadata?.let {
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

    fun executeAsync(command: HostCommand): CompletionStage<CommandSettlement> = javaFuture {
        settle(command)
    }

    private suspend fun settle(command: HostCommand): CommandSettlement =
            execute(command).use { it.await() }

    suspend fun announce(
            destination: DestinationHash,
            interfaceId: InterfaceId? = null,
    ): CommandSettlement =
            settle(
                    HostCommandAnnounce(destination = destination, `interface` = interfaceId),
            )

    fun announceAsync(
            destination: DestinationHash,
            interfaceId: InterfaceId?,
    ): CompletionStage<CommandSettlement> = javaFuture { announce(destination, interfaceId) }

    suspend fun sendSinglePacket(
            destination: DestinationHash,
            payload: Bytes,
    ): CommandSettlement =
            settle(
                    HostCommandSendSinglePacket(destination = destination, payload = payload),
            )

    fun sendSinglePacketAsync(
            destination: DestinationHash,
            payload: Bytes,
    ): CompletionStage<CommandSettlement> = javaFuture { sendSinglePacket(destination, payload) }

    suspend fun closeLink(linkId: LinkId): CommandSettlement =
            settle(HostCommandCloseLink(linkId = linkId))

    fun closeLinkAsync(linkId: LinkId): CompletionStage<CommandSettlement> = javaFuture {
        closeLink(linkId)
    }

    suspend fun attachTcpServer(
            bind: String,
            bitrate: Bitrate,
    ): CommandSettlement =
            settle(
                    HostCommandAttachTcpServer(bind = bind, bitrate = bitrate),
            )

    fun attachTcpServerAsync(
            bind: String,
            bitrate: Bitrate,
    ): CompletionStage<CommandSettlement> = javaFuture { attachTcpServer(bind, bitrate) }

    suspend fun attachTcpClient(
            target: String,
            bitrate: Bitrate,
    ): CommandSettlement =
            settle(
                    HostCommandAttachTcpClient(target = target, bitrate = bitrate),
            )

    fun attachTcpClientAsync(
            target: String,
            bitrate: Bitrate,
    ): CompletionStage<CommandSettlement> = javaFuture { attachTcpClient(target, bitrate) }

    suspend fun attachUdp(
            local: String,
            peer: String,
            bitrate: Bitrate,
    ): CommandSettlement =
            settle(
                    HostCommandAttachUdp(local = local, peer = peer, bitrate = bitrate),
            )

    fun attachUdpAsync(
            local: String,
            peer: String,
            bitrate: Bitrate,
    ): CompletionStage<CommandSettlement> = javaFuture { attachUdp(local, peer, bitrate) }

    /**
     * Begins an ordinary Pipe whose connected descriptors are supplied through the returned
     * controller. Android VPN applications can open and protect a socket inside
     * [SuppliedPipe.serve] before detaching its descriptor.
     */
    fun beginSuppliedPipe(
            name: String,
            respawnDelayMillis: Long,
            bitrate: Bitrate,
    ): SuppliedPipe = withPointer { host ->
        require(name.isNotEmpty()) { "a supplied pipe needs a non-empty name" }
        require(respawnDelayMillis in 0..HostContract.SAFE_UINT_MAX) {
            "respawnDelayMillis is outside the host contract range"
        }
        NativeArena().use { arena ->
            val output = PointerByReference()
            val nativeBitrate = bitrate.native()
            checkedStatus(
                    NativeApi.library.prns_host_attach_supplied_pipe(
                            host,
                            arena.string(name),
                            respawnDelayMillis,
                            nativeBitrate.first,
                            nativeBitrate.second,
                            output,
                    ),
                    "beginSuppliedPipe",
            )
            SuppliedPipe(requireNotNull(output.value))
        }
    }

    suspend fun attachInterface(
            config: InterfaceConfig,
            routing: InterfaceRoutingPolicy? = null,
    ): CommandSettlement =
            settle(
                    HostCommandAttachInterface(config = config, routing = routing),
            )

    fun attachInterfaceAsync(
            config: InterfaceConfig,
    ): CompletionStage<CommandSettlement> = javaFuture { attachInterface(config, null) }

    fun attachInterfaceAsync(
            config: InterfaceConfig,
            routing: InterfaceRoutingPolicy,
    ): CompletionStage<CommandSettlement> = javaFuture { attachInterface(config, routing) }

    suspend fun detachInterface(interfaceId: InterfaceId): CommandSettlement =
            settle(HostCommandDetachInterface(`interface` = interfaceId))

    fun detachInterfaceAsync(
            interfaceId: InterfaceId,
    ): CompletionStage<CommandSettlement> = javaFuture { detachInterface(interfaceId) }

    suspend fun establishLink(destination: DestinationHash): CommandSettlement =
            settle(HostCommandEstablishLink(destination = destination))

    fun establishLinkAsync(
            destination: DestinationHash,
    ): CompletionStage<CommandSettlement> = javaFuture { establishLink(destination) }

    suspend fun requestPath(destination: DestinationHash): CommandSettlement =
            settle(HostCommandRequestPath(destination = destination))

    fun requestPathAsync(
            destination: DestinationHash,
    ): CompletionStage<CommandSettlement> = javaFuture { requestPath(destination) }

    suspend fun identify(
            linkId: LinkId,
            identity: IdentityHash,
    ): CommandSettlement =
            settle(
                    HostCommandIdentify(linkId = linkId, identity = identity),
            )

    fun identifyAsync(
            linkId: LinkId,
            identity: IdentityHash,
    ): CompletionStage<CommandSettlement> = javaFuture { identify(linkId, identity) }

    suspend fun sendLinkPacket(
            linkId: LinkId,
            payload: Bytes,
    ): CommandSettlement =
            settle(
                    HostCommandSendLinkPacket(linkId = linkId, payload = payload),
            )

    fun sendLinkPacketAsync(
            linkId: LinkId,
            payload: Bytes,
    ): CompletionStage<CommandSettlement> = javaFuture { sendLinkPacket(linkId, payload) }

    suspend fun request(
            linkId: LinkId,
            pathHash: RequestPathHash,
            payload: Bytes,
            timeout: ResponseTimeout,
            maximumResponseBytes: Long? = null,
    ): CommandSettlement =
            settle(
                    HostCommandRequest(
                            linkId = linkId,
                            pathHash = pathHash,
                            payload = payload,
                            timeout = timeout,
                            maximumResponseBytes = maximumResponseBytes,
                    ),
            )

    fun requestAsync(
            linkId: LinkId,
            pathHash: RequestPathHash,
            payload: Bytes,
            timeout: ResponseTimeout,
    ): CompletionStage<CommandSettlement> = requestAsync(linkId, pathHash, payload, timeout, null)

    fun requestAsync(
            linkId: LinkId,
            pathHash: RequestPathHash,
            payload: Bytes,
            timeout: ResponseTimeout,
            maximumResponseBytes: Long?,
    ): CompletionStage<CommandSettlement> = javaFuture {
        request(linkId, pathHash, payload, timeout, maximumResponseBytes)
    }

    suspend fun respond(
            linkId: LinkId,
            requestId: RequestId,
            requestRttMillis: Long,
            payload: Bytes,
    ): CommandSettlement =
            settle(
                    HostCommandRespond(
                            linkId = linkId,
                            requestId = requestId,
                            requestRttMillis = requestRttMillis,
                            payload = payload,
                    ),
            )

    fun respondAsync(
            linkId: LinkId,
            requestId: RequestId,
            requestRttMillis: Long,
            payload: Bytes,
    ): CompletionStage<CommandSettlement> = javaFuture {
        respond(linkId, requestId, requestRttMillis, payload)
    }

    suspend fun setLinkResourceStrategy(
            linkId: LinkId,
            strategy: ResourceStrategy,
    ): CommandSettlement =
            settle(
                    HostCommandSetLinkResourceStrategy(linkId = linkId, strategy = strategy),
            )

    fun setLinkResourceStrategyAsync(
            linkId: LinkId,
            strategy: ResourceStrategy,
    ): CompletionStage<CommandSettlement> = javaFuture { setLinkResourceStrategy(linkId, strategy) }

    suspend fun setDestinationResourceStrategy(
            destination: DestinationHash,
            strategy: ResourceStrategy,
    ): CommandSettlement =
            settle(
                    HostCommandSetDestinationResourceStrategy(
                            destination = destination,
                            strategy = strategy,
                    ),
            )

    fun setDestinationResourceStrategyAsync(
            destination: DestinationHash,
            strategy: ResourceStrategy,
    ): CompletionStage<CommandSettlement> = javaFuture {
        setDestinationResourceStrategy(destination, strategy)
    }

    suspend fun sendChannelMessage(
            linkId: LinkId,
            messageType: Int,
            payload: Bytes,
    ): CommandSettlement =
            settle(
                    HostCommandSendChannelMessage(
                            linkId = linkId,
                            messageType = messageType,
                            payload = payload,
                    ),
            )

    fun sendChannelMessageAsync(
            linkId: LinkId,
            messageType: Int,
            payload: Bytes,
    ): CompletionStage<CommandSettlement> = javaFuture {
        sendChannelMessage(linkId, messageType, payload)
    }

    suspend fun allowRequester(
            destination: DestinationHash,
            pathHash: RequestPathHash,
            identity: IdentityHash,
    ): CommandSettlement =
            settle(
                    HostCommandAllowRequester(
                            destination = destination,
                            pathHash = pathHash,
                            identity = identity,
                    ),
            )

    fun allowRequesterAsync(
            destination: DestinationHash,
            pathHash: RequestPathHash,
            identity: IdentityHash,
    ): CompletionStage<CommandSettlement> = javaFuture {
        allowRequester(destination, pathHash, identity)
    }

    fun snapshot(timeoutMillis: Long = 5_000): HostSnapshot = withPointer { host ->
        require(timeoutMillis in 0..0xffff_ffffL) { "timeoutMillis must fit in 32 bits" }
        val inspection = PointerByReference()
        checkedStatus(
                NativeApi.library.prns_host_snapshot(host, timeoutMillis.toInt(), inspection),
                "captureSnapshot",
        )
        val pointer = requireNotNull(inspection.value)
        try {
            val value = NativeHostSnapshot()
            value.structSize = SizeT(value.size().toLong())
            value.write()
            checkedStatus(
                    NativeApi.library.prns_host_snapshot_read(pointer, value),
                    "readSnapshot",
            )
            value.read()
            value.decode()
        } finally {
            NativeApi.library.prns_host_snapshot_release(pointer)
        }
    }

    fun beginResourceUpload(
            linkId: LinkId,
            declaredLength: Long,
            packedMetadata: Bytes?,
            compression: ResourceCompression,
    ): ResourceUpload = withPointer { host ->
        require(declaredLength >= 0) { "declaredLength must be non-negative" }
        NativeArena().use { arena ->
            val output = PointerByReference()
            val metadata = packedMetadata?.let { arena.bytesReference(it.copyBytes()) }
            checkedStatus(
                    NativeApi.library.prns_host_begin_resource_upload(
                            host,
                            arena.bytes(linkId.copyBytes()),
                            declaredLength,
                            metadata,
                            compression.native(),
                            output,
                    ),
                    "beginResourceUpload",
            )
            ResourceUpload(requireNotNull(output.value))
        }
    }

    suspend fun sendResource(
            linkId: LinkId,
            payload: Bytes,
            packedMetadata: Bytes?,
            compression: ResourceCompression,
    ): CommandSettlement {
        val upload =
                beginResourceUpload(
                        linkId,
                        payload.size.toLong(),
                        packedMetadata,
                        compression,
                )
        return try {
            upload.write(payload)
            upload.finish()
        } catch (failure: Throwable) {
            upload.abort()
            upload.close()
            throw failure
        }
    }

    fun sendResourceAsync(
            linkId: LinkId,
            payload: Bytes,
            packedMetadata: Bytes?,
            compression: ResourceCompression,
    ): CompletionStage<CommandSettlement> = javaFuture {
        sendResource(linkId, payload, packedMetadata, compression)
    }

    fun claimApplicationEvents(): StreamClaim<EventFlow<ApplicationEvent>> =
            claimEvents("claimApplicationEvents") { host, output ->
                NativeApi.library.prns_host_claim_application_events(host, output)
            }
                    .map { pointer -> EventFlow(pointer, ::decodeApplicationEvent) }

    fun claimDiagnostics(): StreamClaim<EventFlow<DiagnosticEvent>> =
            claimEvents("claimDiagnostics") { host, output ->
                NativeApi.library.prns_host_claim_diagnostics(host, output)
            }
                    .map { pointer -> EventFlow(pointer, ::decodeDiagnosticEvent) }

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
            val status =
                    Status.fromRawValue(NativeApi.library.prns_host_stop(host))
                            ?: Status.BACKEND_FAILED
            if (status != Status.OK && status != Status.STOPPED) {
                throw StatusException("stopHost", status)
            }
        }
    }

    override fun close() {
        val nativePointer =
                stateLock.withLock {
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

internal fun Bitrate.native(): Pair<Int, Long> =
        when (this) {
            BitrateAuto -> BitrateKind.AUTO.rawValue to 0L
            is BitrateBitsPerSecond -> BitrateKind.BITS_PER_SECOND.rawValue to value
        }

private fun ResponseTimeout.native(): Pair<Int, Long> =
        when (this) {
            ResponseTimeoutLinkDefault -> ResponseTimeoutKind.LINK_DEFAULT.rawValue to 0L
            is ResponseTimeoutExact -> ResponseTimeoutKind.EXACT.rawValue to millis
        }

internal fun ResourceCompression.native(): Int =
        when (this) {
            ResourceCompressionAuto -> ResourceCompressionKind.AUTO.rawValue
            ResourceCompressionNever -> ResourceCompressionKind.NEVER.rawValue
        }

private data class NativeResourceStrategy(
        val kind: Int,
        val maximumUncompressedBytes: Long,
        val acceptCompressed: Byte,
)

private fun ResourceStrategy.native(): NativeResourceStrategy =
        when (this) {
            ResourceStrategyRefuse ->
                    NativeResourceStrategy(
                            ResourceStrategyKind.REFUSE.rawValue,
                            0L,
                            0,
                    )
            is ResourceStrategyAccept ->
                    NativeResourceStrategy(
                            ResourceStrategyKind.ACCEPT.rawValue,
                            maximumUncompressedBytes,
                            if (acceptCompressed) 1 else 0,
                    )
        }

private fun <Input, Output> StreamClaim<Input>.map(
        transform: (Input) -> Output,
): StreamClaim<Output> =
        when (this) {
            is StreamClaimed -> StreamClaimed(transform(stream))
            StreamAlreadyClaimed -> StreamAlreadyClaimed
        }
