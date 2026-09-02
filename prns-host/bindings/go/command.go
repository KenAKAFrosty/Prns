package prns

import (
	"context"
	"sync"
	"time"
)

type ResourceUpload struct {
	mutex    sync.Mutex
	native   nativeResourceUpload
	finished bool
}

func (upload *ResourceUpload) Write(ctx context.Context, chunk []byte) error {
	for {
		upload.mutex.Lock()
		if upload.native.pointer == nil || upload.finished {
			upload.mutex.Unlock()
			return StatusError{Operation: "write resource upload", Status: StatusStopped}
		}
		status := ffiResourceUploadWrite(upload.native, chunk)
		upload.mutex.Unlock()
		switch status {
		case StatusOk:
			return nil
		case StatusWouldBlock:
			select {
			case <-ctx.Done():
				upload.Abort()
				return ctx.Err()
			case <-time.After(time.Millisecond):
			}
		default:
			return StatusError{Operation: "write resource upload", Status: status}
		}
	}
}

func (upload *ResourceUpload) Finish() (*Command, error) {
	upload.mutex.Lock()
	defer upload.mutex.Unlock()
	if upload.native.pointer == nil || upload.finished {
		return nil, StatusError{Operation: "finish resource upload", Status: StatusStopped}
	}
	command, status := ffiResourceUploadFinish(upload.native)
	if status != StatusOk {
		return nil, StatusError{Operation: "finish resource upload", Status: status}
	}
	upload.finished = true
	return &Command{native: command}, nil
}

func (upload *ResourceUpload) Abort() {
	upload.mutex.Lock()
	defer upload.mutex.Unlock()
	if upload.native.pointer != nil && !upload.finished {
		ffiResourceUploadAbort(upload.native)
		upload.finished = true
	}
}

func (upload *ResourceUpload) Close() error {
	upload.mutex.Lock()
	defer upload.mutex.Unlock()
	if upload.native.pointer == nil {
		return nil
	}
	if !upload.finished {
		ffiResourceUploadAbort(upload.native)
	}
	ffiResourceUploadClose(upload.native)
	upload.native = nativeResourceUpload{}
	return nil
}

type CommandSettlement interface {
	commandSettlement()
}

type CommandSucceeded struct {
	Outcome CommandOutcome
}

func (CommandSucceeded) commandSettlement() {}

type CommandFailed struct {
	Failure CommandFailure
}

func (CommandFailed) commandSettlement() {}

type commandWaitResult struct {
	result nativeCommandResult
	status Status
}

type Command struct {
	stateMutex sync.Mutex
	waitMutex  sync.Mutex
	native     nativeCommand
}

func (command *Command) Wait(ctx context.Context) (CommandSettlement, error) {
	command.waitMutex.Lock()
	defer command.waitMutex.Unlock()
	command.stateMutex.Lock()
	native := command.native
	command.stateMutex.Unlock()
	if native.pointer == nil {
		return nil, StatusError{Operation: "wait command", Status: StatusStopped}
	}
	completed := make(chan commandWaitResult, 1)
	go func() {
		result, status := ffiCommandWait(native)
		completed <- commandWaitResult{result: result, status: status}
	}()
	var waited commandWaitResult
	select {
	case waited = <-completed:
	case <-ctx.Done():
		ffiCommandInterrupt(native)
		waited = <-completed
		if waited.status == StatusInterrupted {
			return nil, ctx.Err()
		}
	}
	if waited.status != StatusOk {
		return nil, StatusError{Operation: "wait command", Status: waited.status}
	}
	return decodeCommandSettlement(waited.result)
}

func decodeCommandSettlement(
	result nativeCommandResult,
) (CommandSettlement, error) {
	if result.failure != 0 {
		failure, err := decodeCommandFailure(result.failure, result.detail)
		if err != nil {
			return nil, err
		}
		return CommandFailed{Failure: failure}, nil
	}
	var outcome CommandOutcome
	switch result.outcome {
	case CommandOutcomeKindAnnounced:
		outcome = CommandOutcomeAnnounced{}
	case CommandOutcomeKindPacketDelivered:
		var packetHash *PacketHash
		switch result.evidence {
		case DeliveryEvidenceKindResponse:
			if len(result.value) != 0 {
				return nil, StatusError{
					Operation: "decode response evidence",
					Status:    StatusBackendFailed,
				}
			}
		case DeliveryEvidenceKindExplicitProof,
			DeliveryEvidenceKindImplicitProof:
			if len(result.value) != PacketHashLength {
				return nil, StatusError{
					Operation: "decode proof evidence",
					Status:    StatusBackendFailed,
				}
			}
			value := PacketHash(result.value)
			packetHash = &value
		default:
			return nil, StatusError{
				Operation: "decode delivery evidence",
				Status:    StatusBackendFailed,
			}
		}
		outcome = CommandOutcomePacketDelivered{
			RttMillis:  result.rttMillis,
			Evidence:   result.evidence,
			PacketHash: packetHash,
		}
	case CommandOutcomeKindLinkCloseQueued:
		outcome = CommandOutcomeLinkCloseQueued{}
	case CommandOutcomeKindInterfaceAttached:
		if len(result.value) != InterfaceIdLength {
			return nil, StatusError{
				Operation: "decode command outcome",
				Status:    StatusBackendFailed,
			}
		}
		outcome = CommandOutcomeInterfaceAttached{
			Interface: InterfaceId(result.value),
		}
	case CommandOutcomeKindInterfaceDetached:
		if len(result.value) != InterfaceIdLength {
			return nil, StatusError{
				Operation: "decode command outcome",
				Status:    StatusBackendFailed,
			}
		}
		outcome = CommandOutcomeInterfaceDetached{
			Interface: InterfaceId(result.value),
		}
	case CommandOutcomeKindLinkEstablished:
		if len(result.value) != LinkIdLength {
			return nil, StatusError{
				Operation: "decode link establishment",
				Status:    StatusBackendFailed,
			}
		}
		outcome = CommandOutcomeLinkEstablished{
			LinkId:    LinkId(result.value),
			RttMillis: result.rttMillis,
		}
	case CommandOutcomeKindPathDiscovered:
		if len(result.value) != 1 {
			return nil, StatusError{
				Operation: "decode path discovery",
				Status:    StatusBackendFailed,
			}
		}
		outcome = CommandOutcomePathDiscovered{Hops: result.value[0]}
	case CommandOutcomeKindIdentified:
		outcome = CommandOutcomeIdentified{}
	case CommandOutcomeKindResponseReceived:
		outcome = CommandOutcomeResponseReceived{
			Data:      result.value,
			RttMillis: result.rttMillis,
		}
	case CommandOutcomeKindResponseSent:
		outcome = CommandOutcomeResponseSent{RttMillis: result.rttMillis}
	case CommandOutcomeKindResourceSent:
		outcome = CommandOutcomeResourceSent{}
	case CommandOutcomeKindResourceStrategySet:
		outcome = CommandOutcomeResourceStrategySet{}
	case CommandOutcomeKindRequesterAllowed:
		outcome = CommandOutcomeRequesterAllowed{}
	default:
		return nil, StatusError{
			Operation: "decode command outcome",
			Status:    StatusBackendFailed,
		}
	}
	return CommandSucceeded{Outcome: outcome}, nil
}

func decodeCommandFailure(kind CommandFailureKind, detail string) (CommandFailure, error) {
	switch kind {
	case CommandFailureKindNodeStopped:
		return CommandFailureNodeStopped{}, nil
	case CommandFailureKindBusy:
		return CommandFailureBusy{}, nil
	case CommandFailureKindPayloadTooLarge:
		return CommandFailurePayloadTooLarge{}, nil
	case CommandFailureKindUnknownDestination:
		return CommandFailureUnknownDestination{}, nil
	case CommandFailureKindNotSingleDestination:
		return CommandFailureNotSingleDestination{}, nil
	case CommandFailureKindAnnounceAppDataTooLong:
		return CommandFailureAnnounceAppDataTooLong{}, nil
	case CommandFailureKindUnknownInterface:
		return CommandFailureUnknownInterface{}, nil
	case CommandFailureKindNoRouteToDestination:
		return CommandFailureNoRouteToDestination{}, nil
	case CommandFailureKindNotDirectlyReachable:
		return CommandFailureNotDirectlyReachable{}, nil
	case CommandFailureKindPacketCulled:
		return CommandFailurePacketCulled{}, nil
	case CommandFailureKindDeliveryTimedOut:
		return CommandFailureDeliveryTimedOut{}, nil
	case CommandFailureKindInvalidBitrate:
		return CommandFailureInvalidBitrate{}, nil
	case CommandFailureKindBindFailed:
		return CommandFailureBindFailed{Detail: detail}, nil
	case CommandFailureKindWriteFailed:
		return CommandFailureWriteFailed{Detail: detail}, nil
	case CommandFailureKindUnsupportedByBackend:
		return CommandFailureUnsupportedByBackend{}, nil
	case CommandFailureKindUnknownLink:
		return CommandFailureUnknownLink{}, nil
	case CommandFailureKindLinkNotActive:
		return CommandFailureLinkNotActive{}, nil
	case CommandFailureKindEntropyUnavailable:
		return CommandFailureEntropyUnavailable{}, nil
	case CommandFailureKindNotLinkInitiator:
		return CommandFailureNotLinkInitiator{}, nil
	case CommandFailureKindIdentityNotHeld:
		return CommandFailureIdentityNotHeld{}, nil
	case CommandFailureKindUnknownRequestHandler:
		return CommandFailureUnknownRequestHandler{}, nil
	case CommandFailureKindRequestPolicyNotAllowList:
		return CommandFailureRequestPolicyNotAllowList{}, nil
	case CommandFailureKindRequestAllowListFull:
		return CommandFailureRequestAllowListFull{}, nil
	case CommandFailureKindLinkBusy:
		return CommandFailureLinkBusy{}, nil
	case CommandFailureKindResourceTableFull:
		return CommandFailureResourceTableFull{}, nil
	case CommandFailureKindResourceMetadataTooLarge:
		return CommandFailureResourceMetadataTooLarge{}, nil
	case CommandFailureKindResourceRejectedByPeer:
		return CommandFailureResourceRejectedByPeer{}, nil
	case CommandFailureKindResourceSequencingFailed:
		return CommandFailureResourceSequencingFailed{}, nil
	case CommandFailureKindResourcePredecessorFailed:
		return CommandFailureResourcePredecessorFailed{}, nil
	case CommandFailureKindChannelWindowFull:
		return CommandFailureChannelWindowFull{}, nil
	case CommandFailureKindChannelUntrackable:
		return CommandFailureChannelUntrackable{}, nil
	case CommandFailureKindInvalidChannelMessageType:
		return CommandFailureInvalidChannelMessageType{}, nil
	case CommandFailureKindInvalidConfiguration:
		return CommandFailureInvalidConfiguration{Detail: detail}, nil
	case CommandFailureKindResourceUploadCancelled:
		return CommandFailureResourceUploadCancelled{}, nil
	case CommandFailureKindResourceEarlyEof:
		return CommandFailureResourceEarlyEof{}, nil
	case CommandFailureKindResourceLengthOverrun:
		return CommandFailureResourceLengthOverrun{}, nil
	case CommandFailureKindPermissionDenied:
		return CommandFailurePermissionDenied{Detail: detail}, nil
	case CommandFailureKindDeviceUnavailable:
		return CommandFailureDeviceUnavailable{Detail: detail}, nil
	case CommandFailureKindConnectFailed:
		return CommandFailureConnectFailed{Detail: detail}, nil
	case CommandFailureKindBackendFailed:
		return CommandFailureBackendFailed{Detail: detail}, nil
	case CommandFailureKindResponseTooLarge:
		return CommandFailureResponseTooLarge{}, nil
	case CommandFailureKindLinkClosed:
		return CommandFailureLinkClosed{}, nil
	case CommandFailureKindResponseCancelledBySender:
		return CommandFailureResponseCancelledBySender{}, nil
	case CommandFailureKindResponseHashmapBeyondPartCount:
		return CommandFailureResponseHashmapBeyondPartCount{}, nil
	case CommandFailureKindResponseHashmapSkipsAhead:
		return CommandFailureResponseHashmapSkipsAhead{}, nil
	case CommandFailureKindResponseHashmapTooLong:
		return CommandFailureResponseHashmapTooLong{}, nil
	case CommandFailureKindResponseHashmapRagged:
		return CommandFailureResponseHashmapRagged{}, nil
	case CommandFailureKindResponseRetriesExhausted:
		return CommandFailureResponseRetriesExhausted{}, nil
	case CommandFailureKindResponseLinkVanished:
		return CommandFailureResponseLinkVanished{}, nil
	case CommandFailureKindResponseTransferUnopenable:
		return CommandFailureResponseTransferUnopenable{}, nil
	case CommandFailureKindResponseTransferCorrupt:
		return CommandFailureResponseTransferCorrupt{}, nil
	case CommandFailureKindResponseProofUnsendable:
		return CommandFailureResponseProofUnsendable{}, nil
	case CommandFailureKindResponseDecompressionFailed:
		return CommandFailureResponseDecompressionFailed{}, nil
	case CommandFailureKindResponseDecompressionTimedOut:
		return CommandFailureResponseDecompressionTimedOut{}, nil
	case CommandFailureKindResponseOpenTimedOut:
		return CommandFailureResponseOpenTimedOut{}, nil
	case CommandFailureKindResponseMetadataOverrun:
		return CommandFailureResponseMetadataOverrun{}, nil
	default:
		return nil, StatusError{
			Operation: "decode command failure",
			Status:    StatusBackendFailed,
		}
	}
}

func (command *Command) Close() error {
	command.stateMutex.Lock()
	native := command.native
	command.native = nativeCommand{}
	if native.pointer != nil {
		ffiCommandInterrupt(native)
	}
	command.stateMutex.Unlock()
	command.waitMutex.Lock()
	defer command.waitMutex.Unlock()
	if native.pointer != nil {
		ffiCommandClose(native)
	}
	return nil
}
