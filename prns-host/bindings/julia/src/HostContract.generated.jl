const HOST_CONTRACT_ABI = UInt32(1)
const HOST_SCHEMA_VERSION = UInt32(1)
const PRODUCT_VERSION = "0.3.6"
const DESTINATION_HASH_LENGTH = 16
const IDENTITY_HASH_LENGTH = 16
const INTERFACE_ID_LENGTH = 8
const LINK_ID_LENGTH = 16
const PACKET_HASH_LENGTH = 32
const REQUEST_ID_LENGTH = 16
const REQUEST_PATH_HASH_LENGTH = 16
const RESOURCE_HASH_LENGTH = 32
const IDENTITY_SECRET_LENGTH = 64
const SAFE_INT_MIN = Int64(-9007199254740991)
const SAFE_INT_MAX = Int64(9007199254740991)
const SAFE_UINT_MAX = UInt64(9007199254740991)
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
    StatusUnsupported = 13
    StatusPermissionDenied = 14
    StatusUnavailable = 15
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
    CapabilitySuppliedPipe = 13
end

@enum InterfaceKind::UInt32 begin
    InterfaceKindAutoLan = 1
    InterfaceKindTcpClient = 2
    InterfaceKindTcpServer = 3
    InterfaceKindUdp = 4
    InterfaceKindSerial = 5
    InterfaceKindKiss = 6
    InterfaceKindAx25Kiss = 7
    InterfaceKindRNode = 8
    InterfaceKindMultiRNode = 9
    InterfaceKindPipe = 10
    InterfaceKindBackboneClient = 11
    InterfaceKindBackboneServer = 12
    InterfaceKindI2p = 13
    InterfaceKindWeave = 14
    InterfaceKindAutomaticUsb = 15
    InterfaceKindAutomaticBluetoothLe = 16
    InterfaceKindWebSocketClient = 17
    InterfaceKindWebSocketServer = 18
    InterfaceKindBrowserRendezvous = 19
end

@enum InterfaceMode::UInt32 begin
    InterfaceModeFull = 1
    InterfaceModePointToPoint = 2
    InterfaceModeAccessPoint = 3
    InterfaceModeRoaming = 4
    InterfaceModeBoundary = 5
    InterfaceModeGateway = 6
    InterfaceModeInternal = 7
end

@enum WebSocketFramingSelection::UInt32 begin
    WebSocketFramingSelectionRawPacket = 1
    WebSocketFramingSelectionHdlc = 2
    WebSocketFramingSelectionKiss = 3
    WebSocketFramingSelectionAuto = 4
end

@enum InterfaceHealth::UInt32 begin
    InterfaceHealthInitializing = 1
    InterfaceHealthConnected = 2
    InterfaceHealthDegraded = 3
    InterfaceHealthReconnecting = 4
    InterfaceHealthFailed = 5
    InterfaceHealthDisconnected = 6
    InterfaceHealthDisabled = 7
    InterfaceHealthUnknown = 8
end

@enum DiscoveryScope::UInt32 begin
    DiscoveryScopeLink = 1
    DiscoveryScopeAdmin = 2
    DiscoveryScopeSite = 3
    DiscoveryScopeOrganization = 4
    DiscoveryScopeGlobal = 5
end

@enum MulticastAddressType::UInt32 begin
    MulticastAddressTypeTemporary = 1
    MulticastAddressTypePermanent = 2
end

@enum SerialDataBits::UInt32 begin
    SerialDataBitsFive = 5
    SerialDataBitsSix = 6
    SerialDataBitsSeven = 7
    SerialDataBitsEight = 8
end

@enum SerialParity::UInt32 begin
    SerialParityNone = 1
    SerialParityEven = 2
    SerialParityOdd = 3
end

@enum SerialStopBits::UInt32 begin
    SerialStopBitsOne = 1
    SerialStopBitsTwo = 2
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

@enum PersistenceConfigKind::UInt32 begin
    PersistenceConfigKindEphemeral = 1
    PersistenceConfigKindDirectory = 2
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
    CommandFailureKindInvalidConfiguration = 33
    CommandFailureKindResourceUploadCancelled = 34
    CommandFailureKindResourceEarlyEof = 35
    CommandFailureKindResourceLengthOverrun = 36
    CommandFailureKindPermissionDenied = 37
    CommandFailureKindDeviceUnavailable = 38
    CommandFailureKindConnectFailed = 39
    CommandFailureKindBackendFailed = 40
    CommandFailureKindResponseTooLarge = 41
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
    ApplicationEventKindLinkDelivery = 108
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
    DiagnosticEventKindPersistenceRestored = 217
    DiagnosticEventKindPersistenceFlushed = 218
    DiagnosticEventKindPersistenceFlushFailed = 219
end

@enum PersistenceFlushCause::UInt32 begin
    PersistenceFlushCauseStartup = 1
    PersistenceFlushCauseInterval = 2
    PersistenceFlushCauseRouteChange = 3
    PersistenceFlushCauseRatchetRotation = 4
    PersistenceFlushCauseShutdown = 5
end

@enum PersistenceFlushTarget::UInt32 begin
    PersistenceFlushTargetRoutingState = 1
    PersistenceFlushTargetRatchets = 2
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
    EventFieldRoutes = 32
    EventFieldDestinationIdentities = 33
    EventFieldTunnels = 34
    EventFieldRatchets = 35
    EventFieldRefused = 36
    EventFieldDropped = 37
    EventFieldPersistenceCause = 38
    EventFieldPersistenceTarget = 39
    EventFieldAppData = 40
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

struct SerialLineConfig
    baud::UInt32
    data_bits::SerialDataBits
    parity::SerialParity
    stop_bits::SerialStopBits
end

struct RNodeRadioConfig
    frequency_hz::UInt64
    bandwidth_hz::UInt32
    tx_power_dbm::Int16
    spreading_factor::UInt8
    coding_rate::UInt8
end

struct MultiRNodeMemberConfig
    name::String
    virtual_port::UInt8
    radio::RNodeRadioConfig
    flow_control::Bool
    outgoing::Bool
end

struct InterfaceRoutingPolicy
    mode::Union{Nothing,InterfaceMode}
    gravity::Union{Nothing,Int64}
    recursive_path_requests::Union{Nothing,Bool}
    announces_from_internal::Union{Nothing,Bool}
    announces_to_internal::Union{Nothing,Bool}
end

struct BackendInfo
    backend::BackendKind
    capabilities::Vector{Capability}
    interface_kinds::Vector{InterfaceKind}
end

struct InterfaceSnapshot
    interface_id::InterfaceId
    name::Union{Nothing,String}
    kind::Union{Nothing,InterfaceKind}
    health::InterfaceHealth
    failure_detail::Union{Nothing,String}
    rx_bytes::UInt64
    tx_bytes::UInt64
    rx_bps::Union{Nothing,UInt64}
    tx_bps::Union{Nothing,UInt64}
    route_count::UInt32
    link_count::UInt32
    transported_link_count::UInt32
end

struct RouteSnapshot
    destination::DestinationHash
    hops::UInt8
    via_identity::Union{Nothing,IdentityHash}
    interface_id::InterfaceId
    learned_at_millis::UInt64
    last_route_activity_at_millis::UInt64
    expires_at_millis::UInt64
end

struct DestinationIdentitySnapshot
    destination::DestinationHash
    identity::IdentityHash
end

struct RuntimeHealthSnapshot
    running::Bool
    uptime_millis::UInt64
    interface_count::UInt32
    online_interface_count::UInt32
    route_count::UInt32
    link_count::UInt32
    transported_link_count::UInt32
    rx_bytes::UInt64
    tx_bytes::UInt64
    rx_bps::UInt64
    tx_bps::UInt64
end

struct PersistenceSnapshot
    persistent::Bool
    restored::Bool
    last_flush_cause::Union{Nothing,PersistenceFlushCause}
    last_failure_detail::Union{Nothing,String}
end

struct HostSnapshot
    revision::UInt64
    backend::BackendInfo
    interfaces::Vector{InterfaceSnapshot}
    routes::Vector{RouteSnapshot}
    active_link_count::UInt32
    destination_identities::Vector{DestinationIdentitySnapshot}
    runtime::RuntimeHealthSnapshot
    persistence::PersistenceSnapshot
end

abstract type ResourceStream end

abstract type IdentityConfig end

abstract type PersistenceConfig end

abstract type InterfaceConfig end

abstract type DestinationIdentityConfig end

abstract type Bitrate end

abstract type ResponseTimeout end

abstract type ResourceCompression end

abstract type ResourceStrategy end

abstract type DestinationConfig end

abstract type HostCommand end

abstract type CommandOutcome end

abstract type CommandFailure end

abstract type ApplicationEvent end

abstract type DiagnosticEvent end

struct IdentityConfigExisting <: IdentityConfig
    secret::IdentitySecret
end

struct IdentityConfigGenerateEphemeral <: IdentityConfig
end

struct IdentityConfigLoadOrCreate <: IdentityConfig
    path::String
end

struct PersistenceConfigEphemeral <: PersistenceConfig
end

struct PersistenceConfigDirectory <: PersistenceConfig
    path::String
end

struct InterfaceConfigAutoLan <: InterfaceConfig
    group_id::Union{Nothing,String}
    discovery_scope::Union{Nothing,DiscoveryScope}
    discovery_port::Union{Nothing,UInt16}
    data_port::Union{Nothing,UInt16}
    devices::Vector{String}
    ignored_devices::Vector{String}
    multicast_address_type::Union{Nothing,MulticastAddressType}
end

struct InterfaceConfigTcpClient <: InterfaceConfig
    target::String
    bitrate::Bitrate
end

struct InterfaceConfigTcpServer <: InterfaceConfig
    bind::String
    bitrate::Bitrate
end

struct InterfaceConfigUdp <: InterfaceConfig
    var"local"::String
    peer::String
    bitrate::Bitrate
end

struct InterfaceConfigSerial <: InterfaceConfig
    port::String
    line::SerialLineConfig
end

struct InterfaceConfigKiss <: InterfaceConfig
    port::String
    line::SerialLineConfig
    flow_control::Bool
    preamble_millis::UInt32
    transmit_tail_millis::UInt32
    persistence::UInt8
    slot_time_millis::UInt32
    station_callsign::Union{Nothing,String}
    station_interval_seconds::Union{Nothing,UInt64}
end

struct InterfaceConfigAx25Kiss <: InterfaceConfig
    port::String
    line::SerialLineConfig
    flow_control::Bool
    preamble_millis::UInt32
    transmit_tail_millis::UInt32
    persistence::UInt8
    slot_time_millis::UInt32
    callsign::String
    ssid::UInt8
end

struct InterfaceConfigRNode <: InterfaceConfig
    port::String
    radio::RNodeRadioConfig
    flow_control::Bool
    station_callsign::Union{Nothing,String}
    station_interval_seconds::Union{Nothing,UInt64}
    airtime_limit_short_centi_percent::Union{Nothing,UInt16}
    airtime_limit_long_centi_percent::Union{Nothing,UInt16}
end

struct InterfaceConfigMultiRNode <: InterfaceConfig
    port::String
    station_callsign::Union{Nothing,String}
    station_interval_seconds::Union{Nothing,UInt64}
    members::Vector{MultiRNodeMemberConfig}
end

struct InterfaceConfigPipe <: InterfaceConfig
    command::Vector{String}
    respawn_delay_millis::UInt64
end

struct InterfaceConfigBackboneClient <: InterfaceConfig
    target::String
    bitrate::Bitrate
end

struct InterfaceConfigBackboneServer <: InterfaceConfig
    bind::String
    bitrate::Bitrate
end

struct InterfaceConfigI2p <: InterfaceConfig
    peers::Vector{String}
    connectable::Bool
end

struct InterfaceConfigWeave <: InterfaceConfig
    port::String
end

struct InterfaceConfigAutomaticUsb <: InterfaceConfig
end

struct InterfaceConfigAutomaticBluetoothLe <: InterfaceConfig
end

struct InterfaceConfigWebSocketClient <: InterfaceConfig
    target::String
    framing::WebSocketFramingSelection
end

struct InterfaceConfigWebSocketServer <: InterfaceConfig
    bind::String
    framing::WebSocketFramingSelection
end

struct InterfaceConfigBrowserRendezvous <: InterfaceConfig
    url::String
end

struct DestinationIdentityConfigHostIdentity <: DestinationIdentityConfig
end

struct DestinationIdentityConfigDedicatedIdentity <: DestinationIdentityConfig
    identity::IdentityConfig
end

struct BitrateAuto <: Bitrate
end

struct BitrateBitsPerSecond <: Bitrate
    value::UInt64
end

struct ResponseTimeoutLinkDefault <: ResponseTimeout
end

struct ResponseTimeoutExact <: ResponseTimeout
    millis::UInt64
end

struct ResourceCompressionAuto <: ResourceCompression
end

struct ResourceCompressionNever <: ResourceCompression
end

struct ResourceStrategyRefuse <: ResourceStrategy
end

struct ResourceStrategyAccept <: ResourceStrategy
    maximum_uncompressed_bytes::UInt64
    accept_compressed::Bool
end

struct DestinationConfigPlain <: DestinationConfig
    name::DestinationName
end

struct DestinationConfigSingle <: DestinationConfig
    name::DestinationName
    identity::DestinationIdentityConfig
    announce_app_data::Union{Nothing,Vector{UInt8}}
    maximum_request_bytes::Union{Nothing,UInt64}
    request_handlers::Vector{RequestHandlerConfig}
end

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
    maximum_response_bytes::Union{Nothing,UInt64}
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

struct HostCommandAttachInterface <: HostCommand
    config::InterfaceConfig
    routing::Union{Nothing,InterfaceRoutingPolicy}
end

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

struct CommandFailureInvalidConfiguration <: CommandFailure
    detail::String
end

struct CommandFailureResourceUploadCancelled <: CommandFailure
end

struct CommandFailureResourceEarlyEof <: CommandFailure
end

struct CommandFailureResourceLengthOverrun <: CommandFailure
end

struct CommandFailurePermissionDenied <: CommandFailure
    detail::String
end

struct CommandFailureDeviceUnavailable <: CommandFailure
    detail::String
end

struct CommandFailureConnectFailed <: CommandFailure
    detail::String
end

struct CommandFailureBackendFailed <: CommandFailure
    detail::String
end

struct CommandFailureResponseTooLarge <: CommandFailure
end

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

struct ApplicationEventLinkDelivery <: ApplicationEvent
    link_id::LinkId
    source_interface::InterfaceId
    plaintext::Vector{UInt8}
end

struct DiagnosticEventAnnounceHeard <: DiagnosticEvent
    destination::DestinationHash
    hops::UInt8
    source_interface::InterfaceId
    app_data::Vector{UInt8}
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

struct DiagnosticEventPersistenceRestored <: DiagnosticEvent
    routes::UInt64
    destination_identities::UInt64
    tunnels::UInt64
    ratchets::UInt64
    refused::UInt64
    dropped::UInt64
end

struct DiagnosticEventPersistenceFlushed <: DiagnosticEvent
    cause::PersistenceFlushCause
    target::PersistenceFlushTarget
end

struct DiagnosticEventPersistenceFlushFailed <: DiagnosticEvent
    cause::PersistenceFlushCause
    target::PersistenceFlushTarget
end

const HOST_OPERATION_NAMES = (
    :contract_info,
    :backend_info,
    :host_create,
    :host_release,
    :host_lifecycle,
    :host_snapshot,
    :host_snapshot_read,
    :host_snapshot_release,
    :host_identity_hash,
    :host_destination_count,
    :host_destination_hash,
    :host_attach_supplied_pipe,
    :supplied_pipe_claim_attachment,
    :supplied_pipe_next_open_request,
    :supplied_pipe_register_readiness,
    :supplied_pipe_interrupt_wait,
    :supplied_pipe_release,
    :supplied_pipe_open_request_provide,
    :supplied_pipe_open_request_decline,
    :supplied_pipe_open_request_release,
    :host_begin_resource_upload,
    :resource_upload_write,
    :resource_upload_is_writable,
    :resource_upload_finish,
    :resource_upload_abort,
    :resource_upload_release,
    :host_stop,
    :command_wait,
    :command_register_readiness,
    :command_interrupt_wait,
    :command_release,
    :host_claim_application_events,
    :host_claim_diagnostics,
    :event_stream_register_readiness,
    :readiness_registration_release,
    :event_stream_interrupt_wait,
    :event_stream_release,
    :event_stream_next,
    :event_release,
    :event_kind,
    :event_bytes,
    :event_string,
    :event_u64,
    :event_u128,
    :event_resource_stream,
    :resource_stream_release,
    :resource_stream_next,
    :host_announce,
    :host_send_single_packet,
    :host_close_link,
    :host_attach_tcp_server,
    :host_attach_tcp_client,
    :host_attach_udp,
    :host_detach_interface,
    :host_establish_link,
    :host_request_path,
    :host_identify,
    :host_send_link_packet,
    :host_request,
    :host_respond,
    :host_send_resource,
    :host_set_link_resource_strategy,
    :host_set_destination_resource_strategy,
    :host_send_channel_message,
    :host_allow_requester,
    :host_attach_interface,
)

struct RawUnit end
struct RawOwned{Value}; value::Value; end
struct RawBorrowed{Value}; value::Value; end
abstract type RawCallResult{Value} end
struct RawCallSuccess{Value} <: RawCallResult{Value}; value::Value; end
struct RawCallFailure{Value} <: RawCallResult{Value}; error::Status; end
struct RawCommandResult end
struct RawContractInfo end
struct RawEvent end
struct RawEventStream end
struct RawHost end
struct RawHostInspection end
struct RawHostOptions end
struct RawIssuedCommand end
struct RawLifecycle end
struct RawReadinessCallback end
struct RawReadinessRegistration end
struct RawResourceChunk end
struct RawResourceStream end
struct RawResourceUpload end
struct RawSuppliedPipe end
struct RawSuppliedPipeOpenRequest end
struct RawOpaquePointer end

abstract type RawHostProtocol end

function contract_info(protocol::RawHostProtocol)::RawCallResult{RawContractInfo}
    throw(MethodError(contract_info, (protocol,)))
end

function backend_info(protocol::RawHostProtocol)::RawCallResult{BackendInfo}
    throw(MethodError(backend_info, (protocol,)))
end

function host_create(protocol::RawHostProtocol, options::RawHostOptions)::RawCallResult{RawOwned{RawHost}}
    throw(MethodError(host_create, (protocol,)))
end

function host_release(protocol::RawHostProtocol, host::RawHost)::RawUnit
    throw(MethodError(host_release, (protocol,)))
end

function host_lifecycle(protocol::RawHostProtocol, host::RawHost)::RawCallResult{RawLifecycle}
    throw(MethodError(host_lifecycle, (protocol,)))
end

function host_snapshot(protocol::RawHostProtocol, host::RawHost, timeout_millis::UInt32)::RawCallResult{RawOwned{RawHostInspection}}
    throw(MethodError(host_snapshot, (protocol,)))
end

function host_snapshot_read(protocol::RawHostProtocol, host_inspection::RawHostInspection)::RawCallResult{RawBorrowed{HostSnapshot}}
    throw(MethodError(host_snapshot_read, (protocol,)))
end

function host_snapshot_release(protocol::RawHostProtocol, host_inspection::RawHostInspection)::RawUnit
    throw(MethodError(host_snapshot_release, (protocol,)))
end

function host_identity_hash(protocol::RawHostProtocol, host::RawHost)::RawCallResult{RawBorrowed{Vector{UInt8}}}
    throw(MethodError(host_identity_hash, (protocol,)))
end

function host_destination_count(protocol::RawHostProtocol, host::RawHost)::UInt
    throw(MethodError(host_destination_count, (protocol,)))
end

function host_destination_hash(protocol::RawHostProtocol, host::RawHost, index::UInt)::RawCallResult{RawBorrowed{Vector{UInt8}}}
    throw(MethodError(host_destination_hash, (protocol,)))
end

function host_attach_supplied_pipe(protocol::RawHostProtocol, host::RawHost, name::String, respawn_delay_millis::UInt64, bitrate::Bitrate)::RawCallResult{RawOwned{RawSuppliedPipe}}
    throw(MethodError(host_attach_supplied_pipe, (protocol,)))
end

function supplied_pipe_claim_attachment(protocol::RawHostProtocol, supplied_pipe::RawSuppliedPipe)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(supplied_pipe_claim_attachment, (protocol,)))
end

function supplied_pipe_next_open_request(protocol::RawHostProtocol, supplied_pipe::RawSuppliedPipe, timeout_millis::UInt32)::RawCallResult{RawOwned{RawSuppliedPipeOpenRequest}}
    throw(MethodError(supplied_pipe_next_open_request, (protocol,)))
end

function supplied_pipe_register_readiness(protocol::RawHostProtocol, supplied_pipe::RawSuppliedPipe, callback::RawReadinessCallback, context::RawOpaquePointer)::RawCallResult{RawOwned{RawReadinessRegistration}}
    throw(MethodError(supplied_pipe_register_readiness, (protocol,)))
end

function supplied_pipe_interrupt_wait(protocol::RawHostProtocol, supplied_pipe::RawSuppliedPipe)::RawUnit
    throw(MethodError(supplied_pipe_interrupt_wait, (protocol,)))
end

function supplied_pipe_release(protocol::RawHostProtocol, supplied_pipe::RawSuppliedPipe)::RawUnit
    throw(MethodError(supplied_pipe_release, (protocol,)))
end

function supplied_pipe_open_request_provide(protocol::RawHostProtocol, supplied_pipe_open_request::RawSuppliedPipeOpenRequest, descriptor::Int64)::RawCallResult{Bool}
    throw(MethodError(supplied_pipe_open_request_provide, (protocol,)))
end

function supplied_pipe_open_request_decline(protocol::RawHostProtocol, supplied_pipe_open_request::RawSuppliedPipeOpenRequest)::RawCallResult{Bool}
    throw(MethodError(supplied_pipe_open_request_decline, (protocol,)))
end

function supplied_pipe_open_request_release(protocol::RawHostProtocol, supplied_pipe_open_request::RawSuppliedPipeOpenRequest)::RawUnit
    throw(MethodError(supplied_pipe_open_request_release, (protocol,)))
end

function host_begin_resource_upload(protocol::RawHostProtocol, host::RawHost, link_id::LinkId, declared_length::UInt64, packed_metadata::Union{Nothing,Vector{UInt8}}, compression::ResourceCompression)::RawCallResult{RawOwned{RawResourceUpload}}
    throw(MethodError(host_begin_resource_upload, (protocol,)))
end

function resource_upload_write(protocol::RawHostProtocol, resource_upload::RawResourceUpload, chunk::Vector{UInt8})::RawCallResult{RawUnit}
    throw(MethodError(resource_upload_write, (protocol,)))
end

function resource_upload_is_writable(protocol::RawHostProtocol, resource_upload::RawResourceUpload)::RawCallResult{Bool}
    throw(MethodError(resource_upload_is_writable, (protocol,)))
end

function resource_upload_finish(protocol::RawHostProtocol, resource_upload::RawResourceUpload)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(resource_upload_finish, (protocol,)))
end

function resource_upload_abort(protocol::RawHostProtocol, resource_upload::RawResourceUpload)::RawUnit
    throw(MethodError(resource_upload_abort, (protocol,)))
end

function resource_upload_release(protocol::RawHostProtocol, resource_upload::RawResourceUpload)::RawUnit
    throw(MethodError(resource_upload_release, (protocol,)))
end

function host_stop(protocol::RawHostProtocol, host::RawHost)::RawCallResult{RawUnit}
    throw(MethodError(host_stop, (protocol,)))
end

function command_wait(protocol::RawHostProtocol, issued_command::RawIssuedCommand, timeout_millis::UInt32)::RawCallResult{RawBorrowed{RawCommandResult}}
    throw(MethodError(command_wait, (protocol,)))
end

function command_register_readiness(protocol::RawHostProtocol, issued_command::RawIssuedCommand, callback::RawReadinessCallback, context::RawOpaquePointer)::RawCallResult{RawOwned{RawReadinessRegistration}}
    throw(MethodError(command_register_readiness, (protocol,)))
end

function command_interrupt_wait(protocol::RawHostProtocol, issued_command::RawIssuedCommand)::RawUnit
    throw(MethodError(command_interrupt_wait, (protocol,)))
end

function command_release(protocol::RawHostProtocol, issued_command::RawIssuedCommand)::RawUnit
    throw(MethodError(command_release, (protocol,)))
end

function host_claim_application_events(protocol::RawHostProtocol, host::RawHost)::RawCallResult{RawOwned{RawEventStream}}
    throw(MethodError(host_claim_application_events, (protocol,)))
end

function host_claim_diagnostics(protocol::RawHostProtocol, host::RawHost)::RawCallResult{RawOwned{RawEventStream}}
    throw(MethodError(host_claim_diagnostics, (protocol,)))
end

function event_stream_register_readiness(protocol::RawHostProtocol, event_stream::RawEventStream, callback::RawReadinessCallback, context::RawOpaquePointer)::RawCallResult{RawOwned{RawReadinessRegistration}}
    throw(MethodError(event_stream_register_readiness, (protocol,)))
end

function readiness_registration_release(protocol::RawHostProtocol, readiness_registration::RawReadinessRegistration)::RawUnit
    throw(MethodError(readiness_registration_release, (protocol,)))
end

function event_stream_interrupt_wait(protocol::RawHostProtocol, event_stream::RawEventStream)::RawUnit
    throw(MethodError(event_stream_interrupt_wait, (protocol,)))
end

function event_stream_release(protocol::RawHostProtocol, event_stream::RawEventStream)::RawUnit
    throw(MethodError(event_stream_release, (protocol,)))
end

function event_stream_next(protocol::RawHostProtocol, event_stream::RawEventStream, timeout_millis::UInt32)::RawCallResult{RawOwned{RawEvent}}
    throw(MethodError(event_stream_next, (protocol,)))
end

function event_release(protocol::RawHostProtocol, event::RawEvent)::RawUnit
    throw(MethodError(event_release, (protocol,)))
end

function event_kind(protocol::RawHostProtocol, event::RawEvent)::UInt32
    throw(MethodError(event_kind, (protocol,)))
end

function event_bytes(protocol::RawHostProtocol, event::RawEvent, field::EventField)::RawCallResult{RawBorrowed{Vector{UInt8}}}
    throw(MethodError(event_bytes, (protocol,)))
end

function event_string(protocol::RawHostProtocol, event::RawEvent, field::EventField)::RawCallResult{RawBorrowed{String}}
    throw(MethodError(event_string, (protocol,)))
end

function event_u64(protocol::RawHostProtocol, event::RawEvent, field::EventField)::RawCallResult{UInt64}
    throw(MethodError(event_u64, (protocol,)))
end

function event_u128(protocol::RawHostProtocol, event::RawEvent, field::EventField)::RawCallResult{UInt128}
    throw(MethodError(event_u128, (protocol,)))
end

function event_resource_stream(protocol::RawHostProtocol, event::RawEvent)::RawCallResult{RawOwned{RawResourceStream}}
    throw(MethodError(event_resource_stream, (protocol,)))
end

function resource_stream_release(protocol::RawHostProtocol, resource_stream::RawResourceStream)::RawUnit
    throw(MethodError(resource_stream_release, (protocol,)))
end

function resource_stream_next(protocol::RawHostProtocol, resource_stream::RawResourceStream, maximum_bytes::UInt)::RawCallResult{RawBorrowed{RawResourceChunk}}
    throw(MethodError(resource_stream_next, (protocol,)))
end

function host_announce(protocol::RawHostProtocol, host::RawHost, destination::DestinationHash, interface::Union{Nothing,InterfaceId})::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_announce, (protocol,)))
end

function host_send_single_packet(protocol::RawHostProtocol, host::RawHost, destination::DestinationHash, payload::Vector{UInt8})::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_send_single_packet, (protocol,)))
end

function host_close_link(protocol::RawHostProtocol, host::RawHost, link_id::LinkId)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_close_link, (protocol,)))
end

function host_attach_tcp_server(protocol::RawHostProtocol, host::RawHost, bind::String, bitrate::Bitrate)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_attach_tcp_server, (protocol,)))
end

function host_attach_tcp_client(protocol::RawHostProtocol, host::RawHost, target::String, bitrate::Bitrate)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_attach_tcp_client, (protocol,)))
end

function host_attach_udp(protocol::RawHostProtocol, host::RawHost, var"local"::String, peer::String, bitrate::Bitrate)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_attach_udp, (protocol,)))
end

function host_detach_interface(protocol::RawHostProtocol, host::RawHost, interface::InterfaceId)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_detach_interface, (protocol,)))
end

function host_establish_link(protocol::RawHostProtocol, host::RawHost, destination::DestinationHash)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_establish_link, (protocol,)))
end

function host_request_path(protocol::RawHostProtocol, host::RawHost, destination::DestinationHash)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_request_path, (protocol,)))
end

function host_identify(protocol::RawHostProtocol, host::RawHost, link_id::LinkId, identity::IdentityHash)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_identify, (protocol,)))
end

function host_send_link_packet(protocol::RawHostProtocol, host::RawHost, link_id::LinkId, payload::Vector{UInt8})::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_send_link_packet, (protocol,)))
end

function host_request(protocol::RawHostProtocol, host::RawHost, link_id::LinkId, path_hash::RequestPathHash, payload::Vector{UInt8}, timeout::ResponseTimeout, maximum_response_bytes::Union{Nothing,UInt64})::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_request, (protocol,)))
end

function host_respond(protocol::RawHostProtocol, host::RawHost, link_id::LinkId, request_id::RequestId, request_rtt_millis::UInt64, payload::Vector{UInt8})::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_respond, (protocol,)))
end

function host_send_resource(protocol::RawHostProtocol, host::RawHost, link_id::LinkId, payload::Vector{UInt8}, packed_metadata::Union{Nothing,Vector{UInt8}}, compression::ResourceCompression)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_send_resource, (protocol,)))
end

function host_set_link_resource_strategy(protocol::RawHostProtocol, host::RawHost, link_id::LinkId, strategy::ResourceStrategy)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_set_link_resource_strategy, (protocol,)))
end

function host_set_destination_resource_strategy(protocol::RawHostProtocol, host::RawHost, destination::DestinationHash, strategy::ResourceStrategy)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_set_destination_resource_strategy, (protocol,)))
end

function host_send_channel_message(protocol::RawHostProtocol, host::RawHost, link_id::LinkId, message_type::UInt16, payload::Vector{UInt8})::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_send_channel_message, (protocol,)))
end

function host_allow_requester(protocol::RawHostProtocol, host::RawHost, destination::DestinationHash, path_hash::RequestPathHash, identity::IdentityHash)::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_allow_requester, (protocol,)))
end

function host_attach_interface(protocol::RawHostProtocol, host::RawHost, config::InterfaceConfig, routing::Union{Nothing,InterfaceRoutingPolicy})::RawCallResult{RawOwned{RawIssuedCommand}}
    throw(MethodError(host_attach_interface, (protocol,)))
end
