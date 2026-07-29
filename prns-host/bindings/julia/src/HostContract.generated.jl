const HOST_CONTRACT_ABI = UInt32(1)
const HOST_SCHEMA_VERSION = UInt32(1)
const PRODUCT_VERSION = "0.3.1"
const DESTINATION_HASH_LENGTH = 16
const IDENTITY_HASH_LENGTH = 16
const INTERFACE_ID_LENGTH = 8
const LINK_ID_LENGTH = 16
const PACKET_HASH_LENGTH = 32
const REQUEST_ID_LENGTH = 16
const REQUEST_PATH_HASH_LENGTH = 16
const RESOURCE_HASH_LENGTH = 32
const IDENTITY_SECRET_LENGTH = 64
const BALANCED_PENDING_COMMANDS = 256
const BALANCED_APPLICATION_EVENTS = 1024
const BALANCED_RETAINED_EVENT_BYTES = 8388608
const BALANCED_DIAGNOSTICS = 1024

@enum Status::UInt32 begin
    StatusOk = 0
    StatusInvalidArgument = 1
    StatusContractMismatch = 2
    StatusInvalidHandle = 3
    StatusNotReady = 4
    StatusAlreadyClaimed = 5
    StatusWouldBlock = 6
    StatusTimedOut = 7
    StatusQueueFull = 8
    StatusStopped = 9
    StatusBackendFailed = 10
    StatusPanic = 11
    StatusInterrupted = 12
end

@enum BackendKind::UInt32 begin
    BackendKindNative = 1
    BackendKindBrowser = 2
    BackendKindCooperative = 3
end

@enum Capability::UInt32 begin
    CapabilityLoopback = 1
    CapabilityTcpClient = 2
    CapabilityTcpServer = 3
    CapabilityUdp = 4
    CapabilitySerial = 5
    CapabilityUsb = 6
    CapabilityBluetooth = 7
    CapabilityWifi = 8
    CapabilityWebSocket = 9
    CapabilityBrowserRendezvous = 10
    CapabilityI2p = 11
    CapabilityWeave = 12
end

@enum HostRole::UInt32 begin
    HostRoleEndpoint = 1
    HostRoleTransport = 2
end

@enum IdentityConfigKind::UInt32 begin
    IdentityConfigKindExisting = 1
    IdentityConfigKindGenerateEphemeral = 2
    IdentityConfigKindLoadOrCreate = 3
end

@enum DestinationConfigKind::UInt32 begin
    DestinationConfigKindPlain = 1
    DestinationConfigKindSingle = 2
end

@enum DestinationIdentityConfigKind::UInt32 begin
    DestinationIdentityConfigKindHostIdentity = 1
    DestinationIdentityConfigKindDedicatedIdentity = 2
end

@enum BitrateKind::UInt32 begin
    BitrateKindAuto = 1
    BitrateKindBitsPerSecond = 2
end

@enum ResponseTimeoutKind::UInt32 begin
    ResponseTimeoutKindLinkDefault = 1
    ResponseTimeoutKindExact = 2
end

@enum ResourceCompressionKind::UInt32 begin
    ResourceCompressionKindAuto = 1
    ResourceCompressionKindNever = 2
end

@enum ResourceStrategyKind::UInt32 begin
    ResourceStrategyKindRefuse = 1
    ResourceStrategyKindAccept = 2
end

@enum RequestPolicy::UInt32 begin
    RequestPolicyAllowNone = 1
    RequestPolicyAllowAll = 2
    RequestPolicyAllowList = 3
end

@enum CommandOutcomeKind::UInt32 begin
    CommandOutcomeKindAnnounced = 1
    CommandOutcomeKindPacketDelivered = 2
    CommandOutcomeKindLinkCloseQueued = 3
    CommandOutcomeKindInterfaceAttached = 4
    CommandOutcomeKindInterfaceDetached = 5
    CommandOutcomeKindLinkEstablished = 6
    CommandOutcomeKindPathDiscovered = 7
    CommandOutcomeKindIdentified = 8
    CommandOutcomeKindResponseReceived = 9
    CommandOutcomeKindResponseSent = 10
    CommandOutcomeKindResourceSent = 11
    CommandOutcomeKindResourceStrategySet = 12
    CommandOutcomeKindRequesterAllowed = 13
end

@enum CommandFailureKind::UInt32 begin
    CommandFailureKindNodeStopped = 1
    CommandFailureKindBusy = 2
    CommandFailureKindPayloadTooLarge = 3
    CommandFailureKindUnknownDestination = 4
    CommandFailureKindNotSingleDestination = 5
    CommandFailureKindAnnounceAppDataTooLong = 6
    CommandFailureKindUnknownInterface = 7
    CommandFailureKindNoRouteToDestination = 8
    CommandFailureKindNotDirectlyReachable = 9
    CommandFailureKindPacketCulled = 10
    CommandFailureKindDeliveryTimedOut = 11
    CommandFailureKindInvalidBitrate = 12
    CommandFailureKindBindFailed = 13
    CommandFailureKindWriteFailed = 14
    CommandFailureKindUnsupportedByBackend = 15
    CommandFailureKindUnknownLink = 16
    CommandFailureKindLinkNotActive = 17
    CommandFailureKindEntropyUnavailable = 18
    CommandFailureKindNotLinkInitiator = 19
    CommandFailureKindIdentityNotHeld = 20
    CommandFailureKindUnknownRequestHandler = 21
    CommandFailureKindRequestPolicyNotAllowList = 22
    CommandFailureKindRequestAllowListFull = 23
    CommandFailureKindLinkBusy = 24
    CommandFailureKindResourceTableFull = 25
    CommandFailureKindResourceMetadataTooLarge = 26
    CommandFailureKindResourceRejectedByPeer = 27
    CommandFailureKindResourceSequencingFailed = 28
    CommandFailureKindResourcePredecessorFailed = 29
    CommandFailureKindChannelWindowFull = 30
    CommandFailureKindChannelUntrackable = 31
    CommandFailureKindInvalidChannelMessageType = 32
end

@enum DeliveryEvidenceKind::UInt32 begin
    DeliveryEvidenceKindExplicitProof = 1
    DeliveryEvidenceKindImplicitProof = 2
    DeliveryEvidenceKindResponse = 3
end

@enum LifecyclePhase::UInt32 begin
    LifecyclePhaseStarting = 1
    LifecyclePhaseRunning = 2
    LifecyclePhaseStopping = 3
    LifecyclePhaseStopped = 4
    LifecyclePhaseFailed = 5
end

@enum StopReason::UInt32 begin
    StopReasonRequested = 1
    StopReasonBackendExited = 2
end

@enum LinkClosedReason::UInt32 begin
    LinkClosedReasonTimeout = 1
    LinkClosedReasonPeerClosed = 2
    LinkClosedReasonMalformedRtt = 3
end

@enum ApplicationEventKind::UInt32 begin
    ApplicationEventKindSingleDelivery = 100
    ApplicationEventKindRequest = 101
    ApplicationEventKindResponse = 102
    ApplicationEventKindResponseSegment = 103
    ApplicationEventKindResourceAvailable = 104
    ApplicationEventKindResourceSegment = 105
    ApplicationEventKindResourceNeedsDecompression = 106
    ApplicationEventKindChannelMessage = 107
end

@enum DiagnosticEventKind::UInt32 begin
    DiagnosticEventKindAnnounceHeard = 200
    DiagnosticEventKindLinkEstablished = 201
    DiagnosticEventKindPeerIdentified = 202
    DiagnosticEventKindLinkClosed = 203
    DiagnosticEventKindLinkInterfaceMismatch = 204
    DiagnosticEventKindResourceAssembled = 205
    DiagnosticEventKindResourceFailed = 206
    DiagnosticEventKindResourceSendProgress = 207
    DiagnosticEventKindSelfRatchetRotated = 208
    DiagnosticEventKindAnnounceHeldDropped = 209
    DiagnosticEventKindDelivered = 210
    DiagnosticEventKindRouteExpired = 211
    DiagnosticEventKindRouteEvicted = 212
    DiagnosticEventKindRouteInterfaceGone = 213
    DiagnosticEventKindRouteDropped = 214
    DiagnosticEventKindBackendDiagnostic = 215
    DiagnosticEventKindDiagnosticsDropped = 216
end

@enum EventField::UInt32 begin
    EventFieldDestination = 1
    EventFieldSourceInterface = 2
    EventFieldPlaintext = 3
    EventFieldLinkId = 4
    EventFieldRequestId = 5
    EventFieldRequester = 6
    EventFieldPathHash = 7
    EventFieldRttMillis = 8
    EventFieldData = 9
    EventFieldSegmentIndex = 10
    EventFieldTotalSegments = 11
    EventFieldHash = 12
    EventFieldOriginalHash = 13
    EventFieldMetadata = 14
    EventFieldTotalBytes = 15
    EventFieldStreamId = 16
    EventFieldUncompressedDataBytes = 17
    EventFieldMessageType = 18
    EventFieldIdentity = 19
    EventFieldReason = 20
    EventFieldAttachedInterface = 21
    EventFieldArrivedOn = 22
    EventFieldTotalSizeBytes = 23
    EventFieldCause = 24
    EventFieldTransferredBytes = 25
    EventFieldPhysicalTransferredBytes = 26
    EventFieldDetail = 27
    EventFieldKind = 28
    EventFieldDroppedCount = 29
    EventFieldHops = 30
    EventFieldStream = 31
end

struct DestinationHash
    bytes::NTuple{16,UInt8}

    function DestinationHash(bytes)
        length(bytes) == 16 || throw(ArgumentError("DestinationHash requires 16 bytes"))
        new(Tuple(UInt8(value) for value in bytes)::NTuple{16,UInt8})
    end
end

struct IdentityHash
    bytes::NTuple{16,UInt8}

    function IdentityHash(bytes)
        length(bytes) == 16 || throw(ArgumentError("IdentityHash requires 16 bytes"))
        new(Tuple(UInt8(value) for value in bytes)::NTuple{16,UInt8})
    end
end

struct InterfaceId
    bytes::NTuple{8,UInt8}

    function InterfaceId(bytes)
        length(bytes) == 8 || throw(ArgumentError("InterfaceId requires 8 bytes"))
        new(Tuple(UInt8(value) for value in bytes)::NTuple{8,UInt8})
    end
end

struct LinkId
    bytes::NTuple{16,UInt8}

    function LinkId(bytes)
        length(bytes) == 16 || throw(ArgumentError("LinkId requires 16 bytes"))
        new(Tuple(UInt8(value) for value in bytes)::NTuple{16,UInt8})
    end
end

struct PacketHash
    bytes::NTuple{32,UInt8}

    function PacketHash(bytes)
        length(bytes) == 32 || throw(ArgumentError("PacketHash requires 32 bytes"))
        new(Tuple(UInt8(value) for value in bytes)::NTuple{32,UInt8})
    end
end

struct RequestId
    bytes::NTuple{16,UInt8}

    function RequestId(bytes)
        length(bytes) == 16 || throw(ArgumentError("RequestId requires 16 bytes"))
        new(Tuple(UInt8(value) for value in bytes)::NTuple{16,UInt8})
    end
end

struct RequestPathHash
    bytes::NTuple{16,UInt8}

    function RequestPathHash(bytes)
        length(bytes) == 16 || throw(ArgumentError("RequestPathHash requires 16 bytes"))
        new(Tuple(UInt8(value) for value in bytes)::NTuple{16,UInt8})
    end
end

struct ResourceHash
    bytes::NTuple{32,UInt8}

    function ResourceHash(bytes)
        length(bytes) == 32 || throw(ArgumentError("ResourceHash requires 32 bytes"))
        new(Tuple(UInt8(value) for value in bytes)::NTuple{32,UInt8})
    end
end

mutable struct IdentitySecret
    bytes::Vector{UInt8}

    function IdentitySecret(bytes::AbstractVector{UInt8})
        length(bytes) == 64 || throw(ArgumentError("IdentitySecret requires 64 bytes"))
        value = new(Vector{UInt8}(bytes))
        finalizer(close, value)
        value
    end
end

function Base.close(value::IdentitySecret)
    fill!(value.bytes, 0x00)
    nothing
end

struct DestinationName
    app_name::String
    aspects::Vector{String}
end

struct RequestHandlerConfig
    path::String
    policy::RequestPolicy
end

abstract type ResourceStream end

abstract type IdentityConfig end

struct IdentityConfigExisting <: IdentityConfig
    secret::IdentitySecret
end

struct IdentityConfigGenerateEphemeral <: IdentityConfig
end

struct IdentityConfigLoadOrCreate <: IdentityConfig
    path::String
end

abstract type DestinationIdentityConfig end

struct DestinationIdentityConfigHostIdentity <: DestinationIdentityConfig
end

struct DestinationIdentityConfigDedicatedIdentity <: DestinationIdentityConfig
    identity::IdentityConfig
end

abstract type Bitrate end

struct BitrateAuto <: Bitrate
end

struct BitrateBitsPerSecond <: Bitrate
    value::UInt64
end

abstract type ResponseTimeout end

struct ResponseTimeoutLinkDefault <: ResponseTimeout
end

struct ResponseTimeoutExact <: ResponseTimeout
    millis::UInt64
end

abstract type ResourceCompression end

struct ResourceCompressionAuto <: ResourceCompression
end

struct ResourceCompressionNever <: ResourceCompression
end

abstract type ResourceStrategy end

struct ResourceStrategyRefuse <: ResourceStrategy
end

struct ResourceStrategyAccept <: ResourceStrategy
    maximum_uncompressed_bytes::UInt64
    accept_compressed::Bool
end

abstract type DestinationConfig end

struct DestinationConfigPlain <: DestinationConfig
    name::DestinationName
end

struct DestinationConfigSingle <: DestinationConfig
    name::DestinationName
    identity::DestinationIdentityConfig
    announce_app_data::Union{Nothing,Vector{UInt8}}
    request_handlers::Vector{RequestHandlerConfig}
end

abstract type HostCommand end

struct HostCommandAnnounce <: HostCommand
    destination::DestinationHash
    interface::Union{Nothing,InterfaceId}
end

struct HostCommandSendSinglePacket <: HostCommand
    destination::DestinationHash
    payload::Vector{UInt8}
end

struct HostCommandCloseLink <: HostCommand
    link_id::LinkId
end

struct HostCommandAttachTcpServer <: HostCommand
    bind::String
    bitrate::Bitrate
end

struct HostCommandAttachTcpClient <: HostCommand
    target::String
    bitrate::Bitrate
end

struct HostCommandAttachUdp <: HostCommand
    var"local"::String
    peer::String
    bitrate::Bitrate
end

struct HostCommandDetachInterface <: HostCommand
    interface::InterfaceId
end

struct HostCommandEstablishLink <: HostCommand
    destination::DestinationHash
end

struct HostCommandRequestPath <: HostCommand
    destination::DestinationHash
end

struct HostCommandIdentify <: HostCommand
    link_id::LinkId
    identity::IdentityHash
end

struct HostCommandSendLinkPacket <: HostCommand
    link_id::LinkId
    payload::Vector{UInt8}
end

struct HostCommandRequest <: HostCommand
    link_id::LinkId
    path_hash::RequestPathHash
    payload::Vector{UInt8}
    timeout::ResponseTimeout
end

struct HostCommandRespond <: HostCommand
    link_id::LinkId
    request_id::RequestId
    request_rtt_millis::UInt64
    payload::Vector{UInt8}
end

struct HostCommandSendResource <: HostCommand
    link_id::LinkId
    payload::Vector{UInt8}
    packed_metadata::Union{Nothing,Vector{UInt8}}
    compression::ResourceCompression
end

struct HostCommandSetLinkResourceStrategy <: HostCommand
    link_id::LinkId
    strategy::ResourceStrategy
end

struct HostCommandSetDestinationResourceStrategy <: HostCommand
    destination::DestinationHash
    strategy::ResourceStrategy
end

struct HostCommandSendChannelMessage <: HostCommand
    link_id::LinkId
    message_type::UInt16
    payload::Vector{UInt8}
end

struct HostCommandAllowRequester <: HostCommand
    destination::DestinationHash
    path_hash::RequestPathHash
    identity::IdentityHash
end

abstract type CommandOutcome end

struct CommandOutcomeAnnounced <: CommandOutcome
end

struct CommandOutcomePacketDelivered <: CommandOutcome
    rtt_millis::UInt64
    evidence::DeliveryEvidenceKind
    packet_hash::Union{Nothing,PacketHash}
end

struct CommandOutcomeLinkCloseQueued <: CommandOutcome
end

struct CommandOutcomeInterfaceAttached <: CommandOutcome
    interface::InterfaceId
end

struct CommandOutcomeInterfaceDetached <: CommandOutcome
    interface::InterfaceId
end

struct CommandOutcomeLinkEstablished <: CommandOutcome
    link_id::LinkId
    rtt_millis::UInt64
end

struct CommandOutcomePathDiscovered <: CommandOutcome
    hops::UInt8
end

struct CommandOutcomeIdentified <: CommandOutcome
end

struct CommandOutcomeResponseReceived <: CommandOutcome
    data::Vector{UInt8}
    rtt_millis::UInt64
end

struct CommandOutcomeResponseSent <: CommandOutcome
    rtt_millis::UInt64
end

struct CommandOutcomeResourceSent <: CommandOutcome
end

struct CommandOutcomeResourceStrategySet <: CommandOutcome
end

struct CommandOutcomeRequesterAllowed <: CommandOutcome
end

abstract type CommandFailure end

struct CommandFailureNodeStopped <: CommandFailure
end

struct CommandFailureBusy <: CommandFailure
end

struct CommandFailurePayloadTooLarge <: CommandFailure
end

struct CommandFailureUnknownDestination <: CommandFailure
end

struct CommandFailureNotSingleDestination <: CommandFailure
end

struct CommandFailureAnnounceAppDataTooLong <: CommandFailure
end

struct CommandFailureUnknownInterface <: CommandFailure
end

struct CommandFailureNoRouteToDestination <: CommandFailure
end

struct CommandFailureNotDirectlyReachable <: CommandFailure
end

struct CommandFailurePacketCulled <: CommandFailure
end

struct CommandFailureDeliveryTimedOut <: CommandFailure
end

struct CommandFailureInvalidBitrate <: CommandFailure
end

struct CommandFailureBindFailed <: CommandFailure
    detail::String
end

struct CommandFailureWriteFailed <: CommandFailure
    detail::String
end

struct CommandFailureUnsupportedByBackend <: CommandFailure
end

struct CommandFailureUnknownLink <: CommandFailure
end

struct CommandFailureLinkNotActive <: CommandFailure
end

struct CommandFailureEntropyUnavailable <: CommandFailure
end

struct CommandFailureNotLinkInitiator <: CommandFailure
end

struct CommandFailureIdentityNotHeld <: CommandFailure
end

struct CommandFailureUnknownRequestHandler <: CommandFailure
end

struct CommandFailureRequestPolicyNotAllowList <: CommandFailure
end

struct CommandFailureRequestAllowListFull <: CommandFailure
end

struct CommandFailureLinkBusy <: CommandFailure
end

struct CommandFailureResourceTableFull <: CommandFailure
end

struct CommandFailureResourceMetadataTooLarge <: CommandFailure
end

struct CommandFailureResourceRejectedByPeer <: CommandFailure
end

struct CommandFailureResourceSequencingFailed <: CommandFailure
end

struct CommandFailureResourcePredecessorFailed <: CommandFailure
end

struct CommandFailureChannelWindowFull <: CommandFailure
end

struct CommandFailureChannelUntrackable <: CommandFailure
end

struct CommandFailureInvalidChannelMessageType <: CommandFailure
end

abstract type ApplicationEvent end

struct ApplicationEventSingleDelivery <: ApplicationEvent
    destination::DestinationHash
    source_interface::InterfaceId
    plaintext::Vector{UInt8}
end

struct ApplicationEventRequest <: ApplicationEvent
    destination::DestinationHash
    link_id::LinkId
    request_id::RequestId
    requester::Union{Nothing,IdentityHash}
    path_hash::RequestPathHash
    rtt_millis::UInt64
    data::Vector{UInt8}
end

struct ApplicationEventResponse <: ApplicationEvent
    link_id::LinkId
    request_id::RequestId
    data::Vector{UInt8}
end

struct ApplicationEventResponseSegment <: ApplicationEvent
    link_id::LinkId
    request_id::RequestId
    segment_index::UInt64
    total_segments::UInt64
    data::Vector{UInt8}
end

struct ApplicationEventResourceAvailable <: ApplicationEvent
    link_id::LinkId
    hash::ResourceHash
    metadata::Union{Nothing,Vector{UInt8}}
    resource::ResourceStream
end

struct ApplicationEventResourceSegment <: ApplicationEvent
    link_id::LinkId
    original_hash::ResourceHash
    segment_index::UInt64
    total_segments::UInt64
    metadata::Union{Nothing,Vector{UInt8}}
    data::Vector{UInt8}
end

struct ApplicationEventResourceNeedsDecompression <: ApplicationEvent
    link_id::LinkId
    hash::ResourceHash
    stream::Vector{UInt8}
    uncompressed_data_bytes::UInt64
end

struct ApplicationEventChannelMessage <: ApplicationEvent
    link_id::LinkId
    message_type::UInt16
    data::Vector{UInt8}
end

abstract type DiagnosticEvent end

struct DiagnosticEventAnnounceHeard <: DiagnosticEvent
    destination::DestinationHash
    hops::UInt8
    source_interface::InterfaceId
end

struct DiagnosticEventLinkEstablished <: DiagnosticEvent
    link_id::LinkId
    rtt_millis::UInt64
end

struct DiagnosticEventPeerIdentified <: DiagnosticEvent
    link_id::LinkId
    identity::IdentityHash
end

struct DiagnosticEventLinkClosed <: DiagnosticEvent
    link_id::LinkId
    reason::LinkClosedReason
end

struct DiagnosticEventLinkInterfaceMismatch <: DiagnosticEvent
    link_id::LinkId
    attached_interface::InterfaceId
    arrived_on::InterfaceId
end

struct DiagnosticEventResourceAssembled <: DiagnosticEvent
    link_id::LinkId
    original_hash::ResourceHash
    total_size_bytes::UInt64
end

struct DiagnosticEventResourceFailed <: DiagnosticEvent
    link_id::LinkId
    hash::ResourceHash
    cause::String
end

struct DiagnosticEventResourceSendProgress <: DiagnosticEvent
    link_id::LinkId
    transferred_bytes::UInt64
    total_bytes::UInt64
    physical_transferred_bytes::UInt64
    segment_index::UInt64
    total_segments::UInt64
end

struct DiagnosticEventSelfRatchetRotated <: DiagnosticEvent
    destination::DestinationHash
end

struct DiagnosticEventAnnounceHeldDropped <: DiagnosticEvent
    destination::DestinationHash
    source_interface::InterfaceId
    cause::String
end

struct DiagnosticEventDelivered <: DiagnosticEvent
    detail::String
end

struct DiagnosticEventRouteExpired <: DiagnosticEvent
    destination::DestinationHash
end

struct DiagnosticEventRouteEvicted <: DiagnosticEvent
    destination::DestinationHash
end

struct DiagnosticEventRouteInterfaceGone <: DiagnosticEvent
    destination::DestinationHash
end

struct DiagnosticEventRouteDropped <: DiagnosticEvent
    destination::DestinationHash
end

struct DiagnosticEventBackendDiagnostic <: DiagnosticEvent
    kind::String
    detail::String
end

struct DiagnosticEventDiagnosticsDropped <: DiagnosticEvent
    count::UInt128
end
