package rs.reticulum.prns

import java.math.BigInteger

object HostContract {
    const val ABI: Int = 1
    const val SCHEMA_VERSION: Int = 1
    const val PRODUCT_VERSION = "0.3.7"
    const val DESTINATION_HASH_LENGTH = 16
    const val IDENTITY_HASH_LENGTH = 16
    const val INTERFACE_ID_LENGTH = 8
    const val LINK_ID_LENGTH = 16
    const val PACKET_HASH_LENGTH = 32
    const val REQUEST_ID_LENGTH = 16
    const val REQUEST_PATH_HASH_LENGTH = 16
    const val RESOURCE_HASH_LENGTH = 32
    const val IDENTITY_SECRET_LENGTH = 64
    const val SAFE_INT_MIN = -9007199254740991L
    const val SAFE_INT_MAX = 9007199254740991L
    const val SAFE_UINT_MAX = 9007199254740991L
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
    INTERRUPTED(12),
    UNSUPPORTED(13),
    PERMISSION_DENIED(14),
    UNAVAILABLE(15);

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
    WEAVE(12),
    SUPPLIED_PIPE(13);

    companion object {
        fun fromRawValue(value: Int): Capability? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class InterfaceKind(val rawValue: Int) {
    AUTO_LAN(1),
    TCP_CLIENT(2),
    TCP_SERVER(3),
    UDP(4),
    SERIAL(5),
    KISS(6),
    AX25_KISS(7),
    R_NODE(8),
    MULTI_R_NODE(9),
    PIPE(10),
    BACKBONE_CLIENT(11),
    BACKBONE_SERVER(12),
    I2P(13),
    WEAVE(14),
    AUTOMATIC_USB(15),
    AUTOMATIC_BLUETOOTH_LE(16),
    WEB_SOCKET_CLIENT(17),
    WEB_SOCKET_SERVER(18),
    BROWSER_RENDEZVOUS(19);

    companion object {
        fun fromRawValue(value: Int): InterfaceKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class InterfaceMode(val rawValue: Int) {
    FULL(1),
    POINT_TO_POINT(2),
    ACCESS_POINT(3),
    ROAMING(4),
    BOUNDARY(5),
    GATEWAY(6),
    INTERNAL(7);

    companion object {
        fun fromRawValue(value: Int): InterfaceMode? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class WebSocketFramingSelection(val rawValue: Int) {
    RAW_PACKET(1),
    HDLC(2),
    KISS(3),
    AUTO(4);

    companion object {
        fun fromRawValue(value: Int): WebSocketFramingSelection? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class InterfaceHealth(val rawValue: Int) {
    INITIALIZING(1),
    CONNECTED(2),
    DEGRADED(3),
    RECONNECTING(4),
    FAILED(5),
    DISCONNECTED(6),
    DISABLED(7),
    UNKNOWN(8);

    companion object {
        fun fromRawValue(value: Int): InterfaceHealth? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class DiscoveryScope(val rawValue: Int) {
    LINK(1),
    ADMIN(2),
    SITE(3),
    ORGANIZATION(4),
    GLOBAL(5);

    companion object {
        fun fromRawValue(value: Int): DiscoveryScope? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class MulticastAddressType(val rawValue: Int) {
    TEMPORARY(1),
    PERMANENT(2);

    companion object {
        fun fromRawValue(value: Int): MulticastAddressType? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class SerialDataBits(val rawValue: Int) {
    FIVE(5),
    SIX(6),
    SEVEN(7),
    EIGHT(8);

    companion object {
        fun fromRawValue(value: Int): SerialDataBits? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class SerialParity(val rawValue: Int) {
    NONE(1),
    EVEN(2),
    ODD(3);

    companion object {
        fun fromRawValue(value: Int): SerialParity? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class SerialStopBits(val rawValue: Int) {
    ONE(1),
    TWO(2);

    companion object {
        fun fromRawValue(value: Int): SerialStopBits? = entries.firstOrNull { it.rawValue == value }
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

enum class PersistenceConfigKind(val rawValue: Int) {
    EPHEMERAL(1),
    DIRECTORY(2);

    companion object {
        fun fromRawValue(value: Int): PersistenceConfigKind? = entries.firstOrNull { it.rawValue == value }
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

enum class ResponseTimeoutKind(val rawValue: Int) {
    LINK_DEFAULT(1),
    EXACT(2);

    companion object {
        fun fromRawValue(value: Int): ResponseTimeoutKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class ResourceCompressionKind(val rawValue: Int) {
    AUTO(1),
    NEVER(2);

    companion object {
        fun fromRawValue(value: Int): ResourceCompressionKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class ResourceStrategyKind(val rawValue: Int) {
    REFUSE(1),
    ACCEPT(2);

    companion object {
        fun fromRawValue(value: Int): ResourceStrategyKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class RequestPolicy(val rawValue: Int) {
    ALLOW_NONE(1),
    ALLOW_ALL(2),
    ALLOW_LIST(3);

    companion object {
        fun fromRawValue(value: Int): RequestPolicy? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class CommandOutcomeKind(val rawValue: Int) {
    ANNOUNCED(1),
    PACKET_DELIVERED(2),
    LINK_CLOSE_QUEUED(3),
    INTERFACE_ATTACHED(4),
    INTERFACE_DETACHED(5),
    LINK_ESTABLISHED(6),
    PATH_DISCOVERED(7),
    IDENTIFIED(8),
    RESPONSE_RECEIVED(9),
    RESPONSE_SENT(10),
    RESOURCE_SENT(11),
    RESOURCE_STRATEGY_SET(12),
    REQUESTER_ALLOWED(13);

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
    WRITE_FAILED(14),
    UNSUPPORTED_BY_BACKEND(15),
    UNKNOWN_LINK(16),
    LINK_NOT_ACTIVE(17),
    ENTROPY_UNAVAILABLE(18),
    NOT_LINK_INITIATOR(19),
    IDENTITY_NOT_HELD(20),
    UNKNOWN_REQUEST_HANDLER(21),
    REQUEST_POLICY_NOT_ALLOW_LIST(22),
    REQUEST_ALLOW_LIST_FULL(23),
    LINK_BUSY(24),
    RESOURCE_TABLE_FULL(25),
    RESOURCE_METADATA_TOO_LARGE(26),
    RESOURCE_REJECTED_BY_PEER(27),
    RESOURCE_SEQUENCING_FAILED(28),
    RESOURCE_PREDECESSOR_FAILED(29),
    CHANNEL_WINDOW_FULL(30),
    CHANNEL_UNTRACKABLE(31),
    INVALID_CHANNEL_MESSAGE_TYPE(32),
    INVALID_CONFIGURATION(33),
    RESOURCE_UPLOAD_CANCELLED(34),
    RESOURCE_EARLY_EOF(35),
    RESOURCE_LENGTH_OVERRUN(36),
    PERMISSION_DENIED(37),
    DEVICE_UNAVAILABLE(38),
    CONNECT_FAILED(39),
    BACKEND_FAILED(40),
    RESPONSE_TOO_LARGE(41);

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
    CHANNEL_MESSAGE(107),
    LINK_DELIVERY(108);

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
    DIAGNOSTICS_DROPPED(216),
    PERSISTENCE_RESTORED(217),
    PERSISTENCE_FLUSHED(218),
    PERSISTENCE_FLUSH_FAILED(219);

    companion object {
        fun fromRawValue(value: Int): DiagnosticEventKind? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class PersistenceFlushCause(val rawValue: Int) {
    STARTUP(1),
    INTERVAL(2),
    ROUTE_CHANGE(3),
    RATCHET_ROTATION(4),
    SHUTDOWN(5);

    companion object {
        fun fromRawValue(value: Int): PersistenceFlushCause? = entries.firstOrNull { it.rawValue == value }
    }
}

enum class PersistenceFlushTarget(val rawValue: Int) {
    ROUTING_STATE(1),
    RATCHETS(2);

    companion object {
        fun fromRawValue(value: Int): PersistenceFlushTarget? = entries.firstOrNull { it.rawValue == value }
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
    STREAM(31),
    ROUTES(32),
    DESTINATION_IDENTITIES(33),
    TUNNELS(34),
    RATCHETS(35),
    REFUSED(36),
    DROPPED(37),
    PERSISTENCE_CAUSE(38),
    PERSISTENCE_TARGET(39),
    APP_DATA(40);

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

data class RequestHandlerConfig(
    val path: String,
    val policy: RequestPolicy,
)

data class SerialLineConfig(
    val baud: Long,
    val dataBits: SerialDataBits,
    val parity: SerialParity,
    val stopBits: SerialStopBits,
)

data class RNodeRadioConfig(
    val frequencyHz: Long,
    val bandwidthHz: Long,
    val txPowerDbm: Int,
    val spreadingFactor: Int,
    val codingRate: Int,
)

data class MultiRNodeMemberConfig(
    val name: String,
    val virtualPort: Int,
    val radio: RNodeRadioConfig,
    val flowControl: Boolean,
    val outgoing: Boolean,
)

data class InterfaceRoutingPolicy(
    val mode: InterfaceMode?,
    val gravity: Long?,
    val recursivePathRequests: Boolean?,
    val announcesFromInternal: Boolean?,
    val announcesToInternal: Boolean?,
)

data class BackendInfo(
    val backend: BackendKind,
    val capabilities: List<Capability>,
    val interfaceKinds: List<InterfaceKind>,
)

data class InterfaceSnapshot(
    val interfaceId: InterfaceId,
    val name: String?,
    val kind: InterfaceKind?,
    val health: InterfaceHealth,
    val failureDetail: String?,
    val rxBytes: ULong,
    val txBytes: ULong,
    val rxBps: Long?,
    val txBps: Long?,
    val routeCount: Long,
    val linkCount: Long,
    val transportedLinkCount: Long,
)

data class RouteSnapshot(
    val destination: DestinationHash,
    val hops: Int,
    val viaIdentity: IdentityHash?,
    val interfaceId: InterfaceId,
    val learnedAtMillis: Long,
    val lastRouteActivityAtMillis: Long,
    val expiresAtMillis: Long,
)

data class DestinationIdentitySnapshot(
    val destination: DestinationHash,
    val identity: IdentityHash,
)

data class RuntimeHealthSnapshot(
    val running: Boolean,
    val uptimeMillis: Long,
    val interfaceCount: Long,
    val onlineInterfaceCount: Long,
    val routeCount: Long,
    val linkCount: Long,
    val transportedLinkCount: Long,
    val rxBytes: ULong,
    val txBytes: ULong,
    val rxBps: Long,
    val txBps: Long,
)

data class PersistenceSnapshot(
    val persistent: Boolean,
    val restored: Boolean,
    val lastFlushCause: PersistenceFlushCause?,
    val lastFailureDetail: String?,
)

data class HostSnapshot(
    val revision: ULong,
    val backend: BackendInfo,
    val interfaces: List<InterfaceSnapshot>,
    val routes: List<RouteSnapshot>,
    val activeLinkCount: Long,
    val destinationIdentities: List<DestinationIdentitySnapshot>,
    val runtime: RuntimeHealthSnapshot,
    val persistence: PersistenceSnapshot,
)

interface ResourceStream : AutoCloseable {
    val totalBytes: ULong
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

sealed interface PersistenceConfig

data object PersistenceConfigEphemeral : PersistenceConfig

data class PersistenceConfigDirectory(
    val path: String
) : PersistenceConfig

sealed interface InterfaceConfig

data class InterfaceConfigAutoLan(
    val groupId: String?,
    val discoveryScope: DiscoveryScope?,
    val discoveryPort: Int?,
    val dataPort: Int?,
    val devices: List<String>,
    val ignoredDevices: List<String>,
    val multicastAddressType: MulticastAddressType?
) : InterfaceConfig

data class InterfaceConfigTcpClient(
    val target: String,
    val bitrate: Bitrate
) : InterfaceConfig

data class InterfaceConfigTcpServer(
    val bind: String,
    val bitrate: Bitrate
) : InterfaceConfig

data class InterfaceConfigUdp(
    val local: String,
    val peer: String,
    val bitrate: Bitrate
) : InterfaceConfig

data class InterfaceConfigSerial(
    val port: String,
    val line: SerialLineConfig
) : InterfaceConfig

data class InterfaceConfigKiss(
    val port: String,
    val line: SerialLineConfig,
    val flowControl: Boolean,
    val preambleMillis: Long,
    val transmitTailMillis: Long,
    val persistence: Int,
    val slotTimeMillis: Long,
    val stationCallsign: String?,
    val stationIntervalSeconds: Long?
) : InterfaceConfig

data class InterfaceConfigAx25Kiss(
    val port: String,
    val line: SerialLineConfig,
    val flowControl: Boolean,
    val preambleMillis: Long,
    val transmitTailMillis: Long,
    val persistence: Int,
    val slotTimeMillis: Long,
    val callsign: String,
    val ssid: Int
) : InterfaceConfig

data class InterfaceConfigRNode(
    val port: String,
    val radio: RNodeRadioConfig,
    val flowControl: Boolean,
    val stationCallsign: String?,
    val stationIntervalSeconds: Long?,
    val airtimeLimitShortCentiPercent: Int?,
    val airtimeLimitLongCentiPercent: Int?
) : InterfaceConfig

data class InterfaceConfigMultiRNode(
    val port: String,
    val stationCallsign: String?,
    val stationIntervalSeconds: Long?,
    val members: List<MultiRNodeMemberConfig>
) : InterfaceConfig

data class InterfaceConfigPipe(
    val command: List<String>,
    val respawnDelayMillis: Long
) : InterfaceConfig

data class InterfaceConfigBackboneClient(
    val target: String,
    val bitrate: Bitrate
) : InterfaceConfig

data class InterfaceConfigBackboneServer(
    val bind: String,
    val bitrate: Bitrate
) : InterfaceConfig

data class InterfaceConfigI2p(
    val peers: List<String>,
    val connectable: Boolean
) : InterfaceConfig

data class InterfaceConfigWeave(
    val port: String
) : InterfaceConfig

data object InterfaceConfigAutomaticUsb : InterfaceConfig

data object InterfaceConfigAutomaticBluetoothLe : InterfaceConfig

data class InterfaceConfigWebSocketClient(
    val target: String,
    val framing: WebSocketFramingSelection
) : InterfaceConfig

data class InterfaceConfigWebSocketServer(
    val bind: String,
    val framing: WebSocketFramingSelection
) : InterfaceConfig

data class InterfaceConfigBrowserRendezvous(
    val url: String
) : InterfaceConfig

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

sealed interface ResponseTimeout

data object ResponseTimeoutLinkDefault : ResponseTimeout

data class ResponseTimeoutExact(
    val millis: Long
) : ResponseTimeout

sealed interface ResourceCompression

data object ResourceCompressionAuto : ResourceCompression

data object ResourceCompressionNever : ResourceCompression

sealed interface ResourceStrategy

data object ResourceStrategyRefuse : ResourceStrategy

data class ResourceStrategyAccept(
    val maximumUncompressedBytes: Long,
    val acceptCompressed: Boolean
) : ResourceStrategy

sealed interface DestinationConfig

data class DestinationConfigPlain(
    val name: DestinationName
) : DestinationConfig

data class DestinationConfigSingle(
    val name: DestinationName,
    val identity: DestinationIdentityConfig,
    val announceAppData: Bytes?,
    val maximumRequestBytes: Long?,
    val requestHandlers: List<RequestHandlerConfig>
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

data class HostCommandEstablishLink(
    val destination: DestinationHash
) : HostCommand

data class HostCommandRequestPath(
    val destination: DestinationHash
) : HostCommand

data class HostCommandIdentify(
    val linkId: LinkId,
    val identity: IdentityHash
) : HostCommand

data class HostCommandSendLinkPacket(
    val linkId: LinkId,
    val payload: Bytes
) : HostCommand

data class HostCommandRequest(
    val linkId: LinkId,
    val pathHash: RequestPathHash,
    val payload: Bytes,
    val timeout: ResponseTimeout,
    val maximumResponseBytes: Long?
) : HostCommand

data class HostCommandRespond(
    val linkId: LinkId,
    val requestId: RequestId,
    val requestRttMillis: Long,
    val payload: Bytes
) : HostCommand

data class HostCommandSendResource(
    val linkId: LinkId,
    val payload: Bytes,
    val packedMetadata: Bytes?,
    val compression: ResourceCompression
) : HostCommand

data class HostCommandSetLinkResourceStrategy(
    val linkId: LinkId,
    val strategy: ResourceStrategy
) : HostCommand

data class HostCommandSetDestinationResourceStrategy(
    val destination: DestinationHash,
    val strategy: ResourceStrategy
) : HostCommand

data class HostCommandSendChannelMessage(
    val linkId: LinkId,
    val messageType: Int,
    val payload: Bytes
) : HostCommand

data class HostCommandAllowRequester(
    val destination: DestinationHash,
    val pathHash: RequestPathHash,
    val identity: IdentityHash
) : HostCommand

data class HostCommandAttachInterface(
    val config: InterfaceConfig,
    val routing: InterfaceRoutingPolicy?
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

data class CommandOutcomeLinkEstablished(
    val linkId: LinkId,
    val rttMillis: Long
) : CommandOutcome

data class CommandOutcomePathDiscovered(
    val hops: Int
) : CommandOutcome

data object CommandOutcomeIdentified : CommandOutcome

data class CommandOutcomeResponseReceived(
    val data: Bytes,
    val rttMillis: Long
) : CommandOutcome

data class CommandOutcomeResponseSent(
    val rttMillis: Long
) : CommandOutcome

data object CommandOutcomeResourceSent : CommandOutcome

data object CommandOutcomeResourceStrategySet : CommandOutcome

data object CommandOutcomeRequesterAllowed : CommandOutcome

sealed interface CommandFailure

data object CommandFailureNodeStopped : CommandFailure

data object CommandFailureBusy : CommandFailure

data object CommandFailurePayloadTooLarge : CommandFailure

data object CommandFailureUnknownDestination : CommandFailure

data object CommandFailureNotSingleDestination : CommandFailure

data object CommandFailureAnnounceAppDataTooLong : CommandFailure

data object CommandFailureUnknownInterface : CommandFailure

data object CommandFailureNoRouteToDestination : CommandFailure

data object CommandFailureNotDirectlyReachable : CommandFailure

data object CommandFailurePacketCulled : CommandFailure

data object CommandFailureDeliveryTimedOut : CommandFailure

data object CommandFailureInvalidBitrate : CommandFailure

data class CommandFailureBindFailed(
    val detail: String
) : CommandFailure

data class CommandFailureWriteFailed(
    val detail: String
) : CommandFailure

data object CommandFailureUnsupportedByBackend : CommandFailure

data object CommandFailureUnknownLink : CommandFailure

data object CommandFailureLinkNotActive : CommandFailure

data object CommandFailureEntropyUnavailable : CommandFailure

data object CommandFailureNotLinkInitiator : CommandFailure

data object CommandFailureIdentityNotHeld : CommandFailure

data object CommandFailureUnknownRequestHandler : CommandFailure

data object CommandFailureRequestPolicyNotAllowList : CommandFailure

data object CommandFailureRequestAllowListFull : CommandFailure

data object CommandFailureLinkBusy : CommandFailure

data object CommandFailureResourceTableFull : CommandFailure

data object CommandFailureResourceMetadataTooLarge : CommandFailure

data object CommandFailureResourceRejectedByPeer : CommandFailure

data object CommandFailureResourceSequencingFailed : CommandFailure

data object CommandFailureResourcePredecessorFailed : CommandFailure

data object CommandFailureChannelWindowFull : CommandFailure

data object CommandFailureChannelUntrackable : CommandFailure

data object CommandFailureInvalidChannelMessageType : CommandFailure

data class CommandFailureInvalidConfiguration(
    val detail: String
) : CommandFailure

data object CommandFailureResourceUploadCancelled : CommandFailure

data object CommandFailureResourceEarlyEof : CommandFailure

data object CommandFailureResourceLengthOverrun : CommandFailure

data class CommandFailurePermissionDenied(
    val detail: String
) : CommandFailure

data class CommandFailureDeviceUnavailable(
    val detail: String
) : CommandFailure

data class CommandFailureConnectFailed(
    val detail: String
) : CommandFailure

data class CommandFailureBackendFailed(
    val detail: String
) : CommandFailure

data object CommandFailureResponseTooLarge : CommandFailure

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
    val uncompressedDataBytes: ULong
) : ApplicationEvent

data class ApplicationEventChannelMessage(
    val linkId: LinkId,
    val messageType: Int,
    val data: Bytes
) : ApplicationEvent

data class ApplicationEventLinkDelivery(
    val linkId: LinkId,
    val sourceInterface: InterfaceId,
    val plaintext: Bytes
) : ApplicationEvent

sealed interface DiagnosticEvent

data class DiagnosticEventAnnounceHeard(
    val destination: DestinationHash,
    val hops: Int,
    val sourceInterface: InterfaceId,
    val appData: Bytes
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
    val totalSizeBytes: ULong
) : DiagnosticEvent

data class DiagnosticEventResourceFailed(
    val linkId: LinkId,
    val hash: ResourceHash,
    val cause: String
) : DiagnosticEvent

data class DiagnosticEventResourceSendProgress(
    val linkId: LinkId,
    val transferredBytes: ULong,
    val totalBytes: ULong,
    val physicalTransferredBytes: ULong,
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

data class DiagnosticEventPersistenceRestored(
    val routes: Long,
    val destinationIdentities: Long,
    val tunnels: Long,
    val ratchets: Long,
    val refused: Long,
    val dropped: Long
) : DiagnosticEvent

data class DiagnosticEventPersistenceFlushed(
    val cause: PersistenceFlushCause,
    val target: PersistenceFlushTarget
) : DiagnosticEvent

data class DiagnosticEventPersistenceFlushFailed(
    val cause: PersistenceFlushCause,
    val target: PersistenceFlushTarget
) : DiagnosticEvent

internal val HOST_OPERATION_NAMES = listOf(
    "contractInfo",
    "backendInfo",
    "hostCreate",
    "hostRelease",
    "hostLifecycle",
    "hostSnapshot",
    "hostSnapshotRead",
    "hostSnapshotRelease",
    "hostIdentityHash",
    "hostDestinationCount",
    "hostDestinationHash",
    "hostAttachSuppliedPipe",
    "suppliedPipeClaimAttachment",
    "suppliedPipeNextOpenRequest",
    "suppliedPipeRegisterReadiness",
    "suppliedPipeInterruptWait",
    "suppliedPipeRelease",
    "suppliedPipeOpenRequestProvide",
    "suppliedPipeOpenRequestDecline",
    "suppliedPipeOpenRequestRelease",
    "hostBeginResourceUpload",
    "resourceUploadWrite",
    "resourceUploadIsWritable",
    "resourceUploadFinish",
    "resourceUploadAbort",
    "resourceUploadRelease",
    "hostStop",
    "commandWait",
    "commandRegisterReadiness",
    "commandInterruptWait",
    "commandRelease",
    "hostClaimApplicationEvents",
    "hostClaimDiagnostics",
    "eventStreamRegisterReadiness",
    "readinessRegistrationRelease",
    "eventStreamInterruptWait",
    "eventStreamRelease",
    "eventStreamNext",
    "eventRelease",
    "eventKind",
    "eventBytes",
    "eventString",
    "eventU64",
    "eventU128",
    "eventResourceStream",
    "resourceStreamRelease",
    "resourceStreamNext",
    "hostAnnounce",
    "hostSendSinglePacket",
    "hostCloseLink",
    "hostAttachTcpServer",
    "hostAttachTcpClient",
    "hostAttachUdp",
    "hostDetachInterface",
    "hostEstablishLink",
    "hostRequestPath",
    "hostIdentify",
    "hostSendLinkPacket",
    "hostRequest",
    "hostRespond",
    "hostSendResource",
    "hostSetLinkResourceStrategy",
    "hostSetDestinationResourceStrategy",
    "hostSendChannelMessage",
    "hostAllowRequester",
    "hostAttachInterface",
)

internal data object RawUnit
internal data class RawOwned<Value>(val value: Value)
internal data class RawBorrowed<Value>(val value: Value)
internal sealed interface RawCallResult<out Value>
internal data class RawCallSuccess<Value>(val value: Value) : RawCallResult<Value>
internal data class RawCallFailure(val error: Status) : RawCallResult<Nothing>
internal class RawCommandResult
internal class RawContractInfo
internal class RawEvent
internal class RawEventStream
internal class RawHost
internal class RawHostInspection
internal class RawHostOptions
internal class RawIssuedCommand
internal class RawLifecycle
internal class RawReadinessCallback
internal class RawReadinessRegistration
internal class RawResourceChunk
internal class RawResourceStream
internal class RawResourceUpload
internal class RawSuppliedPipe
internal class RawSuppliedPipeOpenRequest
internal class RawOpaquePointer

internal interface RawHostProtocol {
    fun contractInfo(): RawCallResult<RawContractInfo>
    fun backendInfo(): RawCallResult<BackendInfo>
    fun hostCreate(options: RawHostOptions): RawCallResult<RawOwned<RawHost>>
    fun hostRelease(host: RawHost): RawUnit
    fun hostLifecycle(host: RawHost): RawCallResult<RawLifecycle>
    fun hostSnapshot(host: RawHost, timeoutMillis: Long): RawCallResult<RawOwned<RawHostInspection>>
    fun hostSnapshotRead(host_inspection: RawHostInspection): RawCallResult<RawBorrowed<HostSnapshot>>
    fun hostSnapshotRelease(host_inspection: RawHostInspection): RawUnit
    fun hostIdentityHash(host: RawHost): RawCallResult<RawBorrowed<Bytes>>
    fun hostDestinationCount(host: RawHost): Long
    fun hostDestinationHash(host: RawHost, index: Long): RawCallResult<RawBorrowed<Bytes>>
    fun hostAttachSuppliedPipe(host: RawHost, name: String, respawnDelayMillis: Long, bitrate: Bitrate): RawCallResult<RawOwned<RawSuppliedPipe>>
    fun suppliedPipeClaimAttachment(supplied_pipe: RawSuppliedPipe): RawCallResult<RawOwned<RawIssuedCommand>>
    fun suppliedPipeNextOpenRequest(supplied_pipe: RawSuppliedPipe, timeoutMillis: Long): RawCallResult<RawOwned<RawSuppliedPipeOpenRequest>>
    fun suppliedPipeRegisterReadiness(supplied_pipe: RawSuppliedPipe, callback: RawReadinessCallback, context: RawOpaquePointer): RawCallResult<RawOwned<RawReadinessRegistration>>
    fun suppliedPipeInterruptWait(supplied_pipe: RawSuppliedPipe): RawUnit
    fun suppliedPipeRelease(supplied_pipe: RawSuppliedPipe): RawUnit
    fun suppliedPipeOpenRequestProvide(supplied_pipe_open_request: RawSuppliedPipeOpenRequest, descriptor: Long): RawCallResult<Boolean>
    fun suppliedPipeOpenRequestDecline(supplied_pipe_open_request: RawSuppliedPipeOpenRequest): RawCallResult<Boolean>
    fun suppliedPipeOpenRequestRelease(supplied_pipe_open_request: RawSuppliedPipeOpenRequest): RawUnit
    fun hostBeginResourceUpload(host: RawHost, linkId: LinkId, declaredLength: ULong, packedMetadata: Bytes?, compression: ResourceCompression): RawCallResult<RawOwned<RawResourceUpload>>
    fun resourceUploadWrite(resource_upload: RawResourceUpload, chunk: Bytes): RawCallResult<RawUnit>
    fun resourceUploadIsWritable(resource_upload: RawResourceUpload): RawCallResult<Boolean>
    fun resourceUploadFinish(resource_upload: RawResourceUpload): RawCallResult<RawOwned<RawIssuedCommand>>
    fun resourceUploadAbort(resource_upload: RawResourceUpload): RawUnit
    fun resourceUploadRelease(resource_upload: RawResourceUpload): RawUnit
    fun hostStop(host: RawHost): RawCallResult<RawUnit>
    fun commandWait(issued_command: RawIssuedCommand, timeoutMillis: Long): RawCallResult<RawBorrowed<RawCommandResult>>
    fun commandRegisterReadiness(issued_command: RawIssuedCommand, callback: RawReadinessCallback, context: RawOpaquePointer): RawCallResult<RawOwned<RawReadinessRegistration>>
    fun commandInterruptWait(issued_command: RawIssuedCommand): RawUnit
    fun commandRelease(issued_command: RawIssuedCommand): RawUnit
    fun hostClaimApplicationEvents(host: RawHost): RawCallResult<RawOwned<RawEventStream>>
    fun hostClaimDiagnostics(host: RawHost): RawCallResult<RawOwned<RawEventStream>>
    fun eventStreamRegisterReadiness(event_stream: RawEventStream, callback: RawReadinessCallback, context: RawOpaquePointer): RawCallResult<RawOwned<RawReadinessRegistration>>
    fun readinessRegistrationRelease(readiness_registration: RawReadinessRegistration): RawUnit
    fun eventStreamInterruptWait(event_stream: RawEventStream): RawUnit
    fun eventStreamRelease(event_stream: RawEventStream): RawUnit
    fun eventStreamNext(event_stream: RawEventStream, timeoutMillis: Long): RawCallResult<RawOwned<RawEvent>>
    fun eventRelease(event: RawEvent): RawUnit
    fun eventKind(event: RawEvent): Long
    fun eventBytes(event: RawEvent, field: EventField): RawCallResult<RawBorrowed<Bytes>>
    fun eventString(event: RawEvent, field: EventField): RawCallResult<RawBorrowed<String>>
    fun eventU64(event: RawEvent, field: EventField): RawCallResult<ULong>
    fun eventU128(event: RawEvent, field: EventField): RawCallResult<BigInteger>
    fun eventResourceStream(event: RawEvent): RawCallResult<RawOwned<RawResourceStream>>
    fun resourceStreamRelease(resource_stream: RawResourceStream): RawUnit
    fun resourceStreamNext(resource_stream: RawResourceStream, maximumBytes: Long): RawCallResult<RawBorrowed<RawResourceChunk>>
    fun hostAnnounce(host: RawHost, destination: DestinationHash, `interface`: InterfaceId?): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostSendSinglePacket(host: RawHost, destination: DestinationHash, payload: Bytes): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostCloseLink(host: RawHost, linkId: LinkId): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostAttachTcpServer(host: RawHost, bind: String, bitrate: Bitrate): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostAttachTcpClient(host: RawHost, target: String, bitrate: Bitrate): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostAttachUdp(host: RawHost, local: String, peer: String, bitrate: Bitrate): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostDetachInterface(host: RawHost, `interface`: InterfaceId): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostEstablishLink(host: RawHost, destination: DestinationHash): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostRequestPath(host: RawHost, destination: DestinationHash): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostIdentify(host: RawHost, linkId: LinkId, identity: IdentityHash): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostSendLinkPacket(host: RawHost, linkId: LinkId, payload: Bytes): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostRequest(host: RawHost, linkId: LinkId, pathHash: RequestPathHash, payload: Bytes, timeout: ResponseTimeout, maximumResponseBytes: Long?): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostRespond(host: RawHost, linkId: LinkId, requestId: RequestId, requestRttMillis: Long, payload: Bytes): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostSendResource(host: RawHost, linkId: LinkId, payload: Bytes, packedMetadata: Bytes?, compression: ResourceCompression): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostSetLinkResourceStrategy(host: RawHost, linkId: LinkId, strategy: ResourceStrategy): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostSetDestinationResourceStrategy(host: RawHost, destination: DestinationHash, strategy: ResourceStrategy): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostSendChannelMessage(host: RawHost, linkId: LinkId, messageType: Int, payload: Bytes): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostAllowRequester(host: RawHost, destination: DestinationHash, pathHash: RequestPathHash, identity: IdentityHash): RawCallResult<RawOwned<RawIssuedCommand>>
    fun hostAttachInterface(host: RawHost, config: InterfaceConfig, routing: InterfaceRoutingPolicy?): RawCallResult<RawOwned<RawIssuedCommand>>
}
