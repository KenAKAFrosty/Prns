package io.reticulum.prns

import java.math.BigInteger

object HostContract {
    const val ABI: Int = 1
    const val SCHEMA_VERSION: Int = 1
    const val PRODUCT_VERSION = "0.2.8"
    const val DESTINATION_HASH_LENGTH = 16
    const val IDENTITY_HASH_LENGTH = 16
    const val INTERFACE_ID_LENGTH = 8
    const val LINK_ID_LENGTH = 16
    const val PACKET_HASH_LENGTH = 32
    const val REQUEST_ID_LENGTH = 16
    const val REQUEST_PATH_HASH_LENGTH = 16
    const val RESOURCE_HASH_LENGTH = 32
    const val IDENTITY_SECRET_LENGTH = 64
    const val BALANCED_PENDING_COMMANDS = 256
    const val BALANCED_APPLICATION_EVENTS = 1024
    const val BALANCED_RETAINED_EVENT_BYTES = 8388608
    const val BALANCED_DIAGNOSTICS = 1024
}

enum class Status(val rawValue: Int) {
    OK(0),
    INVALID_ARGUMENT(1),
    CONTRACT_MISMATCH(2),
    INVALID_HANDLE(3),
    NOT_READY(4),
    ALREADY_CLAIMED(5),
    WOULD_BLOCK(6),
    TIMED_OUT(7),
    QUEUE_FULL(8),
    STOPPED(9),
    BACKEND_FAILED(10),
    PANIC(11),
    INTERRUPTED(12);

    companion object {
        fun fromRawValue(value: Int): Status? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class BackendKind(val rawValue: Int) {
    NATIVE(1),
    BROWSER(2),
    COOPERATIVE(3);

    companion object {
        fun fromRawValue(value: Int): BackendKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class Capability(val rawValue: Int) {
    LOOPBACK(1),
    TCP_CLIENT(2),
    TCP_SERVER(3),
    UDP(4),
    SERIAL(5),
    USB(6),
    BLUETOOTH(7),
    WIFI(8),
    WEB_SOCKET(9),
    BROWSER_RENDEZVOUS(10),
    I2P(11),
    WEAVE(12);

    companion object {
        fun fromRawValue(value: Int): Capability? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class HostRole(val rawValue: Int) {
    ENDPOINT(1),
    TRANSPORT(2);

    companion object {
        fun fromRawValue(value: Int): HostRole? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class IdentityConfigKind(val rawValue: Int) {
    EXISTING(1),
    GENERATE_EPHEMERAL(2),
    LOAD_OR_CREATE(3);

    companion object {
        fun fromRawValue(value: Int): IdentityConfigKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class DestinationConfigKind(val rawValue: Int) {
    PLAIN(1),
    SINGLE(2);

    companion object {
        fun fromRawValue(value: Int): DestinationConfigKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class DestinationIdentityConfigKind(val rawValue: Int) {
    HOST_IDENTITY(1),
    DEDICATED_IDENTITY(2);

    companion object {
        fun fromRawValue(value: Int): DestinationIdentityConfigKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class BitrateKind(val rawValue: Int) {
    AUTO(1),
    BITS_PER_SECOND(2);

    companion object {
        fun fromRawValue(value: Int): BitrateKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class CommandOutcomeKind(val rawValue: Int) {
    ANNOUNCED(1),
    PACKET_DELIVERED(2),
    LINK_CLOSE_QUEUED(3),
    INTERFACE_ATTACHED(4),
    INTERFACE_DETACHED(5);

    companion object {
        fun fromRawValue(value: Int): CommandOutcomeKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class CommandFailureKind(val rawValue: Int) {
    NODE_STOPPED(1),
    BUSY(2),
    PAYLOAD_TOO_LARGE(3),
    UNKNOWN_DESTINATION(4),
    NOT_SINGLE_DESTINATION(5),
    ANNOUNCE_APP_DATA_TOO_LONG(6),
    UNKNOWN_INTERFACE(7),
    NO_ROUTE_TO_DESTINATION(8),
    NOT_DIRECTLY_REACHABLE(9),
    PACKET_CULLED(10),
    DELIVERY_TIMED_OUT(11),
    INVALID_BITRATE(12),
    BIND_FAILED(13),
    WRITE_FAILED(14);

    companion object {
        fun fromRawValue(value: Int): CommandFailureKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class DeliveryEvidenceKind(val rawValue: Int) {
    EXPLICIT_PROOF(1),
    IMPLICIT_PROOF(2),
    RESPONSE(3);

    companion object {
        fun fromRawValue(value: Int): DeliveryEvidenceKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class LifecyclePhase(val rawValue: Int) {
    STARTING(1),
    RUNNING(2),
    STOPPING(3),
    STOPPED(4),
    FAILED(5);

    companion object {
        fun fromRawValue(value: Int): LifecyclePhase? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class StopReason(val rawValue: Int) {
    REQUESTED(1),
    BACKEND_EXITED(2);

    companion object {
        fun fromRawValue(value: Int): StopReason? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class LinkClosedReason(val rawValue: Int) {
    TIMEOUT(1),
    PEER_CLOSED(2),
    MALFORMED_RTT(3);

    companion object {
        fun fromRawValue(value: Int): LinkClosedReason? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class ApplicationEventKind(val rawValue: Int) {
    SINGLE_DELIVERY(100),
    REQUEST(101),
    RESPONSE(102),
    RESPONSE_SEGMENT(103),
    RESOURCE_AVAILABLE(104),
    RESOURCE_SEGMENT(105),
    RESOURCE_NEEDS_DECOMPRESSION(106),
    CHANNEL_MESSAGE(107);

    companion object {
        fun fromRawValue(value: Int): ApplicationEventKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class DiagnosticEventKind(val rawValue: Int) {
    ANNOUNCE_HEARD(200),
    LINK_ESTABLISHED(201),
    PEER_IDENTIFIED(202),
    LINK_CLOSED(203),
    LINK_INTERFACE_MISMATCH(204),
    RESOURCE_ASSEMBLED(205),
    RESOURCE_FAILED(206),
    RESOURCE_SEND_PROGRESS(207),
    SELF_RATCHET_ROTATED(208),
    ANNOUNCE_HELD_DROPPED(209),
    DELIVERED(210),
    ROUTE_EXPIRED(211),
    ROUTE_EVICTED(212),
    ROUTE_INTERFACE_GONE(213),
    ROUTE_DROPPED(214),
    BACKEND_DIAGNOSTIC(215),
    DIAGNOSTICS_DROPPED(216);

    companion object {
        fun fromRawValue(value: Int): DiagnosticEventKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class EventField(val rawValue: Int) {
    DESTINATION(1),
    SOURCE_INTERFACE(2),
    PLAINTEXT(3),
    LINK_ID(4),
    REQUEST_ID(5),
    REQUESTER(6),
    PATH_HASH(7),
    RTT_MILLIS(8),
    DATA(9),
    SEGMENT_INDEX(10),
    TOTAL_SEGMENTS(11),
    HASH(12),
    ORIGINAL_HASH(13),
    METADATA(14),
    TOTAL_BYTES(15),
    STREAM_ID(16),
    UNCOMPRESSED_DATA_BYTES(17),
    MESSAGE_TYPE(18),
    IDENTITY(19),
    REASON(20),
    ATTACHED_INTERFACE(21),
    ARRIVED_ON(22),
    TOTAL_SIZE_BYTES(23),
    CAUSE(24),
    TRANSFERRED_BYTES(25),
    PHYSICAL_TRANSFERRED_BYTES(26),
    DETAIL(27),
    KIND(28),
    DROPPED_COUNT(29),
    HOPS(30),
    STREAM(31);

    companion object {
        fun fromRawValue(value: Int): EventField? = entries.firstOrNull { it.rawValue == value }
    }
}

class DestinationHash(bytes: ByteArray) {
    private val storage = bytes.copyOf()

    init {
        require(storage.size == HostContract.DESTINATION_HASH_LENGTH)
    }

    fun copyBytes(): ByteArray = storage.copyOf()

    override fun equals(other: Any?): Boolean = other is DestinationHash && storage.contentEquals(other.storage)
    override fun hashCode(): Int = storage.contentHashCode()
}

class IdentityHash(bytes: ByteArray) {
    private val storage = bytes.copyOf()

    init {
        require(storage.size == HostContract.IDENTITY_HASH_LENGTH)
    }

    fun copyBytes(): ByteArray = storage.copyOf()

    override fun equals(other: Any?): Boolean = other is IdentityHash && storage.contentEquals(other.storage)
    override fun hashCode(): Int = storage.contentHashCode()
}

class InterfaceId(bytes: ByteArray) {
    private val storage = bytes.copyOf()

    init {
        require(storage.size == HostContract.INTERFACE_ID_LENGTH)
    }

    fun copyBytes(): ByteArray = storage.copyOf()

    override fun equals(other: Any?): Boolean = other is InterfaceId && storage.contentEquals(other.storage)
    override fun hashCode(): Int = storage.contentHashCode()
}

class LinkId(bytes: ByteArray) {
    private val storage = bytes.copyOf()

    init {
        require(storage.size == HostContract.LINK_ID_LENGTH)
    }

    fun copyBytes(): ByteArray = storage.copyOf()

    override fun equals(other: Any?): Boolean = other is LinkId && storage.contentEquals(other.storage)
    override fun hashCode(): Int = storage.contentHashCode()
}

class PacketHash(bytes: ByteArray) {
    private val storage = bytes.copyOf()

    init {
        require(storage.size == HostContract.PACKET_HASH_LENGTH)
    }

    fun copyBytes(): ByteArray = storage.copyOf()

    override fun equals(other: Any?): Boolean = other is PacketHash && storage.contentEquals(other.storage)
    override fun hashCode(): Int = storage.contentHashCode()
}

class RequestId(bytes: ByteArray) {
    private val storage = bytes.copyOf()

    init {
        require(storage.size == HostContract.REQUEST_ID_LENGTH)
    }

    fun copyBytes(): ByteArray = storage.copyOf()

    override fun equals(other: Any?): Boolean = other is RequestId && storage.contentEquals(other.storage)
    override fun hashCode(): Int = storage.contentHashCode()
}

class RequestPathHash(bytes: ByteArray) {
    private val storage = bytes.copyOf()

    init {
        require(storage.size == HostContract.REQUEST_PATH_HASH_LENGTH)
    }

    fun copyBytes(): ByteArray = storage.copyOf()

    override fun equals(other: Any?): Boolean = other is RequestPathHash && storage.contentEquals(other.storage)
    override fun hashCode(): Int = storage.contentHashCode()
}

class ResourceHash(bytes: ByteArray) {
    private val storage = bytes.copyOf()

    init {
        require(storage.size == HostContract.RESOURCE_HASH_LENGTH)
    }

    fun copyBytes(): ByteArray = storage.copyOf()

    override fun equals(other: Any?): Boolean = other is ResourceHash && storage.contentEquals(other.storage)
    override fun hashCode(): Int = storage.contentHashCode()
}

class IdentitySecret(bytes: ByteArray) : AutoCloseable {
    private val storage = bytes.copyOf()

    init {
        require(storage.size == HostContract.IDENTITY_SECRET_LENGTH)
    }

    fun copyBytes(): ByteArray = storage.copyOf()

    override fun close() {
        storage.fill(0)
    }
}

class Bytes(bytes: ByteArray) {
    private val storage = bytes.copyOf()

    val size: Int
        get() = storage.size

    fun copyBytes(): ByteArray = storage.copyOf()

    override fun equals(other: Any?): Boolean = other is Bytes && storage.contentEquals(other.storage)
    override fun hashCode(): Int = storage.contentHashCode()
    override fun toString(): String = "Bytes(size=$size)"
}

data class DestinationName(
    val appName: String,
    val aspects: List<String>,
)

interface ResourceStream : AutoCloseable {
    val totalBytes: Long
    fun next(maximumBytes: Int): ResourceChunk
}

data class ResourceChunk(val bytes: Bytes, val finished: Boolean)

sealed interface IdentityConfig

data class IdentityConfigExisting(
    val secret: IdentitySecret
) : IdentityConfig

data object IdentityConfigGenerateEphemeral : IdentityConfig

data class IdentityConfigLoadOrCreate(
    val path: String
) : IdentityConfig

sealed interface DestinationIdentityConfig

data object DestinationIdentityConfigHostIdentity : DestinationIdentityConfig

data class DestinationIdentityConfigDedicatedIdentity(
    val identity: IdentityConfig
) : DestinationIdentityConfig

sealed interface Bitrate

data object BitrateAuto : Bitrate

data class BitrateBitsPerSecond(
    val value: Long
) : Bitrate

sealed interface DestinationConfig

data class DestinationConfigPlain(
    val name: DestinationName
) : DestinationConfig

data class DestinationConfigSingle(
    val name: DestinationName,
    val identity: DestinationIdentityConfig,
    val announceAppData: Bytes?
) : DestinationConfig

sealed interface HostCommand

data class HostCommandAnnounce(
    val destination: DestinationHash,
    val `interface`: InterfaceId?
) : HostCommand

data class HostCommandSendSinglePacket(
    val destination: DestinationHash,
    val payload: Bytes
) : HostCommand

data class HostCommandCloseLink(
    val linkId: LinkId
) : HostCommand

data class HostCommandAttachTcpServer(
    val bind: String,
    val bitrate: Bitrate
) : HostCommand

data class HostCommandAttachTcpClient(
    val target: String,
    val bitrate: Bitrate
) : HostCommand

data class HostCommandAttachUdp(
    val local: String,
    val peer: String,
    val bitrate: Bitrate
) : HostCommand

data class HostCommandDetachInterface(
    val `interface`: InterfaceId
) : HostCommand

sealed interface CommandOutcome

data object CommandOutcomeAnnounced : CommandOutcome

data class CommandOutcomePacketDelivered(
    val rttMillis: Long,
    val evidence: DeliveryEvidenceKind,
    val packetHash: PacketHash?
) : CommandOutcome

data object CommandOutcomeLinkCloseQueued : CommandOutcome

data class CommandOutcomeInterfaceAttached(
    val `interface`: InterfaceId
) : CommandOutcome

data class CommandOutcomeInterfaceDetached(
    val `interface`: InterfaceId
) : CommandOutcome

sealed interface ApplicationEvent

data class ApplicationEventSingleDelivery(
    val destination: DestinationHash,
    val sourceInterface: InterfaceId,
    val plaintext: Bytes
) : ApplicationEvent

data class ApplicationEventRequest(
    val destination: DestinationHash,
    val linkId: LinkId,
    val requestId: RequestId,
    val requester: IdentityHash?,
    val pathHash: RequestPathHash,
    val rttMillis: Long,
    val data: Bytes
) : ApplicationEvent

data class ApplicationEventResponse(
    val linkId: LinkId,
    val requestId: RequestId,
    val data: Bytes
) : ApplicationEvent

data class ApplicationEventResponseSegment(
    val linkId: LinkId,
    val requestId: RequestId,
    val segmentIndex: Long,
    val totalSegments: Long,
    val data: Bytes
) : ApplicationEvent

data class ApplicationEventResourceAvailable(
    val linkId: LinkId,
    val hash: ResourceHash,
    val metadata: Bytes?,
    val resource: ResourceStream
) : ApplicationEvent

data class ApplicationEventResourceSegment(
    val linkId: LinkId,
    val originalHash: ResourceHash,
    val segmentIndex: Long,
    val totalSegments: Long,
    val metadata: Bytes?,
    val data: Bytes
) : ApplicationEvent

data class ApplicationEventResourceNeedsDecompression(
    val linkId: LinkId,
    val hash: ResourceHash,
    val stream: Bytes,
    val uncompressedDataBytes: Long
) : ApplicationEvent

data class ApplicationEventChannelMessage(
    val linkId: LinkId,
    val messageType: String,
    val data: Bytes
) : ApplicationEvent

sealed interface DiagnosticEvent

data class DiagnosticEventAnnounceHeard(
    val destination: DestinationHash,
    val hops: Int,
    val sourceInterface: InterfaceId
) : DiagnosticEvent

data class DiagnosticEventLinkEstablished(
    val linkId: LinkId,
    val rttMillis: Long
) : DiagnosticEvent

data class DiagnosticEventPeerIdentified(
    val linkId: LinkId,
    val identity: IdentityHash
) : DiagnosticEvent

data class DiagnosticEventLinkClosed(
    val linkId: LinkId,
    val reason: LinkClosedReason
) : DiagnosticEvent

data class DiagnosticEventLinkInterfaceMismatch(
    val linkId: LinkId,
    val attachedInterface: InterfaceId,
    val arrivedOn: InterfaceId
) : DiagnosticEvent

data class DiagnosticEventResourceAssembled(
    val linkId: LinkId,
    val originalHash: ResourceHash,
    val totalSizeBytes: Long
) : DiagnosticEvent

data class DiagnosticEventResourceFailed(
    val linkId: LinkId,
    val hash: ResourceHash,
    val cause: String
) : DiagnosticEvent

data class DiagnosticEventResourceSendProgress(
    val linkId: LinkId,
    val transferredBytes: Long,
    val totalBytes: Long,
    val physicalTransferredBytes: Long,
    val segmentIndex: Long,
    val totalSegments: Long
) : DiagnosticEvent

data class DiagnosticEventSelfRatchetRotated(
    val destination: DestinationHash
) : DiagnosticEvent

data class DiagnosticEventAnnounceHeldDropped(
    val destination: DestinationHash,
    val sourceInterface: InterfaceId,
    val cause: String
) : DiagnosticEvent

data class DiagnosticEventDelivered(
    val detail: String
) : DiagnosticEvent

data class DiagnosticEventRouteExpired(
    val destination: DestinationHash
) : DiagnosticEvent

data class DiagnosticEventRouteEvicted(
    val destination: DestinationHash
) : DiagnosticEvent

data class DiagnosticEventRouteInterfaceGone(
    val destination: DestinationHash
) : DiagnosticEvent

data class DiagnosticEventRouteDropped(
    val destination: DestinationHash
) : DiagnosticEvent

data class DiagnosticEventBackendDiagnostic(
    val kind: String,
    val detail: String
) : DiagnosticEvent

data class DiagnosticEventDiagnosticsDropped(
    val count: BigInteger
) : DiagnosticEvent
