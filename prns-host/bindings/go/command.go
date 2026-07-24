package prns

import (
	"context"
	"sync"
)

type CommandSettlement interface {
	commandSettlement()
}

type CommandSucceeded struct {
	Outcome CommandOutcome
}

func (CommandSucceeded) commandSettlement() {}

type CommandFailed struct {
	Kind   CommandFailureKind
	Detail string
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
		return CommandFailed{Kind: result.failure, Detail: result.detail}, nil
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
	default:
		return nil, StatusError{
			Operation: "decode command outcome",
			Status:    StatusBackendFailed,
		}
	}
	return CommandSucceeded{Outcome: outcome}, nil
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
