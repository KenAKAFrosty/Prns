package prns

import (
	"context"
	"io"
	"sync"
	"unsafe"
)

type StreamClaim[T any] interface {
	streamClaim()
}

type StreamClaimed[T any] struct {
	Stream T
}

func (StreamClaimed[T]) streamClaim() {}

type StreamAlreadyClaimed[T any] struct{}

func (StreamAlreadyClaimed[T]) streamClaim() {}

type eventWaitResult struct {
	event  nativeEvent
	status Status
}

type ownedEventStream struct {
	stateMutex sync.Mutex
	waitMutex  sync.Mutex
	native     nativeEventStream
}

func newOwnedEventStream(native nativeEventStream) *ownedEventStream {
	return &ownedEventStream{native: native}
}

func (stream *ownedEventStream) next(ctx context.Context) (nativeEvent, error) {
	stream.waitMutex.Lock()
	defer stream.waitMutex.Unlock()
	stream.stateMutex.Lock()
	native := stream.native
	stream.stateMutex.Unlock()
	if native.pointer == nil {
		return nativeEvent{}, io.EOF
	}
	completed := make(chan eventWaitResult, 1)
	go func() {
		event, status := ffiEventNext(native)
		completed <- eventWaitResult{event: event, status: status}
	}()
	var waited eventWaitResult
	select {
	case waited = <-completed:
	case <-ctx.Done():
		ffiEventStreamInterrupt(native)
		waited = <-completed
		if waited.status == StatusInterrupted {
			return nativeEvent{}, ctx.Err()
		}
	}
	switch waited.status {
	case StatusOk:
		return waited.event, nil
	case StatusStopped:
		return nativeEvent{}, io.EOF
	default:
		return nativeEvent{}, StatusError{
			Operation: "read event stream",
			Status:    waited.status,
		}
	}
}

func (stream *ownedEventStream) close() error {
	stream.stateMutex.Lock()
	native := stream.native
	stream.native = nativeEventStream{}
	if native.pointer != nil {
		ffiEventStreamInterrupt(native)
	}
	stream.stateMutex.Unlock()
	stream.waitMutex.Lock()
	defer stream.waitMutex.Unlock()
	if native.pointer != nil {
		ffiEventStreamClose(native)
	}
	return nil
}

type ApplicationEventStream struct {
	owned *ownedEventStream
}

func newApplicationEventStream(native nativeEventStream) *ApplicationEventStream {
	return &ApplicationEventStream{owned: newOwnedEventStream(native)}
}

func (stream *ApplicationEventStream) Next(
	ctx context.Context,
) (ApplicationEvent, error) {
	event, err := stream.owned.next(ctx)
	if err != nil {
		return nil, err
	}
	defer ffiEventClose(event)
	return decodeApplicationEvent(event)
}

func (stream *ApplicationEventStream) Close() error {
	return stream.owned.close()
}

type DiagnosticEventStream struct {
	owned *ownedEventStream
}

func newDiagnosticEventStream(native nativeEventStream) *DiagnosticEventStream {
	return &DiagnosticEventStream{owned: newOwnedEventStream(native)}
}

func (stream *DiagnosticEventStream) Next(
	ctx context.Context,
) (DiagnosticEvent, error) {
	event, err := stream.owned.next(ctx)
	if err != nil {
		return nil, err
	}
	defer ffiEventClose(event)
	return decodeDiagnosticEvent(event)
}

func (stream *DiagnosticEventStream) Close() error {
	return stream.owned.close()
}

func requiredBytes(event nativeEvent, field EventField) ([]byte, error) {
	value, status := ffiEventBytes(event, field)
	if status != StatusOk {
		return nil, StatusError{Operation: "decode event bytes", Status: status}
	}
	return value, nil
}

func optionalBytes(event nativeEvent, field EventField) ([]byte, error) {
	value, status := ffiEventBytes(event, field)
	switch status {
	case StatusOk:
		return value, nil
	case StatusInvalidArgument:
		return nil, nil
	default:
		return nil, StatusError{Operation: "decode event bytes", Status: status}
	}
}

func requiredString(event nativeEvent, field EventField) (string, error) {
	value, status := ffiEventString(event, field)
	if status != StatusOk {
		return "", StatusError{Operation: "decode event string", Status: status}
	}
	return value, nil
}

func requiredU64(event nativeEvent, field EventField) (uint64, error) {
	value, status := ffiEventU64(event, field)
	if status != StatusOk {
		return 0, StatusError{Operation: "decode event integer", Status: status}
	}
	return value, nil
}

type fixedBytes interface {
	DestinationHash |
		IdentityHash |
		InterfaceId |
		LinkId |
		PacketHash |
		RequestId |
		RequestPathHash |
		ResourceHash
}

func fixedValue[T fixedBytes](
	event nativeEvent,
	field EventField,
) (T, error) {
	value, err := requiredBytes(event, field)
	var result T
	if err != nil {
		return result, err
	}
	target := unsafe.Slice(
		(*byte)(unsafe.Pointer(&result)),
		int(unsafe.Sizeof(result)),
	)
	if len(value) != len(target) {
		return result, StatusError{
			Operation: "decode fixed event value",
			Status:    StatusBackendFailed,
		}
	}
	copy(target, value)
	return result, nil
}

func optionalIdentityHash(
	event nativeEvent,
	field EventField,
) (*IdentityHash, error) {
	value, err := optionalBytes(event, field)
	if err != nil || value == nil {
		return nil, err
	}
	if len(value) != IdentityHashLength {
		return nil, StatusError{
			Operation: "decode optional identity hash",
			Status:    StatusBackendFailed,
		}
	}
	result := IdentityHash(value)
	return &result, nil
}

func optionalByteSlice(
	event nativeEvent,
	field EventField,
) (*[]byte, error) {
	value, err := optionalBytes(event, field)
	if err != nil || value == nil {
		return nil, err
	}
	return &value, nil
}

func decodeApplicationEvent(event nativeEvent) (ApplicationEvent, error) {
	switch ApplicationEventKind(ffiEventKind(event)) {
	case ApplicationEventKindSingleDelivery:
		destination, err := fixedValue[DestinationHash](
			event,
			EventFieldDestination,
		)
		if err != nil {
			return nil, err
		}
		source, err := fixedValue[InterfaceId](
			event,
			EventFieldSourceInterface,
		)
		if err != nil {
			return nil, err
		}
		plaintext, err := requiredBytes(event, EventFieldPlaintext)
		if err != nil {
			return nil, err
		}
		return ApplicationEventSingleDelivery{
			Destination:     destination,
			SourceInterface: source,
			Plaintext:       plaintext,
		}, nil
	case ApplicationEventKindLinkDelivery:
		linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
		if err != nil {
			return nil, err
		}
		source, err := fixedValue[InterfaceId](event, EventFieldSourceInterface)
		if err != nil {
			return nil, err
		}
		plaintext, err := requiredBytes(event, EventFieldPlaintext)
		if err != nil {
			return nil, err
		}
		return ApplicationEventLinkDelivery{
			LinkId:          linkID,
			SourceInterface: source,
			Plaintext:       plaintext,
		}, nil
	case ApplicationEventKindRequest:
		return decodeRequest(event)
	case ApplicationEventKindResponse:
		linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
		if err != nil {
			return nil, err
		}
		requestID, err := fixedValue[RequestId](
			event,
			EventFieldRequestId,
		)
		if err != nil {
			return nil, err
		}
		data, err := requiredBytes(event, EventFieldData)
		if err != nil {
			return nil, err
		}
		return ApplicationEventResponse{
			LinkId: linkID, RequestId: requestID, Data: data,
		}, nil
	case ApplicationEventKindResponseSegment:
		return decodeResponseSegment(event)
	case ApplicationEventKindResourceAvailable:
		return decodeResourceAvailable(event)
	case ApplicationEventKindResourceSegment:
		return decodeResourceSegment(event)
	case ApplicationEventKindResourceNeedsDecompression:
		return decodeResourceNeedsDecompression(event)
	case ApplicationEventKindChannelMessage:
		linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
		if err != nil {
			return nil, err
		}
		messageType, err := requiredU64(event, EventFieldMessageType)
		if err != nil {
			return nil, err
		}
		if messageType > uint64(^uint16(0)) {
			return nil, StatusError{
				Operation: "decode channel message type",
				Status:    StatusBackendFailed,
			}
		}
		data, err := requiredBytes(event, EventFieldData)
		if err != nil {
			return nil, err
		}
		return ApplicationEventChannelMessage{
			LinkId: linkID, MessageType: uint16(messageType), Data: data,
		}, nil
	default:
		return nil, StatusError{
			Operation: "decode application event kind",
			Status:    StatusBackendFailed,
		}
	}
}

func decodeRequest(event nativeEvent) (ApplicationEvent, error) {
	destination, err := fixedValue[DestinationHash](
		event,
		EventFieldDestination,
	)
	if err != nil {
		return nil, err
	}
	linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
	if err != nil {
		return nil, err
	}
	requestID, err := fixedValue[RequestId](
		event,
		EventFieldRequestId,
	)
	if err != nil {
		return nil, err
	}
	requester, err := optionalIdentityHash(event, EventFieldRequester)
	if err != nil {
		return nil, err
	}
	pathHash, err := fixedValue[RequestPathHash](
		event,
		EventFieldPathHash,
	)
	if err != nil {
		return nil, err
	}
	rtt, err := requiredU64(event, EventFieldRttMillis)
	if err != nil {
		return nil, err
	}
	data, err := requiredBytes(event, EventFieldData)
	if err != nil {
		return nil, err
	}
	return ApplicationEventRequest{
		Destination: destination,
		LinkId:      linkID,
		RequestId:   requestID,
		Requester:   requester,
		PathHash:    pathHash,
		RttMillis:   rtt,
		Data:        data,
	}, nil
}

func decodeResponseSegment(event nativeEvent) (ApplicationEvent, error) {
	linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
	if err != nil {
		return nil, err
	}
	requestID, err := fixedValue[RequestId](
		event,
		EventFieldRequestId,
	)
	if err != nil {
		return nil, err
	}
	index, err := requiredU64(event, EventFieldSegmentIndex)
	if err != nil {
		return nil, err
	}
	total, err := requiredU64(event, EventFieldTotalSegments)
	if err != nil {
		return nil, err
	}
	data, err := requiredBytes(event, EventFieldData)
	if err != nil {
		return nil, err
	}
	return ApplicationEventResponseSegment{
		LinkId:        linkID,
		RequestId:     requestID,
		SegmentIndex:  index,
		TotalSegments: total,
		Data:          data,
	}, nil
}

func decodeResourceAvailable(event nativeEvent) (ApplicationEvent, error) {
	linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
	if err != nil {
		return nil, err
	}
	hash, err := fixedValue[ResourceHash](event, EventFieldHash)
	if err != nil {
		return nil, err
	}
	metadata, err := optionalByteSlice(event, EventFieldMetadata)
	if err != nil {
		return nil, err
	}
	totalBytes, err := requiredU64(event, EventFieldTotalBytes)
	if err != nil {
		return nil, err
	}
	native, status := ffiEventResource(event)
	if status != StatusOk {
		return nil, StatusError{
			Operation: "claim resource stream",
			Status:    status,
		}
	}
	return ApplicationEventResourceAvailable{
		LinkId:   linkID,
		Hash:     hash,
		Metadata: metadata,
		Resource: &resourceStream{native: native, totalBytes: totalBytes},
	}, nil
}

func decodeResourceSegment(event nativeEvent) (ApplicationEvent, error) {
	linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
	if err != nil {
		return nil, err
	}
	hash, err := fixedValue[ResourceHash](
		event,
		EventFieldOriginalHash,
	)
	if err != nil {
		return nil, err
	}
	index, err := requiredU64(event, EventFieldSegmentIndex)
	if err != nil {
		return nil, err
	}
	total, err := requiredU64(event, EventFieldTotalSegments)
	if err != nil {
		return nil, err
	}
	metadata, err := optionalByteSlice(event, EventFieldMetadata)
	if err != nil {
		return nil, err
	}
	data, err := requiredBytes(event, EventFieldData)
	if err != nil {
		return nil, err
	}
	return ApplicationEventResourceSegment{
		LinkId:        linkID,
		OriginalHash:  hash,
		SegmentIndex:  index,
		TotalSegments: total,
		Metadata:      metadata,
		Data:          data,
	}, nil
}

func decodeResourceNeedsDecompression(
	event nativeEvent,
) (ApplicationEvent, error) {
	linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
	if err != nil {
		return nil, err
	}
	hash, err := fixedValue[ResourceHash](event, EventFieldHash)
	if err != nil {
		return nil, err
	}
	value, err := requiredBytes(event, EventFieldStream)
	if err != nil {
		return nil, err
	}
	uncompressed, err := requiredU64(event, EventFieldUncompressedDataBytes)
	if err != nil {
		return nil, err
	}
	return ApplicationEventResourceNeedsDecompression{
		LinkId:                linkID,
		Hash:                  hash,
		Stream:                value,
		UncompressedDataBytes: uncompressed,
	}, nil
}

func decodeDiagnosticEvent(event nativeEvent) (DiagnosticEvent, error) {
	switch DiagnosticEventKind(ffiEventKind(event)) {
	case DiagnosticEventKindAnnounceHeard:
		return decodeAnnounceHeard(event)
	case DiagnosticEventKindLinkEstablished:
		linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
		if err != nil {
			return nil, err
		}
		rtt, err := requiredU64(event, EventFieldRttMillis)
		if err != nil {
			return nil, err
		}
		return DiagnosticEventLinkEstablished{LinkId: linkID, RttMillis: rtt}, nil
	case DiagnosticEventKindPeerIdentified:
		linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
		if err != nil {
			return nil, err
		}
		identity, err := fixedValue[IdentityHash](
			event,
			EventFieldIdentity,
		)
		if err != nil {
			return nil, err
		}
		return DiagnosticEventPeerIdentified{LinkId: linkID, Identity: identity}, nil
	case DiagnosticEventKindLinkClosed:
		return decodeLinkClosed(event)
	case DiagnosticEventKindLinkInterfaceMismatch:
		return decodeLinkInterfaceMismatch(event)
	case DiagnosticEventKindResourceAssembled:
		return decodeResourceAssembled(event)
	case DiagnosticEventKindResourceFailed:
		return decodeResourceFailed(event)
	case DiagnosticEventKindResourceSendProgress:
		return decodeResourceSendProgress(event)
	case DiagnosticEventKindSelfRatchetRotated:
		destination, err := fixedValue[DestinationHash](
			event,
			EventFieldDestination,
		)
		return DiagnosticEventSelfRatchetRotated{Destination: destination}, err
	case DiagnosticEventKindAnnounceHeldDropped:
		return decodeAnnounceHeldDropped(event)
	case DiagnosticEventKindDelivered:
		detail, err := requiredString(event, EventFieldDetail)
		return DiagnosticEventDelivered{Detail: detail}, err
	case DiagnosticEventKindRouteExpired,
		DiagnosticEventKindRouteEvicted,
		DiagnosticEventKindRouteInterfaceGone,
		DiagnosticEventKindRouteDropped:
		return decodeRouteDiagnostic(event)
	case DiagnosticEventKindBackendDiagnostic:
		kind, err := requiredString(event, EventFieldKind)
		if err != nil {
			return nil, err
		}
		detail, err := requiredString(event, EventFieldDetail)
		return DiagnosticEventBackendDiagnostic{Kind: kind, Detail: detail}, err
	case DiagnosticEventKindDiagnosticsDropped:
		count, status := ffiEventU128(event, EventFieldDroppedCount)
		if status != StatusOk {
			return nil, StatusError{
				Operation: "decode diagnostic gap",
				Status:    status,
			}
		}
		return DiagnosticEventDiagnosticsDropped{Count: count}, nil
	case DiagnosticEventKindPersistenceRestored:
		return decodePersistenceRestored(event)
	case DiagnosticEventKindPersistenceFlushed,
		DiagnosticEventKindPersistenceFlushFailed:
		return decodePersistenceFlush(event)
	default:
		return nil, StatusError{
			Operation: "decode diagnostic event kind",
			Status:    StatusBackendFailed,
		}
	}
}

func decodePersistenceRestored(event nativeEvent) (DiagnosticEvent, error) {
	fields := [...]EventField{
		EventFieldRoutes,
		EventFieldDestinationIdentities,
		EventFieldTunnels,
		EventFieldRatchets,
		EventFieldRefused,
		EventFieldDropped,
	}
	values := [len(fields)]uint64{}
	for index, field := range fields {
		value, err := requiredU64(event, field)
		if err != nil {
			return nil, err
		}
		values[index] = value
	}
	return DiagnosticEventPersistenceRestored{
		Routes:                values[0],
		DestinationIdentities: values[1],
		Tunnels:               values[2],
		Ratchets:              values[3],
		Refused:               values[4],
		Dropped:               values[5],
	}, nil
}

func decodePersistenceFlush(event nativeEvent) (DiagnosticEvent, error) {
	rawCause, err := requiredU64(event, EventFieldPersistenceCause)
	if err != nil {
		return nil, err
	}
	rawTarget, err := requiredU64(event, EventFieldPersistenceTarget)
	if err != nil {
		return nil, err
	}
	cause := PersistenceFlushCause(rawCause)
	target := PersistenceFlushTarget(rawTarget)
	if cause < PersistenceFlushCauseStartup || cause > PersistenceFlushCauseShutdown ||
		target < PersistenceFlushTargetRoutingState || target > PersistenceFlushTargetRatchets {
		return nil, StatusError{
			Operation: "decode persistence diagnostic",
			Status:    StatusBackendFailed,
		}
	}
	if DiagnosticEventKind(ffiEventKind(event)) == DiagnosticEventKindPersistenceFlushed {
		return DiagnosticEventPersistenceFlushed{Cause: cause, Target: target}, nil
	}
	return DiagnosticEventPersistenceFlushFailed{Cause: cause, Target: target}, nil
}

func decodeAnnounceHeard(event nativeEvent) (DiagnosticEvent, error) {
	destination, err := fixedValue[DestinationHash](
		event,
		EventFieldDestination,
	)
	if err != nil {
		return nil, err
	}
	hops, err := requiredU64(event, EventFieldHops)
	if err != nil {
		return nil, err
	}
	if hops > 255 {
		return nil, StatusError{
			Operation: "decode announce hops",
			Status:    StatusBackendFailed,
		}
	}
	source, err := fixedValue[InterfaceId](
		event,
		EventFieldSourceInterface,
	)
	if err != nil {
		return nil, err
	}
	appData, err := requiredBytes(event, EventFieldAppData)
	if err != nil {
		return nil, err
	}
	return DiagnosticEventAnnounceHeard{
		Destination:     destination,
		Hops:            uint8(hops),
		SourceInterface: source,
		AppData:         appData,
	}, nil
}

func decodeLinkClosed(event nativeEvent) (DiagnosticEvent, error) {
	linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
	if err != nil {
		return nil, err
	}
	reason, err := requiredU64(event, EventFieldReason)
	if err != nil {
		return nil, err
	}
	if reason != uint64(LinkClosedReasonTimeout) &&
		reason != uint64(LinkClosedReasonPeerClosed) &&
		reason != uint64(LinkClosedReasonMalformedRtt) {
		return nil, StatusError{
			Operation: "decode link closed reason",
			Status:    StatusBackendFailed,
		}
	}
	return DiagnosticEventLinkClosed{
		LinkId: linkID,
		Reason: LinkClosedReason(reason),
	}, nil
}

func decodeLinkInterfaceMismatch(event nativeEvent) (DiagnosticEvent, error) {
	linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
	if err != nil {
		return nil, err
	}
	attached, err := fixedValue[InterfaceId](
		event,
		EventFieldAttachedInterface,
	)
	if err != nil {
		return nil, err
	}
	arrived, err := fixedValue[InterfaceId](
		event,
		EventFieldArrivedOn,
	)
	if err != nil {
		return nil, err
	}
	return DiagnosticEventLinkInterfaceMismatch{
		LinkId:            linkID,
		AttachedInterface: attached,
		ArrivedOn:         arrived,
	}, nil
}

func decodeResourceAssembled(event nativeEvent) (DiagnosticEvent, error) {
	linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
	if err != nil {
		return nil, err
	}
	hash, err := fixedValue[ResourceHash](
		event,
		EventFieldOriginalHash,
	)
	if err != nil {
		return nil, err
	}
	total, err := requiredU64(event, EventFieldTotalSizeBytes)
	if err != nil {
		return nil, err
	}
	return DiagnosticEventResourceAssembled{
		LinkId: linkID, OriginalHash: hash, TotalSizeBytes: total,
	}, nil
}

func decodeResourceFailed(event nativeEvent) (DiagnosticEvent, error) {
	linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
	if err != nil {
		return nil, err
	}
	hash, err := fixedValue[ResourceHash](event, EventFieldHash)
	if err != nil {
		return nil, err
	}
	cause, err := requiredString(event, EventFieldCause)
	if err != nil {
		return nil, err
	}
	return DiagnosticEventResourceFailed{LinkId: linkID, Hash: hash, Cause: cause}, nil
}

func decodeResourceSendProgress(event nativeEvent) (DiagnosticEvent, error) {
	linkID, err := fixedValue[LinkId](event, EventFieldLinkId)
	if err != nil {
		return nil, err
	}
	transferred, err := requiredU64(event, EventFieldTransferredBytes)
	if err != nil {
		return nil, err
	}
	total, err := requiredU64(event, EventFieldTotalBytes)
	if err != nil {
		return nil, err
	}
	physical, err := requiredU64(event, EventFieldPhysicalTransferredBytes)
	if err != nil {
		return nil, err
	}
	index, err := requiredU64(event, EventFieldSegmentIndex)
	if err != nil {
		return nil, err
	}
	segments, err := requiredU64(event, EventFieldTotalSegments)
	if err != nil {
		return nil, err
	}
	return DiagnosticEventResourceSendProgress{
		LinkId:                   linkID,
		TransferredBytes:         transferred,
		TotalBytes:               total,
		PhysicalTransferredBytes: physical,
		SegmentIndex:             index,
		TotalSegments:            segments,
	}, nil
}

func decodeAnnounceHeldDropped(event nativeEvent) (DiagnosticEvent, error) {
	destination, err := fixedValue[DestinationHash](
		event,
		EventFieldDestination,
	)
	if err != nil {
		return nil, err
	}
	source, err := fixedValue[InterfaceId](
		event,
		EventFieldSourceInterface,
	)
	if err != nil {
		return nil, err
	}
	cause, err := requiredString(event, EventFieldCause)
	if err != nil {
		return nil, err
	}
	return DiagnosticEventAnnounceHeldDropped{
		Destination:     destination,
		SourceInterface: source,
		Cause:           cause,
	}, nil
}

func decodeRouteDiagnostic(event nativeEvent) (DiagnosticEvent, error) {
	destination, err := fixedValue[DestinationHash](
		event,
		EventFieldDestination,
	)
	if err != nil {
		return nil, err
	}
	switch DiagnosticEventKind(ffiEventKind(event)) {
	case DiagnosticEventKindRouteExpired:
		return DiagnosticEventRouteExpired{Destination: destination}, nil
	case DiagnosticEventKindRouteEvicted:
		return DiagnosticEventRouteEvicted{Destination: destination}, nil
	case DiagnosticEventKindRouteInterfaceGone:
		return DiagnosticEventRouteInterfaceGone{Destination: destination}, nil
	case DiagnosticEventKindRouteDropped:
		return DiagnosticEventRouteDropped{Destination: destination}, nil
	default:
		return nil, StatusError{
			Operation: "decode route diagnostic",
			Status:    StatusBackendFailed,
		}
	}
}

type resourceStream struct {
	mutex      sync.Mutex
	native     nativeResourceStream
	totalBytes uint64
}

func (stream *resourceStream) TotalBytes() uint64 {
	return stream.totalBytes
}

func (stream *resourceStream) Next(
	maximumBytes int,
) ([]byte, bool, error) {
	stream.mutex.Lock()
	defer stream.mutex.Unlock()
	if stream.native.pointer == nil {
		return nil, true, io.EOF
	}
	value, finished, status := ffiResourceNext(stream.native, maximumBytes)
	if status != StatusOk {
		return nil, false, StatusError{
			Operation: "read resource stream",
			Status:    status,
		}
	}
	return value, finished, nil
}

func (stream *resourceStream) Close() error {
	stream.mutex.Lock()
	defer stream.mutex.Unlock()
	if stream.native.pointer == nil {
		return nil
	}
	ffiResourceClose(stream.native)
	stream.native = nativeResourceStream{}
	return nil
}
