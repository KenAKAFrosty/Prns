from __future__ import annotations

from dataclasses import dataclass
from enum import IntEnum
from typing import Any, TypeAlias

HOST_CONTRACT_ABI = 1
SCHEMA_VERSION = 1
PRODUCT_VERSION = "0.2.8"
DESTINATION_HASH_LENGTH = 16
IDENTITY_HASH_LENGTH = 16
INTERFACE_ID_LENGTH = 8
LINK_ID_LENGTH = 16
PACKET_HASH_LENGTH = 32
REQUEST_ID_LENGTH = 16
REQUEST_PATH_HASH_LENGTH = 16
RESOURCE_HASH_LENGTH = 32
IDENTITY_SECRET_LENGTH = 64
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

class HostRole(IntEnum):
    ENDPOINT = 1
    TRANSPORT = 2

class IdentityConfigKind(IntEnum):
    EXISTING = 1
    GENERATE_EPHEMERAL = 2
    LOAD_OR_CREATE = 3

class DestinationConfigKind(IntEnum):
    PLAIN = 1
    SINGLE = 2

class DestinationIdentityConfigKind(IntEnum):
    HOST_IDENTITY = 1
    DEDICATED_IDENTITY = 2

class BitrateKind(IntEnum):
    AUTO = 1
    BITS_PER_SECOND = 2

class CommandOutcomeKind(IntEnum):
    ANNOUNCED = 1
    PACKET_DELIVERED = 2
    LINK_CLOSE_QUEUED = 3
    INTERFACE_ATTACHED = 4
    INTERFACE_DETACHED = 5

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
class IdentityConfigExisting:
    secret: IdentitySecret

@dataclass(frozen=True, slots=True)
class IdentityConfigGenerateEphemeral:
    pass

@dataclass(frozen=True, slots=True)
class IdentityConfigLoadOrCreate:
    path: str

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
class DestinationConfigPlain:
    name: DestinationName

@dataclass(frozen=True, slots=True)
class DestinationConfigSingle:
    name: DestinationName
    identity: DestinationIdentityConfig
    announce_app_data: bytes | None

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
    message_type: str
    data: bytes

@dataclass(frozen=True, slots=True)
class DiagnosticEventAnnounceHeard:
    destination: DestinationHash
    hops: int
    source_interface: InterfaceId

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

IdentityConfig: TypeAlias = IdentityConfigExisting | IdentityConfigGenerateEphemeral | IdentityConfigLoadOrCreate
DestinationIdentityConfig: TypeAlias = DestinationIdentityConfigHostIdentity | DestinationIdentityConfigDedicatedIdentity
Bitrate: TypeAlias = BitrateAuto | BitrateBitsPerSecond
DestinationConfig: TypeAlias = DestinationConfigPlain | DestinationConfigSingle
HostCommand: TypeAlias = HostCommandAnnounce | HostCommandSendSinglePacket | HostCommandCloseLink | HostCommandAttachTcpServer | HostCommandAttachTcpClient | HostCommandAttachUdp | HostCommandDetachInterface
CommandOutcome: TypeAlias = CommandOutcomeAnnounced | CommandOutcomePacketDelivered | CommandOutcomeLinkCloseQueued | CommandOutcomeInterfaceAttached | CommandOutcomeInterfaceDetached
ApplicationEvent: TypeAlias = ApplicationEventSingleDelivery | ApplicationEventRequest | ApplicationEventResponse | ApplicationEventResponseSegment | ApplicationEventResourceAvailable | ApplicationEventResourceSegment | ApplicationEventResourceNeedsDecompression | ApplicationEventChannelMessage
DiagnosticEvent: TypeAlias = DiagnosticEventAnnounceHeard | DiagnosticEventLinkEstablished | DiagnosticEventPeerIdentified | DiagnosticEventLinkClosed | DiagnosticEventLinkInterfaceMismatch | DiagnosticEventResourceAssembled | DiagnosticEventResourceFailed | DiagnosticEventResourceSendProgress | DiagnosticEventSelfRatchetRotated | DiagnosticEventAnnounceHeldDropped | DiagnosticEventDelivered | DiagnosticEventRouteExpired | DiagnosticEventRouteEvicted | DiagnosticEventRouteInterfaceGone | DiagnosticEventRouteDropped | DiagnosticEventBackendDiagnostic | DiagnosticEventDiagnosticsDropped
