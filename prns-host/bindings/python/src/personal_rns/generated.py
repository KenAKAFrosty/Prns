from __future__ import annotations

from dataclasses import dataclass
from enum import IntEnum
from typing import Any, Generic, Protocol, TypeAlias, TypeVar

HOST_CONTRACT_ABI = 1
SCHEMA_VERSION = 1
PRODUCT_VERSION = "0.3.7"
DESTINATION_HASH_LENGTH = 16
IDENTITY_HASH_LENGTH = 16
INTERFACE_ID_LENGTH = 8
LINK_ID_LENGTH = 16
PACKET_HASH_LENGTH = 32
REQUEST_ID_LENGTH = 16
REQUEST_PATH_HASH_LENGTH = 16
RESOURCE_HASH_LENGTH = 32
IDENTITY_SECRET_LENGTH = 64
SAFE_INT_MIN = -9007199254740991
SAFE_INT_MAX = 9007199254740991
SAFE_UINT_MAX = 9007199254740991
BALANCED_PENDING_COMMANDS = 256
BALANCED_APPLICATION_EVENTS = 1024
BALANCED_RETAINED_EVENT_BYTES = 8388608
BALANCED_DIAGNOSTICS = 1024

class Status(IntEnum):
    OK = 0
    INVALID_ARGUMENT = 1
    CONTRACT_MISMATCH = 2
    INVALID_HANDLE = 3
    NOT_READY = 4
    ALREADY_CLAIMED = 5
    WOULD_BLOCK = 6
    TIMED_OUT = 7
    QUEUE_FULL = 8
    STOPPED = 9
    BACKEND_FAILED = 10
    PANIC = 11
    INTERRUPTED = 12
    UNSUPPORTED = 13
    PERMISSION_DENIED = 14
    UNAVAILABLE = 15

class BackendKind(IntEnum):
    NATIVE = 1
    BROWSER = 2
    COOPERATIVE = 3

class Capability(IntEnum):
    LOOPBACK = 1
    TCP_CLIENT = 2
    TCP_SERVER = 3
    UDP = 4
    SERIAL = 5
    USB = 6
    BLUETOOTH = 7
    WIFI = 8
    WEB_SOCKET = 9
    BROWSER_RENDEZVOUS = 10
    I2P = 11
    WEAVE = 12
    SUPPLIED_PIPE = 13

class InterfaceKind(IntEnum):
    AUTO_LAN = 1
    TCP_CLIENT = 2
    TCP_SERVER = 3
    UDP = 4
    SERIAL = 5
    KISS = 6
    AX25_KISS = 7
    R_NODE = 8
    MULTI_R_NODE = 9
    PIPE = 10
    BACKBONE_CLIENT = 11
    BACKBONE_SERVER = 12
    I2P = 13
    WEAVE = 14
    AUTOMATIC_USB = 15
    AUTOMATIC_BLUETOOTH_LE = 16
    WEB_SOCKET_CLIENT = 17
    WEB_SOCKET_SERVER = 18
    BROWSER_RENDEZVOUS = 19

class InterfaceMode(IntEnum):
    FULL = 1
    POINT_TO_POINT = 2
    ACCESS_POINT = 3
    ROAMING = 4
    BOUNDARY = 5
    GATEWAY = 6
    INTERNAL = 7

class WebSocketFramingSelection(IntEnum):
    RAW_PACKET = 1
    HDLC = 2
    KISS = 3
    AUTO = 4

class InterfaceHealth(IntEnum):
    INITIALIZING = 1
    CONNECTED = 2
    DEGRADED = 3
    RECONNECTING = 4
    FAILED = 5
    DISCONNECTED = 6
    DISABLED = 7
    UNKNOWN = 8

class DiscoveryScope(IntEnum):
    LINK = 1
    ADMIN = 2
    SITE = 3
    ORGANIZATION = 4
    GLOBAL = 5

class MulticastAddressType(IntEnum):
    TEMPORARY = 1
    PERMANENT = 2

class SerialDataBits(IntEnum):
    FIVE = 5
    SIX = 6
    SEVEN = 7
    EIGHT = 8

class SerialParity(IntEnum):
    NONE = 1
    EVEN = 2
    ODD = 3

class SerialStopBits(IntEnum):
    ONE = 1
    TWO = 2

class HostRole(IntEnum):
    ENDPOINT = 1
    TRANSPORT = 2

class IdentityConfigKind(IntEnum):
    EXISTING = 1
    GENERATE_EPHEMERAL = 2
    LOAD_OR_CREATE = 3

class PersistenceConfigKind(IntEnum):
    EPHEMERAL = 1
    DIRECTORY = 2

class DestinationConfigKind(IntEnum):
    PLAIN = 1
    SINGLE = 2

class DestinationIdentityConfigKind(IntEnum):
    HOST_IDENTITY = 1
    DEDICATED_IDENTITY = 2

class BitrateKind(IntEnum):
    AUTO = 1
    BITS_PER_SECOND = 2

class ResponseTimeoutKind(IntEnum):
    LINK_DEFAULT = 1
    EXACT = 2

class ResourceCompressionKind(IntEnum):
    AUTO = 1
    NEVER = 2

class ResourceStrategyKind(IntEnum):
    REFUSE = 1
    ACCEPT = 2

class RequestPolicy(IntEnum):
    ALLOW_NONE = 1
    ALLOW_ALL = 2
    ALLOW_LIST = 3

class CommandOutcomeKind(IntEnum):
    ANNOUNCED = 1
    PACKET_DELIVERED = 2
    LINK_CLOSE_QUEUED = 3
    INTERFACE_ATTACHED = 4
    INTERFACE_DETACHED = 5
    LINK_ESTABLISHED = 6
    PATH_DISCOVERED = 7
    IDENTIFIED = 8
    RESPONSE_RECEIVED = 9
    RESPONSE_SENT = 10
    RESOURCE_SENT = 11
    RESOURCE_STRATEGY_SET = 12
    REQUESTER_ALLOWED = 13

class CommandFailureKind(IntEnum):
    NODE_STOPPED = 1
    BUSY = 2
    PAYLOAD_TOO_LARGE = 3
    UNKNOWN_DESTINATION = 4
    NOT_SINGLE_DESTINATION = 5
    ANNOUNCE_APP_DATA_TOO_LONG = 6
    UNKNOWN_INTERFACE = 7
    NO_ROUTE_TO_DESTINATION = 8
    NOT_DIRECTLY_REACHABLE = 9
    PACKET_CULLED = 10
    DELIVERY_TIMED_OUT = 11
    INVALID_BITRATE = 12
    BIND_FAILED = 13
    WRITE_FAILED = 14
    UNSUPPORTED_BY_BACKEND = 15
    UNKNOWN_LINK = 16
    LINK_NOT_ACTIVE = 17
    ENTROPY_UNAVAILABLE = 18
    NOT_LINK_INITIATOR = 19
    IDENTITY_NOT_HELD = 20
    UNKNOWN_REQUEST_HANDLER = 21
    REQUEST_POLICY_NOT_ALLOW_LIST = 22
    REQUEST_ALLOW_LIST_FULL = 23
    LINK_BUSY = 24
    RESOURCE_TABLE_FULL = 25
    RESOURCE_METADATA_TOO_LARGE = 26
    RESOURCE_REJECTED_BY_PEER = 27
    RESOURCE_SEQUENCING_FAILED = 28
    RESOURCE_PREDECESSOR_FAILED = 29
    CHANNEL_WINDOW_FULL = 30
    CHANNEL_UNTRACKABLE = 31
    INVALID_CHANNEL_MESSAGE_TYPE = 32
    INVALID_CONFIGURATION = 33
    RESOURCE_UPLOAD_CANCELLED = 34
    RESOURCE_EARLY_EOF = 35
    RESOURCE_LENGTH_OVERRUN = 36
    PERMISSION_DENIED = 37
    DEVICE_UNAVAILABLE = 38
    CONNECT_FAILED = 39
    BACKEND_FAILED = 40
    RESPONSE_TOO_LARGE = 41

class DeliveryEvidenceKind(IntEnum):
    EXPLICIT_PROOF = 1
    IMPLICIT_PROOF = 2
    RESPONSE = 3

class LifecyclePhase(IntEnum):
    STARTING = 1
    RUNNING = 2
    STOPPING = 3
    STOPPED = 4
    FAILED = 5

class StopReason(IntEnum):
    REQUESTED = 1
    BACKEND_EXITED = 2

class LinkClosedReason(IntEnum):
    TIMEOUT = 1
    PEER_CLOSED = 2
    MALFORMED_RTT = 3

class ApplicationEventKind(IntEnum):
    SINGLE_DELIVERY = 100
    REQUEST = 101
    RESPONSE = 102
    RESPONSE_SEGMENT = 103
    RESOURCE_AVAILABLE = 104
    RESOURCE_SEGMENT = 105
    RESOURCE_NEEDS_DECOMPRESSION = 106
    CHANNEL_MESSAGE = 107
    LINK_DELIVERY = 108

class DiagnosticEventKind(IntEnum):
    ANNOUNCE_HEARD = 200
    LINK_ESTABLISHED = 201
    PEER_IDENTIFIED = 202
    LINK_CLOSED = 203
    LINK_INTERFACE_MISMATCH = 204
    RESOURCE_ASSEMBLED = 205
    RESOURCE_FAILED = 206
    RESOURCE_SEND_PROGRESS = 207
    SELF_RATCHET_ROTATED = 208
    ANNOUNCE_HELD_DROPPED = 209
    DELIVERED = 210
    ROUTE_EXPIRED = 211
    ROUTE_EVICTED = 212
    ROUTE_INTERFACE_GONE = 213
    ROUTE_DROPPED = 214
    BACKEND_DIAGNOSTIC = 215
    DIAGNOSTICS_DROPPED = 216
    PERSISTENCE_RESTORED = 217
    PERSISTENCE_FLUSHED = 218
    PERSISTENCE_FLUSH_FAILED = 219

class PersistenceFlushCause(IntEnum):
    STARTUP = 1
    INTERVAL = 2
    ROUTE_CHANGE = 3
    RATCHET_ROTATION = 4
    SHUTDOWN = 5

class PersistenceFlushTarget(IntEnum):
    ROUTING_STATE = 1
    RATCHETS = 2

class EventField(IntEnum):
    DESTINATION = 1
    SOURCE_INTERFACE = 2
    PLAINTEXT = 3
    LINK_ID = 4
    REQUEST_ID = 5
    REQUESTER = 6
    PATH_HASH = 7
    RTT_MILLIS = 8
    DATA = 9
    SEGMENT_INDEX = 10
    TOTAL_SEGMENTS = 11
    HASH = 12
    ORIGINAL_HASH = 13
    METADATA = 14
    TOTAL_BYTES = 15
    STREAM_ID = 16
    UNCOMPRESSED_DATA_BYTES = 17
    MESSAGE_TYPE = 18
    IDENTITY = 19
    REASON = 20
    ATTACHED_INTERFACE = 21
    ARRIVED_ON = 22
    TOTAL_SIZE_BYTES = 23
    CAUSE = 24
    TRANSFERRED_BYTES = 25
    PHYSICAL_TRANSFERRED_BYTES = 26
    DETAIL = 27
    KIND = 28
    DROPPED_COUNT = 29
    HOPS = 30
    STREAM = 31
    ROUTES = 32
    DESTINATION_IDENTITIES = 33
    TUNNELS = 34
    RATCHETS = 35
    REFUSED = 36
    DROPPED = 37
    PERSISTENCE_CAUSE = 38
    PERSISTENCE_TARGET = 39
    APP_DATA = 40

@dataclass(frozen=True, slots=True)
class DestinationHash:
    value: bytes

    def __post_init__(self):
        value = bytes(self.value)
        if len(value) != 16:
            raise ValueError("DestinationHash requires exactly 16 bytes")
        object.__setattr__(self, "value", value)

@dataclass(frozen=True, slots=True)
class IdentityHash:
    value: bytes

    def __post_init__(self):
        value = bytes(self.value)
        if len(value) != 16:
            raise ValueError("IdentityHash requires exactly 16 bytes")
        object.__setattr__(self, "value", value)

@dataclass(frozen=True, slots=True)
class InterfaceId:
    value: bytes

    def __post_init__(self):
        value = bytes(self.value)
        if len(value) != 8:
            raise ValueError("InterfaceId requires exactly 8 bytes")
        object.__setattr__(self, "value", value)

@dataclass(frozen=True, slots=True)
class LinkId:
    value: bytes

    def __post_init__(self):
        value = bytes(self.value)
        if len(value) != 16:
            raise ValueError("LinkId requires exactly 16 bytes")
        object.__setattr__(self, "value", value)

@dataclass(frozen=True, slots=True)
class PacketHash:
    value: bytes

    def __post_init__(self):
        value = bytes(self.value)
        if len(value) != 32:
            raise ValueError("PacketHash requires exactly 32 bytes")
        object.__setattr__(self, "value", value)

@dataclass(frozen=True, slots=True)
class RequestId:
    value: bytes

    def __post_init__(self):
        value = bytes(self.value)
        if len(value) != 16:
            raise ValueError("RequestId requires exactly 16 bytes")
        object.__setattr__(self, "value", value)

@dataclass(frozen=True, slots=True)
class RequestPathHash:
    value: bytes

    def __post_init__(self):
        value = bytes(self.value)
        if len(value) != 16:
            raise ValueError("RequestPathHash requires exactly 16 bytes")
        object.__setattr__(self, "value", value)

@dataclass(frozen=True, slots=True)
class ResourceHash:
    value: bytes

    def __post_init__(self):
        value = bytes(self.value)
        if len(value) != 32:
            raise ValueError("ResourceHash requires exactly 32 bytes")
        object.__setattr__(self, "value", value)

class IdentitySecret:
    __slots__ = ("_value",)

    def __init__(self, value: bytes | bytearray):
        value = bytearray(value)
        if len(value) != 64:
            raise ValueError("IdentitySecret requires exactly 64 bytes")
        self._value = value

    @property
    def value(self) -> bytes:
        return bytes(self._value)

    def _view(self) -> memoryview:
        return memoryview(self._value).toreadonly()

    def close(self) -> None:
        for index in range(len(self._value)):
            self._value[index] = 0

    def __del__(self):
        self.close()

    def __enter__(self):
        return self

    def __exit__(self, _type, _value, _traceback):
        self.close()

@dataclass(frozen=True, slots=True)
class DestinationName:
    app_name: str
    aspects: tuple[str, ...]

    def __post_init__(self):
        if not self.app_name or not self.aspects or any(not value for value in self.aspects):
            raise ValueError("a destination requires a non-empty app name and aspects")

@dataclass(frozen=True, slots=True)
class RequestHandlerConfig:
    path: str
    policy: RequestPolicy

@dataclass(frozen=True, slots=True)
class SerialLineConfig:
    baud: int
    data_bits: SerialDataBits
    parity: SerialParity
    stop_bits: SerialStopBits

@dataclass(frozen=True, slots=True)
class RNodeRadioConfig:
    frequency_hz: int
    bandwidth_hz: int
    tx_power_dbm: int
    spreading_factor: int
    coding_rate: int

@dataclass(frozen=True, slots=True)
class MultiRNodeMemberConfig:
    name: str
    virtual_port: int
    radio: RNodeRadioConfig
    flow_control: bool
    outgoing: bool

@dataclass(frozen=True, slots=True)
class InterfaceRoutingPolicy:
    mode: InterfaceMode | None
    gravity: int | None
    recursive_path_requests: bool | None
    announces_from_internal: bool | None
    announces_to_internal: bool | None

@dataclass(frozen=True, slots=True)
class BackendInfo:
    backend: BackendKind
    capabilities: tuple[Capability, ...]
    interface_kinds: tuple[InterfaceKind, ...]

@dataclass(frozen=True, slots=True)
class InterfaceSnapshot:
    interface_id: InterfaceId
    name: str | None
    kind: InterfaceKind | None
    health: InterfaceHealth
    failure_detail: str | None
    rx_bytes: int
    tx_bytes: int
    rx_bps: int | None
    tx_bps: int | None
    route_count: int
    link_count: int
    transported_link_count: int

@dataclass(frozen=True, slots=True)
class RouteSnapshot:
    destination: DestinationHash
    hops: int
    via_identity: IdentityHash | None
    interface_id: InterfaceId
    learned_at_millis: int
    last_route_activity_at_millis: int
    expires_at_millis: int

@dataclass(frozen=True, slots=True)
class DestinationIdentitySnapshot:
    destination: DestinationHash
    identity: IdentityHash

@dataclass(frozen=True, slots=True)
class RuntimeHealthSnapshot:
    running: bool
    uptime_millis: int
    interface_count: int
    online_interface_count: int
    route_count: int
    link_count: int
    transported_link_count: int
    rx_bytes: int
    tx_bytes: int
    rx_bps: int
    tx_bps: int

@dataclass(frozen=True, slots=True)
class PersistenceSnapshot:
    persistent: bool
    restored: bool
    last_flush_cause: PersistenceFlushCause | None
    last_failure_detail: str | None

@dataclass(frozen=True, slots=True)
class HostSnapshot:
    revision: int
    backend: BackendInfo
    interfaces: tuple[InterfaceSnapshot, ...]
    routes: tuple[RouteSnapshot, ...]
    active_link_count: int
    destination_identities: tuple[DestinationIdentitySnapshot, ...]
    runtime: RuntimeHealthSnapshot
    persistence: PersistenceSnapshot

@dataclass(frozen=True, slots=True)
class IdentityConfigExisting:
    secret: IdentitySecret

@dataclass(frozen=True, slots=True)
class IdentityConfigGenerateEphemeral:
    pass

@dataclass(frozen=True, slots=True)
class IdentityConfigLoadOrCreate:
    path: str

@dataclass(frozen=True, slots=True)
class PersistenceConfigEphemeral:
    pass

@dataclass(frozen=True, slots=True)
class PersistenceConfigDirectory:
    path: str

@dataclass(frozen=True, slots=True)
class InterfaceConfigAutoLan:
    group_id: str | None
    discovery_scope: DiscoveryScope | None
    discovery_port: int | None
    data_port: int | None
    devices: tuple[str, ...]
    ignored_devices: tuple[str, ...]
    multicast_address_type: MulticastAddressType | None

@dataclass(frozen=True, slots=True)
class InterfaceConfigTcpClient:
    target: str
    bitrate: Bitrate

@dataclass(frozen=True, slots=True)
class InterfaceConfigTcpServer:
    bind: str
    bitrate: Bitrate

@dataclass(frozen=True, slots=True)
class InterfaceConfigUdp:
    local: str
    peer: str
    bitrate: Bitrate

@dataclass(frozen=True, slots=True)
class InterfaceConfigSerial:
    port: str
    line: SerialLineConfig

@dataclass(frozen=True, slots=True)
class InterfaceConfigKiss:
    port: str
    line: SerialLineConfig
    flow_control: bool
    preamble_millis: int
    transmit_tail_millis: int
    persistence: int
    slot_time_millis: int
    station_callsign: str | None
    station_interval_seconds: int | None

@dataclass(frozen=True, slots=True)
class InterfaceConfigAx25Kiss:
    port: str
    line: SerialLineConfig
    flow_control: bool
    preamble_millis: int
    transmit_tail_millis: int
    persistence: int
    slot_time_millis: int
    callsign: str
    ssid: int

@dataclass(frozen=True, slots=True)
class InterfaceConfigRNode:
    port: str
    radio: RNodeRadioConfig
    flow_control: bool
    station_callsign: str | None
    station_interval_seconds: int | None
    airtime_limit_short_centi_percent: int | None
    airtime_limit_long_centi_percent: int | None

@dataclass(frozen=True, slots=True)
class InterfaceConfigMultiRNode:
    port: str
    station_callsign: str | None
    station_interval_seconds: int | None
    members: tuple[MultiRNodeMemberConfig, ...]

@dataclass(frozen=True, slots=True)
class InterfaceConfigPipe:
    command: tuple[str, ...]
    respawn_delay_millis: int

@dataclass(frozen=True, slots=True)
class InterfaceConfigBackboneClient:
    target: str
    bitrate: Bitrate

@dataclass(frozen=True, slots=True)
class InterfaceConfigBackboneServer:
    bind: str
    bitrate: Bitrate

@dataclass(frozen=True, slots=True)
class InterfaceConfigI2p:
    peers: tuple[str, ...]
    connectable: bool

@dataclass(frozen=True, slots=True)
class InterfaceConfigWeave:
    port: str

@dataclass(frozen=True, slots=True)
class InterfaceConfigAutomaticUsb:
    pass

@dataclass(frozen=True, slots=True)
class InterfaceConfigAutomaticBluetoothLe:
    pass

@dataclass(frozen=True, slots=True)
class InterfaceConfigWebSocketClient:
    target: str
    framing: WebSocketFramingSelection

@dataclass(frozen=True, slots=True)
class InterfaceConfigWebSocketServer:
    bind: str
    framing: WebSocketFramingSelection

@dataclass(frozen=True, slots=True)
class InterfaceConfigBrowserRendezvous:
    url: str

@dataclass(frozen=True, slots=True)
class DestinationIdentityConfigHostIdentity:
    pass

@dataclass(frozen=True, slots=True)
class DestinationIdentityConfigDedicatedIdentity:
    identity: IdentityConfig

@dataclass(frozen=True, slots=True)
class BitrateAuto:
    pass

@dataclass(frozen=True, slots=True)
class BitrateBitsPerSecond:
    value: int

@dataclass(frozen=True, slots=True)
class ResponseTimeoutLinkDefault:
    pass

@dataclass(frozen=True, slots=True)
class ResponseTimeoutExact:
    millis: int

@dataclass(frozen=True, slots=True)
class ResourceCompressionAuto:
    pass

@dataclass(frozen=True, slots=True)
class ResourceCompressionNever:
    pass

@dataclass(frozen=True, slots=True)
class ResourceStrategyRefuse:
    pass

@dataclass(frozen=True, slots=True)
class ResourceStrategyAccept:
    maximum_uncompressed_bytes: int
    accept_compressed: bool

@dataclass(frozen=True, slots=True)
class DestinationConfigPlain:
    name: DestinationName

@dataclass(frozen=True, slots=True)
class DestinationConfigSingle:
    name: DestinationName
    identity: DestinationIdentityConfig
    announce_app_data: bytes | None
    maximum_request_bytes: int | None
    request_handlers: tuple[RequestHandlerConfig, ...]

@dataclass(frozen=True, slots=True)
class HostCommandAnnounce:
    destination: DestinationHash
    interface: InterfaceId | None

@dataclass(frozen=True, slots=True)
class HostCommandSendSinglePacket:
    destination: DestinationHash
    payload: bytes

@dataclass(frozen=True, slots=True)
class HostCommandCloseLink:
    link_id: LinkId

@dataclass(frozen=True, slots=True)
class HostCommandAttachTcpServer:
    bind: str
    bitrate: Bitrate

@dataclass(frozen=True, slots=True)
class HostCommandAttachTcpClient:
    target: str
    bitrate: Bitrate

@dataclass(frozen=True, slots=True)
class HostCommandAttachUdp:
    local: str
    peer: str
    bitrate: Bitrate

@dataclass(frozen=True, slots=True)
class HostCommandDetachInterface:
    interface: InterfaceId

@dataclass(frozen=True, slots=True)
class HostCommandEstablishLink:
    destination: DestinationHash

@dataclass(frozen=True, slots=True)
class HostCommandRequestPath:
    destination: DestinationHash

@dataclass(frozen=True, slots=True)
class HostCommandIdentify:
    link_id: LinkId
    identity: IdentityHash

@dataclass(frozen=True, slots=True)
class HostCommandSendLinkPacket:
    link_id: LinkId
    payload: bytes

@dataclass(frozen=True, slots=True)
class HostCommandRequest:
    link_id: LinkId
    path_hash: RequestPathHash
    payload: bytes
    timeout: ResponseTimeout
    maximum_response_bytes: int | None

@dataclass(frozen=True, slots=True)
class HostCommandRespond:
    link_id: LinkId
    request_id: RequestId
    request_rtt_millis: int
    payload: bytes

@dataclass(frozen=True, slots=True)
class HostCommandSendResource:
    link_id: LinkId
    payload: bytes
    packed_metadata: bytes | None
    compression: ResourceCompression

@dataclass(frozen=True, slots=True)
class HostCommandSetLinkResourceStrategy:
    link_id: LinkId
    strategy: ResourceStrategy

@dataclass(frozen=True, slots=True)
class HostCommandSetDestinationResourceStrategy:
    destination: DestinationHash
    strategy: ResourceStrategy

@dataclass(frozen=True, slots=True)
class HostCommandSendChannelMessage:
    link_id: LinkId
    message_type: int
    payload: bytes

@dataclass(frozen=True, slots=True)
class HostCommandAllowRequester:
    destination: DestinationHash
    path_hash: RequestPathHash
    identity: IdentityHash

@dataclass(frozen=True, slots=True)
class HostCommandAttachInterface:
    config: InterfaceConfig
    routing: InterfaceRoutingPolicy | None

@dataclass(frozen=True, slots=True)
class CommandOutcomeAnnounced:
    pass

@dataclass(frozen=True, slots=True)
class CommandOutcomePacketDelivered:
    rtt_millis: int
    evidence: DeliveryEvidenceKind
    packet_hash: PacketHash | None

@dataclass(frozen=True, slots=True)
class CommandOutcomeLinkCloseQueued:
    pass

@dataclass(frozen=True, slots=True)
class CommandOutcomeInterfaceAttached:
    interface: InterfaceId

@dataclass(frozen=True, slots=True)
class CommandOutcomeInterfaceDetached:
    interface: InterfaceId

@dataclass(frozen=True, slots=True)
class CommandOutcomeLinkEstablished:
    link_id: LinkId
    rtt_millis: int

@dataclass(frozen=True, slots=True)
class CommandOutcomePathDiscovered:
    hops: int

@dataclass(frozen=True, slots=True)
class CommandOutcomeIdentified:
    pass

@dataclass(frozen=True, slots=True)
class CommandOutcomeResponseReceived:
    data: bytes
    rtt_millis: int

@dataclass(frozen=True, slots=True)
class CommandOutcomeResponseSent:
    rtt_millis: int

@dataclass(frozen=True, slots=True)
class CommandOutcomeResourceSent:
    pass

@dataclass(frozen=True, slots=True)
class CommandOutcomeResourceStrategySet:
    pass

@dataclass(frozen=True, slots=True)
class CommandOutcomeRequesterAllowed:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureNodeStopped:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureBusy:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailurePayloadTooLarge:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureUnknownDestination:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureNotSingleDestination:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureAnnounceAppDataTooLong:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureUnknownInterface:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureNoRouteToDestination:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureNotDirectlyReachable:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailurePacketCulled:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureDeliveryTimedOut:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureInvalidBitrate:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureBindFailed:
    detail: str

@dataclass(frozen=True, slots=True)
class CommandFailureWriteFailed:
    detail: str

@dataclass(frozen=True, slots=True)
class CommandFailureUnsupportedByBackend:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureUnknownLink:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureLinkNotActive:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureEntropyUnavailable:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureNotLinkInitiator:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureIdentityNotHeld:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureUnknownRequestHandler:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureRequestPolicyNotAllowList:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureRequestAllowListFull:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureLinkBusy:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureResourceTableFull:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureResourceMetadataTooLarge:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureResourceRejectedByPeer:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureResourceSequencingFailed:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureResourcePredecessorFailed:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureChannelWindowFull:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureChannelUntrackable:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureInvalidChannelMessageType:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureInvalidConfiguration:
    detail: str

@dataclass(frozen=True, slots=True)
class CommandFailureResourceUploadCancelled:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureResourceEarlyEof:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailureResourceLengthOverrun:
    pass

@dataclass(frozen=True, slots=True)
class CommandFailurePermissionDenied:
    detail: str

@dataclass(frozen=True, slots=True)
class CommandFailureDeviceUnavailable:
    detail: str

@dataclass(frozen=True, slots=True)
class CommandFailureConnectFailed:
    detail: str

@dataclass(frozen=True, slots=True)
class CommandFailureBackendFailed:
    detail: str

@dataclass(frozen=True, slots=True)
class CommandFailureResponseTooLarge:
    pass

@dataclass(frozen=True, slots=True)
class ApplicationEventSingleDelivery:
    destination: DestinationHash
    source_interface: InterfaceId
    plaintext: bytes

@dataclass(frozen=True, slots=True)
class ApplicationEventRequest:
    destination: DestinationHash
    link_id: LinkId
    request_id: RequestId
    requester: IdentityHash | None
    path_hash: RequestPathHash
    rtt_millis: int
    data: bytes

@dataclass(frozen=True, slots=True)
class ApplicationEventResponse:
    link_id: LinkId
    request_id: RequestId
    data: bytes

@dataclass(frozen=True, slots=True)
class ApplicationEventResponseSegment:
    link_id: LinkId
    request_id: RequestId
    segment_index: int
    total_segments: int
    data: bytes

@dataclass(frozen=True, slots=True)
class ApplicationEventResourceAvailable:
    link_id: LinkId
    hash: ResourceHash
    metadata: bytes | None
    resource: Any

@dataclass(frozen=True, slots=True)
class ApplicationEventResourceSegment:
    link_id: LinkId
    original_hash: ResourceHash
    segment_index: int
    total_segments: int
    metadata: bytes | None
    data: bytes

@dataclass(frozen=True, slots=True)
class ApplicationEventResourceNeedsDecompression:
    link_id: LinkId
    hash: ResourceHash
    stream: bytes
    uncompressed_data_bytes: int

@dataclass(frozen=True, slots=True)
class ApplicationEventChannelMessage:
    link_id: LinkId
    message_type: int
    data: bytes

@dataclass(frozen=True, slots=True)
class ApplicationEventLinkDelivery:
    link_id: LinkId
    source_interface: InterfaceId
    plaintext: bytes

@dataclass(frozen=True, slots=True)
class DiagnosticEventAnnounceHeard:
    destination: DestinationHash
    hops: int
    source_interface: InterfaceId
    app_data: bytes

@dataclass(frozen=True, slots=True)
class DiagnosticEventLinkEstablished:
    link_id: LinkId
    rtt_millis: int

@dataclass(frozen=True, slots=True)
class DiagnosticEventPeerIdentified:
    link_id: LinkId
    identity: IdentityHash

@dataclass(frozen=True, slots=True)
class DiagnosticEventLinkClosed:
    link_id: LinkId
    reason: LinkClosedReason

@dataclass(frozen=True, slots=True)
class DiagnosticEventLinkInterfaceMismatch:
    link_id: LinkId
    attached_interface: InterfaceId
    arrived_on: InterfaceId

@dataclass(frozen=True, slots=True)
class DiagnosticEventResourceAssembled:
    link_id: LinkId
    original_hash: ResourceHash
    total_size_bytes: int

@dataclass(frozen=True, slots=True)
class DiagnosticEventResourceFailed:
    link_id: LinkId
    hash: ResourceHash
    cause: str

@dataclass(frozen=True, slots=True)
class DiagnosticEventResourceSendProgress:
    link_id: LinkId
    transferred_bytes: int
    total_bytes: int
    physical_transferred_bytes: int
    segment_index: int
    total_segments: int

@dataclass(frozen=True, slots=True)
class DiagnosticEventSelfRatchetRotated:
    destination: DestinationHash

@dataclass(frozen=True, slots=True)
class DiagnosticEventAnnounceHeldDropped:
    destination: DestinationHash
    source_interface: InterfaceId
    cause: str

@dataclass(frozen=True, slots=True)
class DiagnosticEventDelivered:
    detail: str

@dataclass(frozen=True, slots=True)
class DiagnosticEventRouteExpired:
    destination: DestinationHash

@dataclass(frozen=True, slots=True)
class DiagnosticEventRouteEvicted:
    destination: DestinationHash

@dataclass(frozen=True, slots=True)
class DiagnosticEventRouteInterfaceGone:
    destination: DestinationHash

@dataclass(frozen=True, slots=True)
class DiagnosticEventRouteDropped:
    destination: DestinationHash

@dataclass(frozen=True, slots=True)
class DiagnosticEventBackendDiagnostic:
    kind: str
    detail: str

@dataclass(frozen=True, slots=True)
class DiagnosticEventDiagnosticsDropped:
    count: int

@dataclass(frozen=True, slots=True)
class DiagnosticEventPersistenceRestored:
    routes: int
    destination_identities: int
    tunnels: int
    ratchets: int
    refused: int
    dropped: int

@dataclass(frozen=True, slots=True)
class DiagnosticEventPersistenceFlushed:
    cause: PersistenceFlushCause
    target: PersistenceFlushTarget

@dataclass(frozen=True, slots=True)
class DiagnosticEventPersistenceFlushFailed:
    cause: PersistenceFlushCause
    target: PersistenceFlushTarget

IdentityConfig: TypeAlias = IdentityConfigExisting | IdentityConfigGenerateEphemeral | IdentityConfigLoadOrCreate
PersistenceConfig: TypeAlias = PersistenceConfigEphemeral | PersistenceConfigDirectory
InterfaceConfig: TypeAlias = InterfaceConfigAutoLan | InterfaceConfigTcpClient | InterfaceConfigTcpServer | InterfaceConfigUdp | InterfaceConfigSerial | InterfaceConfigKiss | InterfaceConfigAx25Kiss | InterfaceConfigRNode | InterfaceConfigMultiRNode | InterfaceConfigPipe | InterfaceConfigBackboneClient | InterfaceConfigBackboneServer | InterfaceConfigI2p | InterfaceConfigWeave | InterfaceConfigAutomaticUsb | InterfaceConfigAutomaticBluetoothLe | InterfaceConfigWebSocketClient | InterfaceConfigWebSocketServer | InterfaceConfigBrowserRendezvous
DestinationIdentityConfig: TypeAlias = DestinationIdentityConfigHostIdentity | DestinationIdentityConfigDedicatedIdentity
Bitrate: TypeAlias = BitrateAuto | BitrateBitsPerSecond
ResponseTimeout: TypeAlias = ResponseTimeoutLinkDefault | ResponseTimeoutExact
ResourceCompression: TypeAlias = ResourceCompressionAuto | ResourceCompressionNever
ResourceStrategy: TypeAlias = ResourceStrategyRefuse | ResourceStrategyAccept
DestinationConfig: TypeAlias = DestinationConfigPlain | DestinationConfigSingle
HostCommand: TypeAlias = HostCommandAnnounce | HostCommandSendSinglePacket | HostCommandCloseLink | HostCommandAttachTcpServer | HostCommandAttachTcpClient | HostCommandAttachUdp | HostCommandDetachInterface | HostCommandEstablishLink | HostCommandRequestPath | HostCommandIdentify | HostCommandSendLinkPacket | HostCommandRequest | HostCommandRespond | HostCommandSendResource | HostCommandSetLinkResourceStrategy | HostCommandSetDestinationResourceStrategy | HostCommandSendChannelMessage | HostCommandAllowRequester | HostCommandAttachInterface
CommandOutcome: TypeAlias = CommandOutcomeAnnounced | CommandOutcomePacketDelivered | CommandOutcomeLinkCloseQueued | CommandOutcomeInterfaceAttached | CommandOutcomeInterfaceDetached | CommandOutcomeLinkEstablished | CommandOutcomePathDiscovered | CommandOutcomeIdentified | CommandOutcomeResponseReceived | CommandOutcomeResponseSent | CommandOutcomeResourceSent | CommandOutcomeResourceStrategySet | CommandOutcomeRequesterAllowed
CommandFailure: TypeAlias = CommandFailureNodeStopped | CommandFailureBusy | CommandFailurePayloadTooLarge | CommandFailureUnknownDestination | CommandFailureNotSingleDestination | CommandFailureAnnounceAppDataTooLong | CommandFailureUnknownInterface | CommandFailureNoRouteToDestination | CommandFailureNotDirectlyReachable | CommandFailurePacketCulled | CommandFailureDeliveryTimedOut | CommandFailureInvalidBitrate | CommandFailureBindFailed | CommandFailureWriteFailed | CommandFailureUnsupportedByBackend | CommandFailureUnknownLink | CommandFailureLinkNotActive | CommandFailureEntropyUnavailable | CommandFailureNotLinkInitiator | CommandFailureIdentityNotHeld | CommandFailureUnknownRequestHandler | CommandFailureRequestPolicyNotAllowList | CommandFailureRequestAllowListFull | CommandFailureLinkBusy | CommandFailureResourceTableFull | CommandFailureResourceMetadataTooLarge | CommandFailureResourceRejectedByPeer | CommandFailureResourceSequencingFailed | CommandFailureResourcePredecessorFailed | CommandFailureChannelWindowFull | CommandFailureChannelUntrackable | CommandFailureInvalidChannelMessageType | CommandFailureInvalidConfiguration | CommandFailureResourceUploadCancelled | CommandFailureResourceEarlyEof | CommandFailureResourceLengthOverrun | CommandFailurePermissionDenied | CommandFailureDeviceUnavailable | CommandFailureConnectFailed | CommandFailureBackendFailed | CommandFailureResponseTooLarge
ApplicationEvent: TypeAlias = ApplicationEventSingleDelivery | ApplicationEventRequest | ApplicationEventResponse | ApplicationEventResponseSegment | ApplicationEventResourceAvailable | ApplicationEventResourceSegment | ApplicationEventResourceNeedsDecompression | ApplicationEventChannelMessage | ApplicationEventLinkDelivery
DiagnosticEvent: TypeAlias = DiagnosticEventAnnounceHeard | DiagnosticEventLinkEstablished | DiagnosticEventPeerIdentified | DiagnosticEventLinkClosed | DiagnosticEventLinkInterfaceMismatch | DiagnosticEventResourceAssembled | DiagnosticEventResourceFailed | DiagnosticEventResourceSendProgress | DiagnosticEventSelfRatchetRotated | DiagnosticEventAnnounceHeldDropped | DiagnosticEventDelivered | DiagnosticEventRouteExpired | DiagnosticEventRouteEvicted | DiagnosticEventRouteInterfaceGone | DiagnosticEventRouteDropped | DiagnosticEventBackendDiagnostic | DiagnosticEventDiagnosticsDropped | DiagnosticEventPersistenceRestored | DiagnosticEventPersistenceFlushed | DiagnosticEventPersistenceFlushFailed

HOST_OPERATION_NAMES: tuple[str, ...] = (
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

RawValue = TypeVar("RawValue")

@dataclass(frozen=True, slots=True)
class _RawOwned(Generic[RawValue]):
    value: RawValue

@dataclass(frozen=True, slots=True)
class _RawBorrowed(Generic[RawValue]):
    value: RawValue

@dataclass(frozen=True, slots=True)
class _RawCallSuccess(Generic[RawValue]):
    value: RawValue

@dataclass(frozen=True, slots=True)
class _RawCallFailure:
    error: Status

_RawCallResult: TypeAlias = _RawCallSuccess[RawValue] | _RawCallFailure

class _RawUnit: pass

class _RawCommandResult: pass

class _RawContractInfo: pass

class _RawEvent: pass

class _RawEventStream: pass

class _RawHost: pass

class _RawHostInspection: pass

class _RawHostOptions: pass

class _RawIssuedCommand: pass

class _RawLifecycle: pass

class _RawReadinessCallback: pass

class _RawReadinessRegistration: pass

class _RawResourceChunk: pass

class _RawResourceStream: pass

class _RawResourceUpload: pass

class _RawSuppliedPipe: pass

class _RawSuppliedPipeOpenRequest: pass

class _RawOpaquePointer: pass

class _RawHostProtocol(Protocol):
    def contract_info(self) -> _RawCallResult[_RawContractInfo]: ...
    def backend_info(self) -> _RawCallResult[BackendInfo]: ...
    def host_create(self, options: _RawHostOptions) -> _RawCallResult[_RawOwned[_RawHost]]: ...
    def host_release(self, host: _RawHost) -> _RawUnit: ...
    def host_lifecycle(self, host: _RawHost) -> _RawCallResult[_RawLifecycle]: ...
    def host_snapshot(self, host: _RawHost, timeout_millis: int) -> _RawCallResult[_RawOwned[_RawHostInspection]]: ...
    def host_snapshot_read(self, host_inspection: _RawHostInspection) -> _RawCallResult[_RawBorrowed[HostSnapshot]]: ...
    def host_snapshot_release(self, host_inspection: _RawHostInspection) -> _RawUnit: ...
    def host_identity_hash(self, host: _RawHost) -> _RawCallResult[_RawBorrowed[bytes]]: ...
    def host_destination_count(self, host: _RawHost) -> int: ...
    def host_destination_hash(self, host: _RawHost, index: int) -> _RawCallResult[_RawBorrowed[bytes]]: ...
    def host_attach_supplied_pipe(self, host: _RawHost, name: str, respawn_delay_millis: int, bitrate: Bitrate) -> _RawCallResult[_RawOwned[_RawSuppliedPipe]]: ...
    def supplied_pipe_claim_attachment(self, supplied_pipe: _RawSuppliedPipe) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def supplied_pipe_next_open_request(self, supplied_pipe: _RawSuppliedPipe, timeout_millis: int) -> _RawCallResult[_RawOwned[_RawSuppliedPipeOpenRequest]]: ...
    def supplied_pipe_register_readiness(self, supplied_pipe: _RawSuppliedPipe, callback: _RawReadinessCallback, context: _RawOpaquePointer) -> _RawCallResult[_RawOwned[_RawReadinessRegistration]]: ...
    def supplied_pipe_interrupt_wait(self, supplied_pipe: _RawSuppliedPipe) -> _RawUnit: ...
    def supplied_pipe_release(self, supplied_pipe: _RawSuppliedPipe) -> _RawUnit: ...
    def supplied_pipe_open_request_provide(self, supplied_pipe_open_request: _RawSuppliedPipeOpenRequest, descriptor: int) -> _RawCallResult[bool]: ...
    def supplied_pipe_open_request_decline(self, supplied_pipe_open_request: _RawSuppliedPipeOpenRequest) -> _RawCallResult[bool]: ...
    def supplied_pipe_open_request_release(self, supplied_pipe_open_request: _RawSuppliedPipeOpenRequest) -> _RawUnit: ...
    def host_begin_resource_upload(self, host: _RawHost, link_id: LinkId, declared_length: int, packed_metadata: bytes | None, compression: ResourceCompression) -> _RawCallResult[_RawOwned[_RawResourceUpload]]: ...
    def resource_upload_write(self, resource_upload: _RawResourceUpload, chunk: bytes) -> _RawCallResult[_RawUnit]: ...
    def resource_upload_is_writable(self, resource_upload: _RawResourceUpload) -> _RawCallResult[bool]: ...
    def resource_upload_finish(self, resource_upload: _RawResourceUpload) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def resource_upload_abort(self, resource_upload: _RawResourceUpload) -> _RawUnit: ...
    def resource_upload_release(self, resource_upload: _RawResourceUpload) -> _RawUnit: ...
    def host_stop(self, host: _RawHost) -> _RawCallResult[_RawUnit]: ...
    def command_wait(self, issued_command: _RawIssuedCommand, timeout_millis: int) -> _RawCallResult[_RawBorrowed[_RawCommandResult]]: ...
    def command_register_readiness(self, issued_command: _RawIssuedCommand, callback: _RawReadinessCallback, context: _RawOpaquePointer) -> _RawCallResult[_RawOwned[_RawReadinessRegistration]]: ...
    def command_interrupt_wait(self, issued_command: _RawIssuedCommand) -> _RawUnit: ...
    def command_release(self, issued_command: _RawIssuedCommand) -> _RawUnit: ...
    def host_claim_application_events(self, host: _RawHost) -> _RawCallResult[_RawOwned[_RawEventStream]]: ...
    def host_claim_diagnostics(self, host: _RawHost) -> _RawCallResult[_RawOwned[_RawEventStream]]: ...
    def event_stream_register_readiness(self, event_stream: _RawEventStream, callback: _RawReadinessCallback, context: _RawOpaquePointer) -> _RawCallResult[_RawOwned[_RawReadinessRegistration]]: ...
    def readiness_registration_release(self, readiness_registration: _RawReadinessRegistration) -> _RawUnit: ...
    def event_stream_interrupt_wait(self, event_stream: _RawEventStream) -> _RawUnit: ...
    def event_stream_release(self, event_stream: _RawEventStream) -> _RawUnit: ...
    def event_stream_next(self, event_stream: _RawEventStream, timeout_millis: int) -> _RawCallResult[_RawOwned[_RawEvent]]: ...
    def event_release(self, event: _RawEvent) -> _RawUnit: ...
    def event_kind(self, event: _RawEvent) -> int: ...
    def event_bytes(self, event: _RawEvent, field: EventField) -> _RawCallResult[_RawBorrowed[bytes]]: ...
    def event_string(self, event: _RawEvent, field: EventField) -> _RawCallResult[_RawBorrowed[str]]: ...
    def event_u64(self, event: _RawEvent, field: EventField) -> _RawCallResult[int]: ...
    def event_u128(self, event: _RawEvent, field: EventField) -> _RawCallResult[int]: ...
    def event_resource_stream(self, event: _RawEvent) -> _RawCallResult[_RawOwned[_RawResourceStream]]: ...
    def resource_stream_release(self, resource_stream: _RawResourceStream) -> _RawUnit: ...
    def resource_stream_next(self, resource_stream: _RawResourceStream, maximum_bytes: int) -> _RawCallResult[_RawBorrowed[_RawResourceChunk]]: ...
    def host_announce(self, host: _RawHost, destination: DestinationHash, interface: InterfaceId | None) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_send_single_packet(self, host: _RawHost, destination: DestinationHash, payload: bytes) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_close_link(self, host: _RawHost, link_id: LinkId) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_attach_tcp_server(self, host: _RawHost, bind: str, bitrate: Bitrate) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_attach_tcp_client(self, host: _RawHost, target: str, bitrate: Bitrate) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_attach_udp(self, host: _RawHost, local: str, peer: str, bitrate: Bitrate) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_detach_interface(self, host: _RawHost, interface: InterfaceId) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_establish_link(self, host: _RawHost, destination: DestinationHash) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_request_path(self, host: _RawHost, destination: DestinationHash) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_identify(self, host: _RawHost, link_id: LinkId, identity: IdentityHash) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_send_link_packet(self, host: _RawHost, link_id: LinkId, payload: bytes) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_request(self, host: _RawHost, link_id: LinkId, path_hash: RequestPathHash, payload: bytes, timeout: ResponseTimeout, maximum_response_bytes: int | None) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_respond(self, host: _RawHost, link_id: LinkId, request_id: RequestId, request_rtt_millis: int, payload: bytes) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_send_resource(self, host: _RawHost, link_id: LinkId, payload: bytes, packed_metadata: bytes | None, compression: ResourceCompression) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_set_link_resource_strategy(self, host: _RawHost, link_id: LinkId, strategy: ResourceStrategy) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_set_destination_resource_strategy(self, host: _RawHost, destination: DestinationHash, strategy: ResourceStrategy) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_send_channel_message(self, host: _RawHost, link_id: LinkId, message_type: int, payload: bytes) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_allow_requester(self, host: _RawHost, destination: DestinationHash, path_hash: RequestPathHash, identity: IdentityHash) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
    def host_attach_interface(self, host: _RawHost, config: InterfaceConfig, routing: InterfaceRoutingPolicy | None) -> _RawCallResult[_RawOwned[_RawIssuedCommand]]: ...
