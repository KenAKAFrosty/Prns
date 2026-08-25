package prns

const (
	HostContractABI uint32 = 1
	HostSchemaVersion uint32 = 1
	ProductVersion = "0.3.7"
)

const DestinationHashLength = 16
const IdentityHashLength = 16
const InterfaceIdLength = 8
const LinkIdLength = 16
const PacketHashLength = 32
const RequestIdLength = 16
const RequestPathHashLength = 16
const ResourceHashLength = 32
const IdentitySecretLength = 64
const SafeIntMin int64 = -9007199254740991
const SafeIntMax int64 = 9007199254740991
const SafeUintMax uint64 = 9007199254740991
const BalancedPendingCommands = 256
const BalancedApplicationEvents = 1024
const BalancedRetainedEventBytes = 8388608
const BalancedDiagnostics = 1024

type UInt128 struct {
	Low uint64
	High uint64
}

type Status uint32

const (
	StatusOk Status = 0
	StatusInvalidArgument Status = 1
	StatusContractMismatch Status = 2
	StatusInvalidHandle Status = 3
	StatusNotReady Status = 4
	StatusAlreadyClaimed Status = 5
	StatusWouldBlock Status = 6
	StatusTimedOut Status = 7
	StatusQueueFull Status = 8
	StatusStopped Status = 9
	StatusBackendFailed Status = 10
	StatusPanic Status = 11
	StatusInterrupted Status = 12
	StatusUnsupported Status = 13
	StatusPermissionDenied Status = 14
	StatusUnavailable Status = 15
)

type BackendKind uint32

const (
	BackendKindNative BackendKind = 1
	BackendKindBrowser BackendKind = 2
	BackendKindCooperative BackendKind = 3
)

type Capability uint32

const (
	CapabilityLoopback Capability = 1
	CapabilityTcpClient Capability = 2
	CapabilityTcpServer Capability = 3
	CapabilityUdp Capability = 4
	CapabilitySerial Capability = 5
	CapabilityUsb Capability = 6
	CapabilityBluetooth Capability = 7
	CapabilityWifi Capability = 8
	CapabilityWebSocket Capability = 9
	CapabilityBrowserRendezvous Capability = 10
	CapabilityI2p Capability = 11
	CapabilityWeave Capability = 12
	CapabilitySuppliedPipe Capability = 13
)

type InterfaceKind uint32

const (
	InterfaceKindAutoLan InterfaceKind = 1
	InterfaceKindTcpClient InterfaceKind = 2
	InterfaceKindTcpServer InterfaceKind = 3
	InterfaceKindUdp InterfaceKind = 4
	InterfaceKindSerial InterfaceKind = 5
	InterfaceKindKiss InterfaceKind = 6
	InterfaceKindAx25Kiss InterfaceKind = 7
	InterfaceKindRNode InterfaceKind = 8
	InterfaceKindMultiRNode InterfaceKind = 9
	InterfaceKindPipe InterfaceKind = 10
	InterfaceKindBackboneClient InterfaceKind = 11
	InterfaceKindBackboneServer InterfaceKind = 12
	InterfaceKindI2p InterfaceKind = 13
	InterfaceKindWeave InterfaceKind = 14
	InterfaceKindAutomaticUsb InterfaceKind = 15
	InterfaceKindAutomaticBluetoothLe InterfaceKind = 16
	InterfaceKindWebSocketClient InterfaceKind = 17
	InterfaceKindWebSocketServer InterfaceKind = 18
	InterfaceKindBrowserRendezvous InterfaceKind = 19
)

type InterfaceMode uint32

const (
	InterfaceModeFull InterfaceMode = 1
	InterfaceModePointToPoint InterfaceMode = 2
	InterfaceModeAccessPoint InterfaceMode = 3
	InterfaceModeRoaming InterfaceMode = 4
	InterfaceModeBoundary InterfaceMode = 5
	InterfaceModeGateway InterfaceMode = 6
	InterfaceModeInternal InterfaceMode = 7
)

type WebSocketFramingSelection uint32

const (
	WebSocketFramingSelectionRawPacket WebSocketFramingSelection = 1
	WebSocketFramingSelectionHdlc WebSocketFramingSelection = 2
	WebSocketFramingSelectionKiss WebSocketFramingSelection = 3
	WebSocketFramingSelectionAuto WebSocketFramingSelection = 4
)

type InterfaceHealth uint32

const (
	InterfaceHealthInitializing InterfaceHealth = 1
	InterfaceHealthConnected InterfaceHealth = 2
	InterfaceHealthDegraded InterfaceHealth = 3
	InterfaceHealthReconnecting InterfaceHealth = 4
	InterfaceHealthFailed InterfaceHealth = 5
	InterfaceHealthDisconnected InterfaceHealth = 6
	InterfaceHealthDisabled InterfaceHealth = 7
	InterfaceHealthUnknown InterfaceHealth = 8
)

type DiscoveryScope uint32

const (
	DiscoveryScopeLink DiscoveryScope = 1
	DiscoveryScopeAdmin DiscoveryScope = 2
	DiscoveryScopeSite DiscoveryScope = 3
	DiscoveryScopeOrganization DiscoveryScope = 4
	DiscoveryScopeGlobal DiscoveryScope = 5
)

type MulticastAddressType uint32

const (
	MulticastAddressTypeTemporary MulticastAddressType = 1
	MulticastAddressTypePermanent MulticastAddressType = 2
)

type SerialDataBits uint32

const (
	SerialDataBitsFive SerialDataBits = 5
	SerialDataBitsSix SerialDataBits = 6
	SerialDataBitsSeven SerialDataBits = 7
	SerialDataBitsEight SerialDataBits = 8
)

type SerialParity uint32

const (
	SerialParityNone SerialParity = 1
	SerialParityEven SerialParity = 2
	SerialParityOdd SerialParity = 3
)

type SerialStopBits uint32

const (
	SerialStopBitsOne SerialStopBits = 1
	SerialStopBitsTwo SerialStopBits = 2
)

type HostRole uint32

const (
	HostRoleEndpoint HostRole = 1
	HostRoleTransport HostRole = 2
)

type IdentityConfigKind uint32

const (
	IdentityConfigKindExisting IdentityConfigKind = 1
	IdentityConfigKindGenerateEphemeral IdentityConfigKind = 2
	IdentityConfigKindLoadOrCreate IdentityConfigKind = 3
)

type PersistenceConfigKind uint32

const (
	PersistenceConfigKindEphemeral PersistenceConfigKind = 1
	PersistenceConfigKindDirectory PersistenceConfigKind = 2
)

type DestinationConfigKind uint32

const (
	DestinationConfigKindPlain DestinationConfigKind = 1
	DestinationConfigKindSingle DestinationConfigKind = 2
)

type DestinationIdentityConfigKind uint32

const (
	DestinationIdentityConfigKindHostIdentity DestinationIdentityConfigKind = 1
	DestinationIdentityConfigKindDedicatedIdentity DestinationIdentityConfigKind = 2
)

type BitrateKind uint32

const (
	BitrateKindAuto BitrateKind = 1
	BitrateKindBitsPerSecond BitrateKind = 2
)

type ResponseTimeoutKind uint32

const (
	ResponseTimeoutKindLinkDefault ResponseTimeoutKind = 1
	ResponseTimeoutKindExact ResponseTimeoutKind = 2
)

type ResourceCompressionKind uint32

const (
	ResourceCompressionKindAuto ResourceCompressionKind = 1
	ResourceCompressionKindNever ResourceCompressionKind = 2
)

type ResourceStrategyKind uint32

const (
	ResourceStrategyKindRefuse ResourceStrategyKind = 1
	ResourceStrategyKindAccept ResourceStrategyKind = 2
)

type RequestPolicy uint32

const (
	RequestPolicyAllowNone RequestPolicy = 1
	RequestPolicyAllowAll RequestPolicy = 2
	RequestPolicyAllowList RequestPolicy = 3
)

type CommandOutcomeKind uint32

const (
	CommandOutcomeKindAnnounced CommandOutcomeKind = 1
	CommandOutcomeKindPacketDelivered CommandOutcomeKind = 2
	CommandOutcomeKindLinkCloseQueued CommandOutcomeKind = 3
	CommandOutcomeKindInterfaceAttached CommandOutcomeKind = 4
	CommandOutcomeKindInterfaceDetached CommandOutcomeKind = 5
	CommandOutcomeKindLinkEstablished CommandOutcomeKind = 6
	CommandOutcomeKindPathDiscovered CommandOutcomeKind = 7
	CommandOutcomeKindIdentified CommandOutcomeKind = 8
	CommandOutcomeKindResponseReceived CommandOutcomeKind = 9
	CommandOutcomeKindResponseSent CommandOutcomeKind = 10
	CommandOutcomeKindResourceSent CommandOutcomeKind = 11
	CommandOutcomeKindResourceStrategySet CommandOutcomeKind = 12
	CommandOutcomeKindRequesterAllowed CommandOutcomeKind = 13
)

type CommandFailureKind uint32

const (
	CommandFailureKindNodeStopped CommandFailureKind = 1
	CommandFailureKindBusy CommandFailureKind = 2
	CommandFailureKindPayloadTooLarge CommandFailureKind = 3
	CommandFailureKindUnknownDestination CommandFailureKind = 4
	CommandFailureKindNotSingleDestination CommandFailureKind = 5
	CommandFailureKindAnnounceAppDataTooLong CommandFailureKind = 6
	CommandFailureKindUnknownInterface CommandFailureKind = 7
	CommandFailureKindNoRouteToDestination CommandFailureKind = 8
	CommandFailureKindNotDirectlyReachable CommandFailureKind = 9
	CommandFailureKindPacketCulled CommandFailureKind = 10
	CommandFailureKindDeliveryTimedOut CommandFailureKind = 11
	CommandFailureKindInvalidBitrate CommandFailureKind = 12
	CommandFailureKindBindFailed CommandFailureKind = 13
	CommandFailureKindWriteFailed CommandFailureKind = 14
	CommandFailureKindUnsupportedByBackend CommandFailureKind = 15
	CommandFailureKindUnknownLink CommandFailureKind = 16
	CommandFailureKindLinkNotActive CommandFailureKind = 17
	CommandFailureKindEntropyUnavailable CommandFailureKind = 18
	CommandFailureKindNotLinkInitiator CommandFailureKind = 19
	CommandFailureKindIdentityNotHeld CommandFailureKind = 20
	CommandFailureKindUnknownRequestHandler CommandFailureKind = 21
	CommandFailureKindRequestPolicyNotAllowList CommandFailureKind = 22
	CommandFailureKindRequestAllowListFull CommandFailureKind = 23
	CommandFailureKindLinkBusy CommandFailureKind = 24
	CommandFailureKindResourceTableFull CommandFailureKind = 25
	CommandFailureKindResourceMetadataTooLarge CommandFailureKind = 26
	CommandFailureKindResourceRejectedByPeer CommandFailureKind = 27
	CommandFailureKindResourceSequencingFailed CommandFailureKind = 28
	CommandFailureKindResourcePredecessorFailed CommandFailureKind = 29
	CommandFailureKindChannelWindowFull CommandFailureKind = 30
	CommandFailureKindChannelUntrackable CommandFailureKind = 31
	CommandFailureKindInvalidChannelMessageType CommandFailureKind = 32
	CommandFailureKindInvalidConfiguration CommandFailureKind = 33
	CommandFailureKindResourceUploadCancelled CommandFailureKind = 34
	CommandFailureKindResourceEarlyEof CommandFailureKind = 35
	CommandFailureKindResourceLengthOverrun CommandFailureKind = 36
	CommandFailureKindPermissionDenied CommandFailureKind = 37
	CommandFailureKindDeviceUnavailable CommandFailureKind = 38
	CommandFailureKindConnectFailed CommandFailureKind = 39
	CommandFailureKindBackendFailed CommandFailureKind = 40
	CommandFailureKindResponseTooLarge CommandFailureKind = 41
)

type DeliveryEvidenceKind uint32

const (
	DeliveryEvidenceKindExplicitProof DeliveryEvidenceKind = 1
	DeliveryEvidenceKindImplicitProof DeliveryEvidenceKind = 2
	DeliveryEvidenceKindResponse DeliveryEvidenceKind = 3
)

type LifecyclePhase uint32

const (
	LifecyclePhaseStarting LifecyclePhase = 1
	LifecyclePhaseRunning LifecyclePhase = 2
	LifecyclePhaseStopping LifecyclePhase = 3
	LifecyclePhaseStopped LifecyclePhase = 4
	LifecyclePhaseFailed LifecyclePhase = 5
)

type StopReason uint32

const (
	StopReasonRequested StopReason = 1
	StopReasonBackendExited StopReason = 2
)

type LinkClosedReason uint32

const (
	LinkClosedReasonTimeout LinkClosedReason = 1
	LinkClosedReasonPeerClosed LinkClosedReason = 2
	LinkClosedReasonMalformedRtt LinkClosedReason = 3
)

type ApplicationEventKind uint32

const (
	ApplicationEventKindSingleDelivery ApplicationEventKind = 100
	ApplicationEventKindRequest ApplicationEventKind = 101
	ApplicationEventKindResponse ApplicationEventKind = 102
	ApplicationEventKindResponseSegment ApplicationEventKind = 103
	ApplicationEventKindResourceAvailable ApplicationEventKind = 104
	ApplicationEventKindResourceSegment ApplicationEventKind = 105
	ApplicationEventKindResourceNeedsDecompression ApplicationEventKind = 106
	ApplicationEventKindChannelMessage ApplicationEventKind = 107
	ApplicationEventKindLinkDelivery ApplicationEventKind = 108
)

type DiagnosticEventKind uint32

const (
	DiagnosticEventKindAnnounceHeard DiagnosticEventKind = 200
	DiagnosticEventKindLinkEstablished DiagnosticEventKind = 201
	DiagnosticEventKindPeerIdentified DiagnosticEventKind = 202
	DiagnosticEventKindLinkClosed DiagnosticEventKind = 203
	DiagnosticEventKindLinkInterfaceMismatch DiagnosticEventKind = 204
	DiagnosticEventKindResourceAssembled DiagnosticEventKind = 205
	DiagnosticEventKindResourceFailed DiagnosticEventKind = 206
	DiagnosticEventKindResourceSendProgress DiagnosticEventKind = 207
	DiagnosticEventKindSelfRatchetRotated DiagnosticEventKind = 208
	DiagnosticEventKindAnnounceHeldDropped DiagnosticEventKind = 209
	DiagnosticEventKindDelivered DiagnosticEventKind = 210
	DiagnosticEventKindRouteExpired DiagnosticEventKind = 211
	DiagnosticEventKindRouteEvicted DiagnosticEventKind = 212
	DiagnosticEventKindRouteInterfaceGone DiagnosticEventKind = 213
	DiagnosticEventKindRouteDropped DiagnosticEventKind = 214
	DiagnosticEventKindBackendDiagnostic DiagnosticEventKind = 215
	DiagnosticEventKindDiagnosticsDropped DiagnosticEventKind = 216
	DiagnosticEventKindPersistenceRestored DiagnosticEventKind = 217
	DiagnosticEventKindPersistenceFlushed DiagnosticEventKind = 218
	DiagnosticEventKindPersistenceFlushFailed DiagnosticEventKind = 219
)

type PersistenceFlushCause uint32

const (
	PersistenceFlushCauseStartup PersistenceFlushCause = 1
	PersistenceFlushCauseInterval PersistenceFlushCause = 2
	PersistenceFlushCauseRouteChange PersistenceFlushCause = 3
	PersistenceFlushCauseRatchetRotation PersistenceFlushCause = 4
	PersistenceFlushCauseShutdown PersistenceFlushCause = 5
)

type PersistenceFlushTarget uint32

const (
	PersistenceFlushTargetRoutingState PersistenceFlushTarget = 1
	PersistenceFlushTargetRatchets PersistenceFlushTarget = 2
)

type EventField uint32

const (
	EventFieldDestination EventField = 1
	EventFieldSourceInterface EventField = 2
	EventFieldPlaintext EventField = 3
	EventFieldLinkId EventField = 4
	EventFieldRequestId EventField = 5
	EventFieldRequester EventField = 6
	EventFieldPathHash EventField = 7
	EventFieldRttMillis EventField = 8
	EventFieldData EventField = 9
	EventFieldSegmentIndex EventField = 10
	EventFieldTotalSegments EventField = 11
	EventFieldHash EventField = 12
	EventFieldOriginalHash EventField = 13
	EventFieldMetadata EventField = 14
	EventFieldTotalBytes EventField = 15
	EventFieldStreamId EventField = 16
	EventFieldUncompressedDataBytes EventField = 17
	EventFieldMessageType EventField = 18
	EventFieldIdentity EventField = 19
	EventFieldReason EventField = 20
	EventFieldAttachedInterface EventField = 21
	EventFieldArrivedOn EventField = 22
	EventFieldTotalSizeBytes EventField = 23
	EventFieldCause EventField = 24
	EventFieldTransferredBytes EventField = 25
	EventFieldPhysicalTransferredBytes EventField = 26
	EventFieldDetail EventField = 27
	EventFieldKind EventField = 28
	EventFieldDroppedCount EventField = 29
	EventFieldHops EventField = 30
	EventFieldStream EventField = 31
	EventFieldRoutes EventField = 32
	EventFieldDestinationIdentities EventField = 33
	EventFieldTunnels EventField = 34
	EventFieldRatchets EventField = 35
	EventFieldRefused EventField = 36
	EventFieldDropped EventField = 37
	EventFieldPersistenceCause EventField = 38
	EventFieldPersistenceTarget EventField = 39
	EventFieldAppData EventField = 40
)

type DestinationHash [DestinationHashLength]byte

type IdentityHash [IdentityHashLength]byte

type InterfaceId [InterfaceIdLength]byte

type LinkId [LinkIdLength]byte

type PacketHash [PacketHashLength]byte

type RequestId [RequestIdLength]byte

type RequestPathHash [RequestPathHashLength]byte

type ResourceHash [ResourceHashLength]byte

type IdentitySecret [IdentitySecretLength]byte

func (value *IdentitySecret) Close() {
	clear(value[:])
}

type DestinationName struct {
	AppName string
	Aspects []string
}

type RequestHandlerConfig struct {
	Path string
	Policy RequestPolicy
}

type SerialLineConfig struct {
	Baud uint32
	DataBits SerialDataBits
	Parity SerialParity
	StopBits SerialStopBits
}

type RNodeRadioConfig struct {
	FrequencyHz uint64
	BandwidthHz uint32
	TxPowerDbm int16
	SpreadingFactor uint8
	CodingRate uint8
}

type MultiRNodeMemberConfig struct {
	Name string
	VirtualPort uint8
	Radio RNodeRadioConfig
	FlowControl bool
	Outgoing bool
}

type InterfaceRoutingPolicy struct {
	Mode *InterfaceMode
	Gravity *int64
	RecursivePathRequests *bool
	AnnouncesFromInternal *bool
	AnnouncesToInternal *bool
}

type BackendInfo struct {
	Backend BackendKind
	Capabilities []Capability
	InterfaceKinds []InterfaceKind
}

type InterfaceSnapshot struct {
	InterfaceId InterfaceId
	Name *string
	Kind *InterfaceKind
	Health InterfaceHealth
	FailureDetail *string
	RxBytes uint64
	TxBytes uint64
	RxBps *uint64
	TxBps *uint64
	RouteCount uint32
	LinkCount uint32
	TransportedLinkCount uint32
}

type RouteSnapshot struct {
	Destination DestinationHash
	Hops uint8
	ViaIdentity *IdentityHash
	InterfaceId InterfaceId
	LearnedAtMillis uint64
	LastRouteActivityAtMillis uint64
	ExpiresAtMillis uint64
}

type DestinationIdentitySnapshot struct {
	Destination DestinationHash
	Identity IdentityHash
}

type RuntimeHealthSnapshot struct {
	Running bool
	UptimeMillis uint64
	InterfaceCount uint32
	OnlineInterfaceCount uint32
	RouteCount uint32
	LinkCount uint32
	TransportedLinkCount uint32
	RxBytes uint64
	TxBytes uint64
	RxBps uint64
	TxBps uint64
}

type PersistenceSnapshot struct {
	Persistent bool
	Restored bool
	LastFlushCause *PersistenceFlushCause
	LastFailureDetail *string
}

type HostSnapshot struct {
	Revision uint64
	Backend BackendInfo
	Interfaces []InterfaceSnapshot
	Routes []RouteSnapshot
	ActiveLinkCount uint32
	DestinationIdentities []DestinationIdentitySnapshot
	Runtime RuntimeHealthSnapshot
	Persistence PersistenceSnapshot
}

type ResourceStream interface {
	TotalBytes() uint64
	Next(maximumBytes int) ([]byte, bool, error)
	Close() error
}

type IdentityConfig interface {
	identityConfig()
}

type IdentityConfigExisting struct {
	Secret IdentitySecret
}

func (IdentityConfigExisting) identityConfig() {}

type IdentityConfigGenerateEphemeral struct{}

func (IdentityConfigGenerateEphemeral) identityConfig() {}

type IdentityConfigLoadOrCreate struct {
	Path string
}

func (IdentityConfigLoadOrCreate) identityConfig() {}

type PersistenceConfig interface {
	persistenceConfig()
}

type PersistenceConfigEphemeral struct{}

func (PersistenceConfigEphemeral) persistenceConfig() {}

type PersistenceConfigDirectory struct {
	Path string
}

func (PersistenceConfigDirectory) persistenceConfig() {}

type InterfaceConfig interface {
	interfaceConfig()
}

type InterfaceConfigAutoLan struct {
	GroupId *string
	DiscoveryScope *DiscoveryScope
	DiscoveryPort *uint16
	DataPort *uint16
	Devices []string
	IgnoredDevices []string
	MulticastAddressType *MulticastAddressType
}

func (InterfaceConfigAutoLan) interfaceConfig() {}

type InterfaceConfigTcpClient struct {
	Target string
	Bitrate Bitrate
}

func (InterfaceConfigTcpClient) interfaceConfig() {}

type InterfaceConfigTcpServer struct {
	Bind string
	Bitrate Bitrate
}

func (InterfaceConfigTcpServer) interfaceConfig() {}

type InterfaceConfigUdp struct {
	Local string
	Peer string
	Bitrate Bitrate
}

func (InterfaceConfigUdp) interfaceConfig() {}

type InterfaceConfigSerial struct {
	Port string
	Line SerialLineConfig
}

func (InterfaceConfigSerial) interfaceConfig() {}

type InterfaceConfigKiss struct {
	Port string
	Line SerialLineConfig
	FlowControl bool
	PreambleMillis uint32
	TransmitTailMillis uint32
	Persistence uint8
	SlotTimeMillis uint32
	StationCallsign *string
	StationIntervalSeconds *uint64
}

func (InterfaceConfigKiss) interfaceConfig() {}

type InterfaceConfigAx25Kiss struct {
	Port string
	Line SerialLineConfig
	FlowControl bool
	PreambleMillis uint32
	TransmitTailMillis uint32
	Persistence uint8
	SlotTimeMillis uint32
	Callsign string
	Ssid uint8
}

func (InterfaceConfigAx25Kiss) interfaceConfig() {}

type InterfaceConfigRNode struct {
	Port string
	Radio RNodeRadioConfig
	FlowControl bool
	StationCallsign *string
	StationIntervalSeconds *uint64
	AirtimeLimitShortCentiPercent *uint16
	AirtimeLimitLongCentiPercent *uint16
}

func (InterfaceConfigRNode) interfaceConfig() {}

type InterfaceConfigMultiRNode struct {
	Port string
	StationCallsign *string
	StationIntervalSeconds *uint64
	Members []MultiRNodeMemberConfig
}

func (InterfaceConfigMultiRNode) interfaceConfig() {}

type InterfaceConfigPipe struct {
	Command []string
	RespawnDelayMillis uint64
}

func (InterfaceConfigPipe) interfaceConfig() {}

type InterfaceConfigBackboneClient struct {
	Target string
	Bitrate Bitrate
}

func (InterfaceConfigBackboneClient) interfaceConfig() {}

type InterfaceConfigBackboneServer struct {
	Bind string
	Bitrate Bitrate
}

func (InterfaceConfigBackboneServer) interfaceConfig() {}

type InterfaceConfigI2p struct {
	Peers []string
	Connectable bool
}

func (InterfaceConfigI2p) interfaceConfig() {}

type InterfaceConfigWeave struct {
	Port string
}

func (InterfaceConfigWeave) interfaceConfig() {}

type InterfaceConfigAutomaticUsb struct{}

func (InterfaceConfigAutomaticUsb) interfaceConfig() {}

type InterfaceConfigAutomaticBluetoothLe struct{}

func (InterfaceConfigAutomaticBluetoothLe) interfaceConfig() {}

type InterfaceConfigWebSocketClient struct {
	Target string
	Framing WebSocketFramingSelection
}

func (InterfaceConfigWebSocketClient) interfaceConfig() {}

type InterfaceConfigWebSocketServer struct {
	Bind string
	Framing WebSocketFramingSelection
}

func (InterfaceConfigWebSocketServer) interfaceConfig() {}

type InterfaceConfigBrowserRendezvous struct {
	Url string
}

func (InterfaceConfigBrowserRendezvous) interfaceConfig() {}

type DestinationIdentityConfig interface {
	destinationIdentityConfig()
}

type DestinationIdentityConfigHostIdentity struct{}

func (DestinationIdentityConfigHostIdentity) destinationIdentityConfig() {}

type DestinationIdentityConfigDedicatedIdentity struct {
	Identity IdentityConfig
}

func (DestinationIdentityConfigDedicatedIdentity) destinationIdentityConfig() {}

type Bitrate interface {
	bitrate()
}

type BitrateAuto struct{}

func (BitrateAuto) bitrate() {}

type BitrateBitsPerSecond struct {
	Value uint64
}

func (BitrateBitsPerSecond) bitrate() {}

type ResponseTimeout interface {
	responseTimeout()
}

type ResponseTimeoutLinkDefault struct{}

func (ResponseTimeoutLinkDefault) responseTimeout() {}

type ResponseTimeoutExact struct {
	Millis uint64
}

func (ResponseTimeoutExact) responseTimeout() {}

type ResourceCompression interface {
	resourceCompression()
}

type ResourceCompressionAuto struct{}

func (ResourceCompressionAuto) resourceCompression() {}

type ResourceCompressionNever struct{}

func (ResourceCompressionNever) resourceCompression() {}

type ResourceStrategy interface {
	resourceStrategy()
}

type ResourceStrategyRefuse struct{}

func (ResourceStrategyRefuse) resourceStrategy() {}

type ResourceStrategyAccept struct {
	MaximumUncompressedBytes uint64
	AcceptCompressed bool
}

func (ResourceStrategyAccept) resourceStrategy() {}

type DestinationConfig interface {
	destinationConfig()
}

type DestinationConfigPlain struct {
	Name DestinationName
}

func (DestinationConfigPlain) destinationConfig() {}

type DestinationConfigSingle struct {
	Name DestinationName
	Identity DestinationIdentityConfig
	AnnounceAppData *[]byte
	MaximumRequestBytes *uint64
	RequestHandlers []RequestHandlerConfig
}

func (DestinationConfigSingle) destinationConfig() {}

type HostCommand interface {
	hostCommand()
}

type HostCommandAnnounce struct {
	Destination DestinationHash
	Interface *InterfaceId
}

func (HostCommandAnnounce) hostCommand() {}

type HostCommandSendSinglePacket struct {
	Destination DestinationHash
	Payload []byte
}

func (HostCommandSendSinglePacket) hostCommand() {}

type HostCommandCloseLink struct {
	LinkId LinkId
}

func (HostCommandCloseLink) hostCommand() {}

type HostCommandAttachTcpServer struct {
	Bind string
	Bitrate Bitrate
}

func (HostCommandAttachTcpServer) hostCommand() {}

type HostCommandAttachTcpClient struct {
	Target string
	Bitrate Bitrate
}

func (HostCommandAttachTcpClient) hostCommand() {}

type HostCommandAttachUdp struct {
	Local string
	Peer string
	Bitrate Bitrate
}

func (HostCommandAttachUdp) hostCommand() {}

type HostCommandDetachInterface struct {
	Interface InterfaceId
}

func (HostCommandDetachInterface) hostCommand() {}

type HostCommandEstablishLink struct {
	Destination DestinationHash
}

func (HostCommandEstablishLink) hostCommand() {}

type HostCommandRequestPath struct {
	Destination DestinationHash
}

func (HostCommandRequestPath) hostCommand() {}

type HostCommandIdentify struct {
	LinkId LinkId
	Identity IdentityHash
}

func (HostCommandIdentify) hostCommand() {}

type HostCommandSendLinkPacket struct {
	LinkId LinkId
	Payload []byte
}

func (HostCommandSendLinkPacket) hostCommand() {}

type HostCommandRequest struct {
	LinkId LinkId
	PathHash RequestPathHash
	Payload []byte
	Timeout ResponseTimeout
	MaximumResponseBytes *uint64
}

func (HostCommandRequest) hostCommand() {}

type HostCommandRespond struct {
	LinkId LinkId
	RequestId RequestId
	RequestRttMillis uint64
	Payload []byte
}

func (HostCommandRespond) hostCommand() {}

type HostCommandSendResource struct {
	LinkId LinkId
	Payload []byte
	PackedMetadata *[]byte
	Compression ResourceCompression
}

func (HostCommandSendResource) hostCommand() {}

type HostCommandSetLinkResourceStrategy struct {
	LinkId LinkId
	Strategy ResourceStrategy
}

func (HostCommandSetLinkResourceStrategy) hostCommand() {}

type HostCommandSetDestinationResourceStrategy struct {
	Destination DestinationHash
	Strategy ResourceStrategy
}

func (HostCommandSetDestinationResourceStrategy) hostCommand() {}

type HostCommandSendChannelMessage struct {
	LinkId LinkId
	MessageType uint16
	Payload []byte
}

func (HostCommandSendChannelMessage) hostCommand() {}

type HostCommandAllowRequester struct {
	Destination DestinationHash
	PathHash RequestPathHash
	Identity IdentityHash
}

func (HostCommandAllowRequester) hostCommand() {}

type HostCommandAttachInterface struct {
	Config InterfaceConfig
	Routing *InterfaceRoutingPolicy
}

func (HostCommandAttachInterface) hostCommand() {}

type CommandOutcome interface {
	commandOutcome()
}

type CommandOutcomeAnnounced struct{}

func (CommandOutcomeAnnounced) commandOutcome() {}

type CommandOutcomePacketDelivered struct {
	RttMillis uint64
	Evidence DeliveryEvidenceKind
	PacketHash *PacketHash
}

func (CommandOutcomePacketDelivered) commandOutcome() {}

type CommandOutcomeLinkCloseQueued struct{}

func (CommandOutcomeLinkCloseQueued) commandOutcome() {}

type CommandOutcomeInterfaceAttached struct {
	Interface InterfaceId
}

func (CommandOutcomeInterfaceAttached) commandOutcome() {}

type CommandOutcomeInterfaceDetached struct {
	Interface InterfaceId
}

func (CommandOutcomeInterfaceDetached) commandOutcome() {}

type CommandOutcomeLinkEstablished struct {
	LinkId LinkId
	RttMillis uint64
}

func (CommandOutcomeLinkEstablished) commandOutcome() {}

type CommandOutcomePathDiscovered struct {
	Hops uint8
}

func (CommandOutcomePathDiscovered) commandOutcome() {}

type CommandOutcomeIdentified struct{}

func (CommandOutcomeIdentified) commandOutcome() {}

type CommandOutcomeResponseReceived struct {
	Data []byte
	RttMillis uint64
}

func (CommandOutcomeResponseReceived) commandOutcome() {}

type CommandOutcomeResponseSent struct {
	RttMillis uint64
}

func (CommandOutcomeResponseSent) commandOutcome() {}

type CommandOutcomeResourceSent struct{}

func (CommandOutcomeResourceSent) commandOutcome() {}

type CommandOutcomeResourceStrategySet struct{}

func (CommandOutcomeResourceStrategySet) commandOutcome() {}

type CommandOutcomeRequesterAllowed struct{}

func (CommandOutcomeRequesterAllowed) commandOutcome() {}

type CommandFailure interface {
	commandFailure()
}

type CommandFailureNodeStopped struct{}

func (CommandFailureNodeStopped) commandFailure() {}

type CommandFailureBusy struct{}

func (CommandFailureBusy) commandFailure() {}

type CommandFailurePayloadTooLarge struct{}

func (CommandFailurePayloadTooLarge) commandFailure() {}

type CommandFailureUnknownDestination struct{}

func (CommandFailureUnknownDestination) commandFailure() {}

type CommandFailureNotSingleDestination struct{}

func (CommandFailureNotSingleDestination) commandFailure() {}

type CommandFailureAnnounceAppDataTooLong struct{}

func (CommandFailureAnnounceAppDataTooLong) commandFailure() {}

type CommandFailureUnknownInterface struct{}

func (CommandFailureUnknownInterface) commandFailure() {}

type CommandFailureNoRouteToDestination struct{}

func (CommandFailureNoRouteToDestination) commandFailure() {}

type CommandFailureNotDirectlyReachable struct{}

func (CommandFailureNotDirectlyReachable) commandFailure() {}

type CommandFailurePacketCulled struct{}

func (CommandFailurePacketCulled) commandFailure() {}

type CommandFailureDeliveryTimedOut struct{}

func (CommandFailureDeliveryTimedOut) commandFailure() {}

type CommandFailureInvalidBitrate struct{}

func (CommandFailureInvalidBitrate) commandFailure() {}

type CommandFailureBindFailed struct {
	Detail string
}

func (CommandFailureBindFailed) commandFailure() {}

type CommandFailureWriteFailed struct {
	Detail string
}

func (CommandFailureWriteFailed) commandFailure() {}

type CommandFailureUnsupportedByBackend struct{}

func (CommandFailureUnsupportedByBackend) commandFailure() {}

type CommandFailureUnknownLink struct{}

func (CommandFailureUnknownLink) commandFailure() {}

type CommandFailureLinkNotActive struct{}

func (CommandFailureLinkNotActive) commandFailure() {}

type CommandFailureEntropyUnavailable struct{}

func (CommandFailureEntropyUnavailable) commandFailure() {}

type CommandFailureNotLinkInitiator struct{}

func (CommandFailureNotLinkInitiator) commandFailure() {}

type CommandFailureIdentityNotHeld struct{}

func (CommandFailureIdentityNotHeld) commandFailure() {}

type CommandFailureUnknownRequestHandler struct{}

func (CommandFailureUnknownRequestHandler) commandFailure() {}

type CommandFailureRequestPolicyNotAllowList struct{}

func (CommandFailureRequestPolicyNotAllowList) commandFailure() {}

type CommandFailureRequestAllowListFull struct{}

func (CommandFailureRequestAllowListFull) commandFailure() {}

type CommandFailureLinkBusy struct{}

func (CommandFailureLinkBusy) commandFailure() {}

type CommandFailureResourceTableFull struct{}

func (CommandFailureResourceTableFull) commandFailure() {}

type CommandFailureResourceMetadataTooLarge struct{}

func (CommandFailureResourceMetadataTooLarge) commandFailure() {}

type CommandFailureResourceRejectedByPeer struct{}

func (CommandFailureResourceRejectedByPeer) commandFailure() {}

type CommandFailureResourceSequencingFailed struct{}

func (CommandFailureResourceSequencingFailed) commandFailure() {}

type CommandFailureResourcePredecessorFailed struct{}

func (CommandFailureResourcePredecessorFailed) commandFailure() {}

type CommandFailureChannelWindowFull struct{}

func (CommandFailureChannelWindowFull) commandFailure() {}

type CommandFailureChannelUntrackable struct{}

func (CommandFailureChannelUntrackable) commandFailure() {}

type CommandFailureInvalidChannelMessageType struct{}

func (CommandFailureInvalidChannelMessageType) commandFailure() {}

type CommandFailureInvalidConfiguration struct {
	Detail string
}

func (CommandFailureInvalidConfiguration) commandFailure() {}

type CommandFailureResourceUploadCancelled struct{}

func (CommandFailureResourceUploadCancelled) commandFailure() {}

type CommandFailureResourceEarlyEof struct{}

func (CommandFailureResourceEarlyEof) commandFailure() {}

type CommandFailureResourceLengthOverrun struct{}

func (CommandFailureResourceLengthOverrun) commandFailure() {}

type CommandFailurePermissionDenied struct {
	Detail string
}

func (CommandFailurePermissionDenied) commandFailure() {}

type CommandFailureDeviceUnavailable struct {
	Detail string
}

func (CommandFailureDeviceUnavailable) commandFailure() {}

type CommandFailureConnectFailed struct {
	Detail string
}

func (CommandFailureConnectFailed) commandFailure() {}

type CommandFailureBackendFailed struct {
	Detail string
}

func (CommandFailureBackendFailed) commandFailure() {}

type CommandFailureResponseTooLarge struct{}

func (CommandFailureResponseTooLarge) commandFailure() {}

type ApplicationEvent interface {
	applicationEvent()
}

type ApplicationEventSingleDelivery struct {
	Destination DestinationHash
	SourceInterface InterfaceId
	Plaintext []byte
}

func (ApplicationEventSingleDelivery) applicationEvent() {}

type ApplicationEventRequest struct {
	Destination DestinationHash
	LinkId LinkId
	RequestId RequestId
	Requester *IdentityHash
	PathHash RequestPathHash
	RttMillis uint64
	Data []byte
}

func (ApplicationEventRequest) applicationEvent() {}

type ApplicationEventResponse struct {
	LinkId LinkId
	RequestId RequestId
	Data []byte
}

func (ApplicationEventResponse) applicationEvent() {}

type ApplicationEventResponseSegment struct {
	LinkId LinkId
	RequestId RequestId
	SegmentIndex uint64
	TotalSegments uint64
	Data []byte
}

func (ApplicationEventResponseSegment) applicationEvent() {}

type ApplicationEventResourceAvailable struct {
	LinkId LinkId
	Hash ResourceHash
	Metadata *[]byte
	Resource ResourceStream
}

func (ApplicationEventResourceAvailable) applicationEvent() {}

type ApplicationEventResourceSegment struct {
	LinkId LinkId
	OriginalHash ResourceHash
	SegmentIndex uint64
	TotalSegments uint64
	Metadata *[]byte
	Data []byte
}

func (ApplicationEventResourceSegment) applicationEvent() {}

type ApplicationEventResourceNeedsDecompression struct {
	LinkId LinkId
	Hash ResourceHash
	Stream []byte
	UncompressedDataBytes uint64
}

func (ApplicationEventResourceNeedsDecompression) applicationEvent() {}

type ApplicationEventChannelMessage struct {
	LinkId LinkId
	MessageType uint16
	Data []byte
}

func (ApplicationEventChannelMessage) applicationEvent() {}

type ApplicationEventLinkDelivery struct {
	LinkId LinkId
	SourceInterface InterfaceId
	Plaintext []byte
}

func (ApplicationEventLinkDelivery) applicationEvent() {}

type DiagnosticEvent interface {
	diagnosticEvent()
}

type DiagnosticEventAnnounceHeard struct {
	Destination DestinationHash
	Hops uint8
	SourceInterface InterfaceId
	AppData []byte
}

func (DiagnosticEventAnnounceHeard) diagnosticEvent() {}

type DiagnosticEventLinkEstablished struct {
	LinkId LinkId
	RttMillis uint64
}

func (DiagnosticEventLinkEstablished) diagnosticEvent() {}

type DiagnosticEventPeerIdentified struct {
	LinkId LinkId
	Identity IdentityHash
}

func (DiagnosticEventPeerIdentified) diagnosticEvent() {}

type DiagnosticEventLinkClosed struct {
	LinkId LinkId
	Reason LinkClosedReason
}

func (DiagnosticEventLinkClosed) diagnosticEvent() {}

type DiagnosticEventLinkInterfaceMismatch struct {
	LinkId LinkId
	AttachedInterface InterfaceId
	ArrivedOn InterfaceId
}

func (DiagnosticEventLinkInterfaceMismatch) diagnosticEvent() {}

type DiagnosticEventResourceAssembled struct {
	LinkId LinkId
	OriginalHash ResourceHash
	TotalSizeBytes uint64
}

func (DiagnosticEventResourceAssembled) diagnosticEvent() {}

type DiagnosticEventResourceFailed struct {
	LinkId LinkId
	Hash ResourceHash
	Cause string
}

func (DiagnosticEventResourceFailed) diagnosticEvent() {}

type DiagnosticEventResourceSendProgress struct {
	LinkId LinkId
	TransferredBytes uint64
	TotalBytes uint64
	PhysicalTransferredBytes uint64
	SegmentIndex uint64
	TotalSegments uint64
}

func (DiagnosticEventResourceSendProgress) diagnosticEvent() {}

type DiagnosticEventSelfRatchetRotated struct {
	Destination DestinationHash
}

func (DiagnosticEventSelfRatchetRotated) diagnosticEvent() {}

type DiagnosticEventAnnounceHeldDropped struct {
	Destination DestinationHash
	SourceInterface InterfaceId
	Cause string
}

func (DiagnosticEventAnnounceHeldDropped) diagnosticEvent() {}

type DiagnosticEventDelivered struct {
	Detail string
}

func (DiagnosticEventDelivered) diagnosticEvent() {}

type DiagnosticEventRouteExpired struct {
	Destination DestinationHash
}

func (DiagnosticEventRouteExpired) diagnosticEvent() {}

type DiagnosticEventRouteEvicted struct {
	Destination DestinationHash
}

func (DiagnosticEventRouteEvicted) diagnosticEvent() {}

type DiagnosticEventRouteInterfaceGone struct {
	Destination DestinationHash
}

func (DiagnosticEventRouteInterfaceGone) diagnosticEvent() {}

type DiagnosticEventRouteDropped struct {
	Destination DestinationHash
}

func (DiagnosticEventRouteDropped) diagnosticEvent() {}

type DiagnosticEventBackendDiagnostic struct {
	Kind string
	Detail string
}

func (DiagnosticEventBackendDiagnostic) diagnosticEvent() {}

type DiagnosticEventDiagnosticsDropped struct {
	Count UInt128
}

func (DiagnosticEventDiagnosticsDropped) diagnosticEvent() {}

type DiagnosticEventPersistenceRestored struct {
	Routes uint64
	DestinationIdentities uint64
	Tunnels uint64
	Ratchets uint64
	Refused uint64
	Dropped uint64
}

func (DiagnosticEventPersistenceRestored) diagnosticEvent() {}

type DiagnosticEventPersistenceFlushed struct {
	Cause PersistenceFlushCause
	Target PersistenceFlushTarget
}

func (DiagnosticEventPersistenceFlushed) diagnosticEvent() {}

type DiagnosticEventPersistenceFlushFailed struct {
	Cause PersistenceFlushCause
	Target PersistenceFlushTarget
}

func (DiagnosticEventPersistenceFlushFailed) diagnosticEvent() {}

var hostOperationNames = [...]string{
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
}

type rawUnit struct{}
type rawOwned[T any] struct{ value T }
type rawBorrowed[T any] struct{ value T }
type rawCallResult[T any] interface{ rawCallResult() }
type rawCallSuccess[T any] struct{ value T }
type rawCallFailure[T any] struct{ error Status }
func (rawCallSuccess[T]) rawCallResult() {}
func (rawCallFailure[T]) rawCallResult() {}
type rawCommandResult struct{}
type rawContractInfo struct{}
type rawEvent struct{}
type rawEventStream struct{}
type rawHost struct{}
type rawHostInspection struct{}
type rawHostOptions struct{}
type rawIssuedCommand struct{}
type rawLifecycle struct{}
type rawReadinessCallback struct{}
type rawReadinessRegistration struct{}
type rawResourceChunk struct{}
type rawResourceStream struct{}
type rawResourceUpload struct{}
type rawSuppliedPipe struct{}
type rawSuppliedPipeOpenRequest struct{}
type rawOpaquePointer struct{}

type rawHostProtocol interface {
	contractInfo() rawCallResult[rawContractInfo]
	backendInfo() rawCallResult[BackendInfo]
	hostCreate(options rawHostOptions) rawCallResult[rawOwned[rawHost]]
	hostRelease(host rawHost) rawUnit
	hostLifecycle(host rawHost) rawCallResult[rawLifecycle]
	hostSnapshot(host rawHost, timeoutMillis uint32) rawCallResult[rawOwned[rawHostInspection]]
	hostSnapshotRead(host_inspection rawHostInspection) rawCallResult[rawBorrowed[HostSnapshot]]
	hostSnapshotRelease(host_inspection rawHostInspection) rawUnit
	hostIdentityHash(host rawHost) rawCallResult[rawBorrowed[[]byte]]
	hostDestinationCount(host rawHost) uintptr
	hostDestinationHash(host rawHost, index uintptr) rawCallResult[rawBorrowed[[]byte]]
	hostAttachSuppliedPipe(host rawHost, name string, respawnDelayMillis uint64, bitrate Bitrate) rawCallResult[rawOwned[rawSuppliedPipe]]
	suppliedPipeClaimAttachment(supplied_pipe rawSuppliedPipe) rawCallResult[rawOwned[rawIssuedCommand]]
	suppliedPipeNextOpenRequest(supplied_pipe rawSuppliedPipe, timeoutMillis uint32) rawCallResult[rawOwned[rawSuppliedPipeOpenRequest]]
	suppliedPipeRegisterReadiness(supplied_pipe rawSuppliedPipe, callback rawReadinessCallback, context rawOpaquePointer) rawCallResult[rawOwned[rawReadinessRegistration]]
	suppliedPipeInterruptWait(supplied_pipe rawSuppliedPipe) rawUnit
	suppliedPipeRelease(supplied_pipe rawSuppliedPipe) rawUnit
	suppliedPipeOpenRequestProvide(supplied_pipe_open_request rawSuppliedPipeOpenRequest, descriptor int64) rawCallResult[bool]
	suppliedPipeOpenRequestDecline(supplied_pipe_open_request rawSuppliedPipeOpenRequest) rawCallResult[bool]
	suppliedPipeOpenRequestRelease(supplied_pipe_open_request rawSuppliedPipeOpenRequest) rawUnit
	hostBeginResourceUpload(host rawHost, linkId LinkId, declaredLength uint64, packedMetadata *[]byte, compression ResourceCompression) rawCallResult[rawOwned[rawResourceUpload]]
	resourceUploadWrite(resource_upload rawResourceUpload, chunk []byte) rawCallResult[rawUnit]
	resourceUploadIsWritable(resource_upload rawResourceUpload) rawCallResult[bool]
	resourceUploadFinish(resource_upload rawResourceUpload) rawCallResult[rawOwned[rawIssuedCommand]]
	resourceUploadAbort(resource_upload rawResourceUpload) rawUnit
	resourceUploadRelease(resource_upload rawResourceUpload) rawUnit
	hostStop(host rawHost) rawCallResult[rawUnit]
	commandWait(issued_command rawIssuedCommand, timeoutMillis uint32) rawCallResult[rawBorrowed[rawCommandResult]]
	commandRegisterReadiness(issued_command rawIssuedCommand, callback rawReadinessCallback, context rawOpaquePointer) rawCallResult[rawOwned[rawReadinessRegistration]]
	commandInterruptWait(issued_command rawIssuedCommand) rawUnit
	commandRelease(issued_command rawIssuedCommand) rawUnit
	hostClaimApplicationEvents(host rawHost) rawCallResult[rawOwned[rawEventStream]]
	hostClaimDiagnostics(host rawHost) rawCallResult[rawOwned[rawEventStream]]
	eventStreamRegisterReadiness(event_stream rawEventStream, callback rawReadinessCallback, context rawOpaquePointer) rawCallResult[rawOwned[rawReadinessRegistration]]
	readinessRegistrationRelease(readiness_registration rawReadinessRegistration) rawUnit
	eventStreamInterruptWait(event_stream rawEventStream) rawUnit
	eventStreamRelease(event_stream rawEventStream) rawUnit
	eventStreamNext(event_stream rawEventStream, timeoutMillis uint32) rawCallResult[rawOwned[rawEvent]]
	eventRelease(eventValue rawEvent) rawUnit
	eventKind(eventValue rawEvent) uint32
	eventBytes(eventValue rawEvent, field EventField) rawCallResult[rawBorrowed[[]byte]]
	eventString(eventValue rawEvent, field EventField) rawCallResult[rawBorrowed[string]]
	eventU64(eventValue rawEvent, field EventField) rawCallResult[uint64]
	eventU128(eventValue rawEvent, field EventField) rawCallResult[UInt128]
	eventResourceStream(eventValue rawEvent) rawCallResult[rawOwned[rawResourceStream]]
	resourceStreamRelease(resource_stream rawResourceStream) rawUnit
	resourceStreamNext(resource_stream rawResourceStream, maximumBytes uintptr) rawCallResult[rawBorrowed[rawResourceChunk]]
	hostAnnounce(host rawHost, destination DestinationHash, interfaceId *InterfaceId) rawCallResult[rawOwned[rawIssuedCommand]]
	hostSendSinglePacket(host rawHost, destination DestinationHash, payload []byte) rawCallResult[rawOwned[rawIssuedCommand]]
	hostCloseLink(host rawHost, linkId LinkId) rawCallResult[rawOwned[rawIssuedCommand]]
	hostAttachTcpServer(host rawHost, bind string, bitrate Bitrate) rawCallResult[rawOwned[rawIssuedCommand]]
	hostAttachTcpClient(host rawHost, target string, bitrate Bitrate) rawCallResult[rawOwned[rawIssuedCommand]]
	hostAttachUdp(host rawHost, local string, peer string, bitrate Bitrate) rawCallResult[rawOwned[rawIssuedCommand]]
	hostDetachInterface(host rawHost, interfaceId InterfaceId) rawCallResult[rawOwned[rawIssuedCommand]]
	hostEstablishLink(host rawHost, destination DestinationHash) rawCallResult[rawOwned[rawIssuedCommand]]
	hostRequestPath(host rawHost, destination DestinationHash) rawCallResult[rawOwned[rawIssuedCommand]]
	hostIdentify(host rawHost, linkId LinkId, identity IdentityHash) rawCallResult[rawOwned[rawIssuedCommand]]
	hostSendLinkPacket(host rawHost, linkId LinkId, payload []byte) rawCallResult[rawOwned[rawIssuedCommand]]
	hostRequest(host rawHost, linkId LinkId, pathHash RequestPathHash, payload []byte, timeout ResponseTimeout, maximumResponseBytes *uint64) rawCallResult[rawOwned[rawIssuedCommand]]
	hostRespond(host rawHost, linkId LinkId, requestId RequestId, requestRttMillis uint64, payload []byte) rawCallResult[rawOwned[rawIssuedCommand]]
	hostSendResource(host rawHost, linkId LinkId, payload []byte, packedMetadata *[]byte, compression ResourceCompression) rawCallResult[rawOwned[rawIssuedCommand]]
	hostSetLinkResourceStrategy(host rawHost, linkId LinkId, strategy ResourceStrategy) rawCallResult[rawOwned[rawIssuedCommand]]
	hostSetDestinationResourceStrategy(host rawHost, destination DestinationHash, strategy ResourceStrategy) rawCallResult[rawOwned[rawIssuedCommand]]
	hostSendChannelMessage(host rawHost, linkId LinkId, messageType uint16, payload []byte) rawCallResult[rawOwned[rawIssuedCommand]]
	hostAllowRequester(host rawHost, destination DestinationHash, pathHash RequestPathHash, identity IdentityHash) rawCallResult[rawOwned[rawIssuedCommand]]
	hostAttachInterface(host rawHost, config InterfaceConfig, routing *InterfaceRoutingPolicy) rawCallResult[rawOwned[rawIssuedCommand]]
}
