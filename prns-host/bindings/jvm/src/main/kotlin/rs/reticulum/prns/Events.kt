package rs.reticulum.prns

import com.sun.jna.Pointer
import com.sun.jna.ptr.ByteByReference
import com.sun.jna.ptr.LongByReference
import com.sun.jna.ptr.PointerByReference
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.FlowCollector
import kotlinx.coroutines.flow.flow
import java.math.BigInteger
import java.util.concurrent.CompletionStage
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

class EventFlow<Event : Any> internal constructor(
    pointer: Pointer,
    private val decode: (Pointer) -> Event,
) : Flow<Event>, AutoCloseable {
    private val stateLock = ReentrantLock()
    private val waitLock = ReentrantLock()
    private val consumerMode = AtomicInteger(UNCLAIMED)
    private val readiness: NativeReadiness
    private var pointer: Pointer? = pointer

    init {
        try {
            readiness = NativeReadiness.eventStream(pointer)
        } catch (failure: Throwable) {
            NativeApi.library.prns_event_stream_release(pointer)
            throw failure
        }
    }

    override suspend fun collect(collector: FlowCollector<Event>) {
        if (!consumerMode.compareAndSet(UNCLAIMED, FLOW_CONSUMER)) {
            throw StatusException("collectEvents", Status.ALREADY_CLAIMED)
        }
        try {
            while (true) {
                val event = nextEvent() ?: break
                val value = try {
                    decode(event)
                } finally {
                    NativeApi.library.prns_event_release(event)
                }
                collector.emit(value)
            }
        } finally {
            close()
        }
    }

    suspend fun next(): Event? {
        if (!consumerMode.compareAndSet(UNCLAIMED, ASYNC_CONSUMER) &&
            consumerMode.get() != ASYNC_CONSUMER
        ) {
            throw StatusException("nextEvent", Status.ALREADY_CLAIMED)
        }
        val event = nextEvent()
        if (event == null) {
            close()
            return null
        }
        return try {
            decode(event)
        } finally {
            NativeApi.library.prns_event_release(event)
        }
    }

    fun nextAsync(): CompletionStage<Event?> = javaFuture { next() }

    private suspend fun nextEvent(): Pointer? {
        try {
            while (true) {
                currentCoroutineContext().ensureActive()
                val (status, event) = pollEvent()
                when (status) {
                    Status.OK -> return requireNotNull(event)
                    Status.WOULD_BLOCK -> readiness.await()
                    Status.STOPPED -> return null
                    else -> {
                        event?.let(NativeApi.library::prns_event_release)
                        throw StatusException("nextEvent", status)
                    }
                }
            }
        } catch (failure: CancellationException) {
            stateLock.withLock {
                pointer?.let(NativeApi.library::prns_event_stream_interrupt_wait)
            }
            throw failure
        }
    }

    private fun pollEvent(): Pair<Status, Pointer?> = waitLock.withLock {
        val stream = stateLock.withLock { pointer }
            ?: return@withLock Status.STOPPED to null
        val output = PointerByReference()
        val status = Status.fromRawValue(
            NativeApi.library.prns_event_stream_next(
                stream,
                0,
                output,
            ),
        ) ?: Status.BACKEND_FAILED
        status to output.value
    }

    override fun close() {
        val stream = stateLock.withLock {
            val current = pointer
            pointer = null
            current?.let(NativeApi.library::prns_event_stream_interrupt_wait)
            current
        }
        if (stream != null) {
            waitLock.withLock {
                readiness.close()
                NativeApi.library.prns_event_stream_release(stream)
            }
        }
    }

    private companion object {
        const val UNCLAIMED = 0
        const val FLOW_CONSUMER = 1
        const val ASYNC_CONSUMER = 2
    }
}

fun ResourceStream.chunks(maximumBytes: Int = 64 * 1024): Flow<Bytes> = flow {
    while (true) {
        val chunk = next(maximumBytes)
        if (chunk.finished) {
            break
        }
        emit(chunk.bytes)
    }
}

private class NativeResourceStream(
    pointer: Pointer,
    override val totalBytes: ULong,
) : ResourceStream {
    private val lock = ReentrantLock()
    private var pointer: Pointer? = pointer

    override fun next(maximumBytes: Int): ResourceChunk {
        require(maximumBytes > 0)
        return lock.withLock {
            val stream = pointer
                ?: throw StatusException("resourceStream", Status.STOPPED)
            val chunk = NativeByteView()
            val finished = ByteByReference()
            checkedStatus(
                NativeApi.library.prns_resource_stream_next(
                    stream,
                    SizeT(maximumBytes.toLong()),
                    chunk,
                    finished,
                ),
                "nextResourceChunk",
            )
            chunk.read()
            ResourceChunk(
                bytes = Bytes(copyBytes(chunk)),
                finished = finished.value.toInt() != 0,
            )
        }
    }

    override fun close() {
        lock.withLock {
            pointer?.let(NativeApi.library::prns_resource_stream_release)
            pointer = null
        }
    }
}

private class EventReader(private val pointer: Pointer) {
    val applicationKind: ApplicationEventKind
        get() = ApplicationEventKind.fromRawValue(
            NativeApi.library.prns_event_kind(pointer),
        ) ?: throw StatusException("applicationEventKind", Status.BACKEND_FAILED)

    val diagnosticKind: DiagnosticEventKind
        get() = DiagnosticEventKind.fromRawValue(
            NativeApi.library.prns_event_kind(pointer),
        ) ?: throw StatusException("diagnosticEventKind", Status.BACKEND_FAILED)

    fun bytes(field: EventField): ByteArray {
        val value = NativeByteView()
        checkedStatus(
            NativeApi.library.prns_event_bytes(pointer, field.rawValue, value),
            "eventBytes",
        )
        value.read()
        return copyBytes(value)
    }

    fun optionalBytes(field: EventField): ByteArray? {
        val value = NativeByteView()
        val status = Status.fromRawValue(
            NativeApi.library.prns_event_bytes(pointer, field.rawValue, value),
        ) ?: Status.BACKEND_FAILED
        return when (status) {
            Status.OK -> {
                value.read()
                copyBytes(value)
            }
            Status.INVALID_ARGUMENT -> null
            else -> throw StatusException("eventBytes", status)
        }
    }

    fun string(field: EventField): String {
        val value = NativeStringView()
        checkedStatus(
            NativeApi.library.prns_event_string(pointer, field.rawValue, value),
            "eventString",
        )
        value.read()
        return copyString(value)
    }

    fun u64(field: EventField): Long {
        val value = LongByReference()
        checkedStatus(
            NativeApi.library.prns_event_u64(pointer, field.rawValue, value),
            "eventInteger",
        )
        return value.value
    }

    fun u128(field: EventField): BigInteger {
        val low = LongByReference()
        val high = LongByReference()
        checkedStatus(
            NativeApi.library.prns_event_u128(
                pointer,
                field.rawValue,
                low,
                high,
            ),
            "eventInteger",
        )
        return unsigned(high.value).shiftLeft(64).or(unsigned(low.value))
    }

    fun resource(): ResourceStream {
        val output = PointerByReference()
        checkedStatus(
            NativeApi.library.prns_event_resource_stream(pointer, output),
            "claimResourceStream",
        )
        return NativeResourceStream(
            pointer = requireNotNull(output.value),
            totalBytes = u64(EventField.TOTAL_BYTES).toULong(),
        )
    }

    private fun unsigned(value: Long): BigInteger {
        val lower = BigInteger.valueOf(value and Long.MAX_VALUE)
        return if (value < 0) lower.setBit(63) else lower
    }
}

internal fun decodeApplicationEvent(pointer: Pointer): ApplicationEvent {
    val event = EventReader(pointer)
    return when (event.applicationKind) {
        ApplicationEventKind.SINGLE_DELIVERY -> ApplicationEventSingleDelivery(
            destination = DestinationHash(event.bytes(EventField.DESTINATION)),
            sourceInterface = InterfaceId(event.bytes(EventField.SOURCE_INTERFACE)),
            plaintext = Bytes(event.bytes(EventField.PLAINTEXT)),
        )
        ApplicationEventKind.LINK_DELIVERY -> ApplicationEventLinkDelivery(
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            sourceInterface = InterfaceId(event.bytes(EventField.SOURCE_INTERFACE)),
            plaintext = Bytes(event.bytes(EventField.PLAINTEXT)),
        )
        ApplicationEventKind.REQUEST -> ApplicationEventRequest(
            destination = DestinationHash(event.bytes(EventField.DESTINATION)),
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            requestId = RequestId(event.bytes(EventField.REQUEST_ID)),
            requester = event.optionalBytes(EventField.REQUESTER)?.let(::IdentityHash),
            pathHash = RequestPathHash(event.bytes(EventField.PATH_HASH)),
            rttMillis = event.u64(EventField.RTT_MILLIS),
            data = Bytes(event.bytes(EventField.DATA)),
        )
        ApplicationEventKind.RESPONSE -> ApplicationEventResponse(
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            requestId = RequestId(event.bytes(EventField.REQUEST_ID)),
            data = Bytes(event.bytes(EventField.DATA)),
        )
        ApplicationEventKind.RESPONSE_SEGMENT -> ApplicationEventResponseSegment(
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            requestId = RequestId(event.bytes(EventField.REQUEST_ID)),
            segmentIndex = event.u64(EventField.SEGMENT_INDEX),
            totalSegments = event.u64(EventField.TOTAL_SEGMENTS),
            data = Bytes(event.bytes(EventField.DATA)),
        )
        ApplicationEventKind.RESOURCE_AVAILABLE -> ApplicationEventResourceAvailable(
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            hash = ResourceHash(event.bytes(EventField.HASH)),
            metadata = event.optionalBytes(EventField.METADATA)?.let(::Bytes),
            resource = event.resource(),
        )
        ApplicationEventKind.RESOURCE_SEGMENT -> ApplicationEventResourceSegment(
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            originalHash = ResourceHash(event.bytes(EventField.ORIGINAL_HASH)),
            segmentIndex = event.u64(EventField.SEGMENT_INDEX),
            totalSegments = event.u64(EventField.TOTAL_SEGMENTS),
            metadata = event.optionalBytes(EventField.METADATA)?.let(::Bytes),
            data = Bytes(event.bytes(EventField.DATA)),
        )
        ApplicationEventKind.RESOURCE_NEEDS_DECOMPRESSION ->
            ApplicationEventResourceNeedsDecompression(
                linkId = LinkId(event.bytes(EventField.LINK_ID)),
                hash = ResourceHash(event.bytes(EventField.HASH)),
                stream = Bytes(event.bytes(EventField.STREAM)),
                uncompressedDataBytes = event.u64(EventField.UNCOMPRESSED_DATA_BYTES).toULong(),
            )
        ApplicationEventKind.CHANNEL_MESSAGE -> ApplicationEventChannelMessage(
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            messageType = event.u64(EventField.MESSAGE_TYPE).also {
                require(it in 0..0xffff)
            }.toInt(),
            data = Bytes(event.bytes(EventField.DATA)),
        )
    }
}

internal fun decodeDiagnosticEvent(pointer: Pointer): DiagnosticEvent {
    val event = EventReader(pointer)
    return when (event.diagnosticKind) {
        DiagnosticEventKind.ANNOUNCE_HEARD -> {
            val hops = event.u64(EventField.HOPS)
            if (hops !in 0..255) {
                throw StatusException("decodeAnnounceHops", Status.BACKEND_FAILED)
            }
            DiagnosticEventAnnounceHeard(
                destination = DestinationHash(event.bytes(EventField.DESTINATION)),
                hops = hops.toInt(),
                sourceInterface = InterfaceId(
                    event.bytes(EventField.SOURCE_INTERFACE),
                ),
                appData = Bytes(event.bytes(EventField.APP_DATA)),
            )
        }
        DiagnosticEventKind.LINK_ESTABLISHED -> DiagnosticEventLinkEstablished(
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            rttMillis = event.u64(EventField.RTT_MILLIS),
        )
        DiagnosticEventKind.PEER_IDENTIFIED -> DiagnosticEventPeerIdentified(
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            identity = IdentityHash(event.bytes(EventField.IDENTITY)),
        )
        DiagnosticEventKind.LINK_CLOSED -> {
            val rawReason = event.u64(EventField.REASON)
            val reason = if (rawReason in 0..Int.MAX_VALUE.toLong()) {
                LinkClosedReason.fromRawValue(rawReason.toInt())
            } else {
                null
            } ?: throw StatusException("linkClosedReason", Status.BACKEND_FAILED)
            DiagnosticEventLinkClosed(
                linkId = LinkId(event.bytes(EventField.LINK_ID)),
                reason = reason,
            )
        }
        DiagnosticEventKind.LINK_INTERFACE_MISMATCH ->
            DiagnosticEventLinkInterfaceMismatch(
                linkId = LinkId(event.bytes(EventField.LINK_ID)),
                attachedInterface = InterfaceId(
                    event.bytes(EventField.ATTACHED_INTERFACE),
                ),
                arrivedOn = InterfaceId(event.bytes(EventField.ARRIVED_ON)),
            )
        DiagnosticEventKind.RESOURCE_ASSEMBLED -> DiagnosticEventResourceAssembled(
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            originalHash = ResourceHash(event.bytes(EventField.ORIGINAL_HASH)),
            totalSizeBytes = event.u64(EventField.TOTAL_SIZE_BYTES).toULong(),
        )
        DiagnosticEventKind.RESOURCE_FAILED -> DiagnosticEventResourceFailed(
            linkId = LinkId(event.bytes(EventField.LINK_ID)),
            hash = ResourceHash(event.bytes(EventField.HASH)),
            cause = event.string(EventField.CAUSE),
        )
        DiagnosticEventKind.RESOURCE_SEND_PROGRESS ->
            DiagnosticEventResourceSendProgress(
                linkId = LinkId(event.bytes(EventField.LINK_ID)),
                transferredBytes = event.u64(EventField.TRANSFERRED_BYTES).toULong(),
                totalBytes = event.u64(EventField.TOTAL_BYTES).toULong(),
                physicalTransferredBytes = event.u64(
                    EventField.PHYSICAL_TRANSFERRED_BYTES,
                ).toULong(),
                segmentIndex = event.u64(EventField.SEGMENT_INDEX),
                totalSegments = event.u64(EventField.TOTAL_SEGMENTS),
            )
        DiagnosticEventKind.SELF_RATCHET_ROTATED -> DiagnosticEventSelfRatchetRotated(
            DestinationHash(event.bytes(EventField.DESTINATION)),
        )
        DiagnosticEventKind.ANNOUNCE_HELD_DROPPED ->
            DiagnosticEventAnnounceHeldDropped(
                destination = DestinationHash(event.bytes(EventField.DESTINATION)),
                sourceInterface = InterfaceId(
                    event.bytes(EventField.SOURCE_INTERFACE),
                ),
                cause = event.string(EventField.CAUSE),
            )
        DiagnosticEventKind.DELIVERED -> DiagnosticEventDelivered(
            event.string(EventField.DETAIL),
        )
        DiagnosticEventKind.ROUTE_EXPIRED -> DiagnosticEventRouteExpired(
            DestinationHash(event.bytes(EventField.DESTINATION)),
        )
        DiagnosticEventKind.ROUTE_EVICTED -> DiagnosticEventRouteEvicted(
            DestinationHash(event.bytes(EventField.DESTINATION)),
        )
        DiagnosticEventKind.ROUTE_INTERFACE_GONE -> DiagnosticEventRouteInterfaceGone(
            DestinationHash(event.bytes(EventField.DESTINATION)),
        )
        DiagnosticEventKind.ROUTE_DROPPED -> DiagnosticEventRouteDropped(
            DestinationHash(event.bytes(EventField.DESTINATION)),
        )
        DiagnosticEventKind.BACKEND_DIAGNOSTIC -> DiagnosticEventBackendDiagnostic(
            kind = event.string(EventField.KIND),
            detail = event.string(EventField.DETAIL),
        )
        DiagnosticEventKind.DIAGNOSTICS_DROPPED -> DiagnosticEventDiagnosticsDropped(
            event.u128(EventField.DROPPED_COUNT),
        )
        DiagnosticEventKind.PERSISTENCE_RESTORED -> DiagnosticEventPersistenceRestored(
            routes = event.u64(EventField.ROUTES),
            destinationIdentities = event.u64(EventField.DESTINATION_IDENTITIES),
            tunnels = event.u64(EventField.TUNNELS),
            ratchets = event.u64(EventField.RATCHETS),
            refused = event.u64(EventField.REFUSED),
            dropped = event.u64(EventField.DROPPED),
        )
        DiagnosticEventKind.PERSISTENCE_FLUSHED -> DiagnosticEventPersistenceFlushed(
            cause = persistenceCause(event),
            target = persistenceTarget(event),
        )
        DiagnosticEventKind.PERSISTENCE_FLUSH_FAILED ->
            DiagnosticEventPersistenceFlushFailed(
                cause = persistenceCause(event),
                target = persistenceTarget(event),
            )
    }
}

private fun persistenceCause(event: EventReader): PersistenceFlushCause {
    val raw = event.u64(EventField.PERSISTENCE_CAUSE)
    return if (raw in 0..Int.MAX_VALUE.toLong()) {
        PersistenceFlushCause.fromRawValue(raw.toInt())
    } else {
        null
    } ?: throw StatusException("persistenceFlushCause", Status.BACKEND_FAILED)
}

private fun persistenceTarget(event: EventReader): PersistenceFlushTarget {
    val raw = event.u64(EventField.PERSISTENCE_TARGET)
    return if (raw in 0..Int.MAX_VALUE.toLong()) {
        PersistenceFlushTarget.fromRawValue(raw.toInt())
    } else {
        null
    } ?: throw StatusException("persistenceFlushTarget", Status.BACKEND_FAILED)
}
