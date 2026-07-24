package prns

const (
	HostContractABI uint32 = 1
	HostSchemaVersion uint32 = 1
	ProductVersion = "0.2.8"
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

type CommandOutcomeKind uint32

const (
	CommandOutcomeKindAnnounced CommandOutcomeKind = 1
	CommandOutcomeKindPacketDelivered CommandOutcomeKind = 2
	CommandOutcomeKindLinkCloseQueued CommandOutcomeKind = 3
	CommandOutcomeKindInterfaceAttached CommandOutcomeKind = 4
	CommandOutcomeKindInterfaceDetached CommandOutcomeKind = 5
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
	MessageType string
	Data []byte
}

func (ApplicationEventChannelMessage) applicationEvent() {}

type DiagnosticEvent interface {
	diagnosticEvent()
}

type DiagnosticEventAnnounceHeard struct {
	Destination DestinationHash
	Hops uint8
	SourceInterface InterfaceId
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
