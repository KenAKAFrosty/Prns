package prns

import (
	"context"
	"fmt"
	"io"
	"sync"
	"time"
)

type StatusError struct {
	Operation string
	Status    Status
}

func (failure StatusError) Error() string {
	return fmt.Sprintf(
		"personal-rns: %s failed with status %d",
		failure.Operation,
		failure.Status,
	)
}

type ContractMismatch struct {
	ExpectedABI     uint32
	ActualABI       uint32
	ExpectedSchema  uint32
	ActualSchema    uint32
	ExpectedVersion string
	ActualVersion   string
}

func (failure ContractMismatch) Error() string {
	return fmt.Sprintf(
		"personal-rns: native host contract mismatch "+
			"(ABI %d/%d, schema %d/%d, product %q/%q)",
		failure.ActualABI,
		failure.ExpectedABI,
		failure.ActualSchema,
		failure.ExpectedSchema,
		failure.ActualVersion,
		failure.ExpectedVersion,
	)
}

type Host struct {
	mutex             sync.Mutex
	native            nativeHost
	identityHash      IdentityHash
	destinationHashes []DestinationHash
}

func NewHost(options HostOptions) (*Host, error) {
	abi, schema, version, status := ffiContractInfo()
	if status != StatusOk {
		return nil, StatusError{Operation: "read contract", Status: status}
	}
	if abi != HostContractABI ||
		schema != HostSchemaVersion ||
		version != ProductVersion {
		return nil, ContractMismatch{
			ExpectedABI:     HostContractABI,
			ActualABI:       abi,
			ExpectedSchema:  HostSchemaVersion,
			ActualSchema:    schema,
			ExpectedVersion: ProductVersion,
			ActualVersion:   version,
		}
	}
	native, status, err := ffiCreate(options)
	if err != nil {
		return nil, err
	}
	if status != StatusOk {
		return nil, StatusError{Operation: "create host", Status: status}
	}
	identityHash, status := ffiIdentityHash(native)
	if status != StatusOk {
		ffiHostClose(native)
		return nil, StatusError{Operation: "read identity hash", Status: status}
	}
	destinationHashes, status := ffiDestinationHashes(native)
	if status != StatusOk {
		ffiHostClose(native)
		return nil, StatusError{
			Operation: "read destination hashes",
			Status:    status,
		}
	}
	return &Host{
		native:            native,
		identityHash:      identityHash,
		destinationHashes: destinationHashes,
	}, nil
}

func (host *Host) IdentityHash() IdentityHash {
	return host.identityHash
}

func (host *Host) DestinationHashes() []DestinationHash {
	result := make([]DestinationHash, len(host.destinationHashes))
	copy(result, host.destinationHashes)
	return result
}

func (host *Host) BackendInfo() (BackendInfo, error) {
	info, status := ffiBackendInfo()
	if status != StatusOk {
		return BackendInfo{}, StatusError{
			Operation: "read backend info",
			Status:    status,
		}
	}
	return info, nil
}

func (host *Host) Snapshot(timeout time.Duration) (HostSnapshot, error) {
	host.mutex.Lock()
	defer host.mutex.Unlock()
	if host.native.pointer == nil {
		return HostSnapshot{}, StatusError{
			Operation: "capture snapshot",
			Status:    StatusStopped,
		}
	}
	millis := timeout.Milliseconds()
	if millis < 0 {
		millis = int64(nativeNeverTimeout)
	}
	if millis > int64(nativeNeverTimeout) {
		millis = int64(nativeNeverTimeout)
	}
	result, status := ffiHostSnapshot(host.native, uint32(millis))
	if status != StatusOk {
		return HostSnapshot{}, StatusError{
			Operation: "capture snapshot",
			Status:    status,
		}
	}
	return result, nil
}

func (host *Host) BeginResourceUpload(
	linkID LinkId,
	declaredLength uint64,
	packedMetadata *[]byte,
	compression ResourceCompression,
) (*ResourceUpload, error) {
	host.mutex.Lock()
	defer host.mutex.Unlock()
	if host.native.pointer == nil {
		return nil, StatusError{
			Operation: "begin resource upload",
			Status:    StatusStopped,
		}
	}
	native, status, err := ffiBeginResourceUpload(
		host.native,
		linkID,
		declaredLength,
		packedMetadata,
		compression,
	)
	if err != nil {
		return nil, err
	}
	if status != StatusOk {
		return nil, StatusError{
			Operation: "begin resource upload",
			Status:    status,
		}
	}
	return &ResourceUpload{native: native}, nil
}

func (host *Host) SendResourceStream(
	ctx context.Context,
	linkID LinkId,
	declaredLength uint64,
	source io.Reader,
	packedMetadata *[]byte,
	compression ResourceCompression,
) (CommandSettlement, error) {
	upload, err := host.BeginResourceUpload(
		linkID,
		declaredLength,
		packedMetadata,
		compression,
	)
	if err != nil {
		return nil, err
	}
	defer upload.Close()
	buffer := make([]byte, 256*1024)
	for {
		count, readError := source.Read(buffer)
		if count > 0 {
			if err := upload.Write(ctx, buffer[:count]); err != nil {
				return nil, err
			}
		}
		if readError == io.EOF {
			break
		}
		if readError != nil {
			return nil, readError
		}
	}
	command, err := upload.Finish()
	if err != nil {
		return nil, err
	}
	defer command.Close()
	return command.Wait(ctx)
}

func (host *Host) Execute(value HostCommand) (*Command, error) {
	host.mutex.Lock()
	defer host.mutex.Unlock()
	if host.native.pointer == nil {
		return nil, StatusError{Operation: "submit command", Status: StatusStopped}
	}
	native, status, err := ffiExecute(host.native, value)
	if err != nil {
		return nil, err
	}
	if status != StatusOk {
		return nil, StatusError{Operation: "submit command", Status: status}
	}
	return &Command{native: native}, nil
}

func (host *Host) ClaimApplicationEvents() (
	StreamClaim[*ApplicationEventStream],
	error,
) {
	host.mutex.Lock()
	defer host.mutex.Unlock()
	if host.native.pointer == nil {
		return nil, StatusError{
			Operation: "claim application events",
			Status:    StatusStopped,
		}
	}
	native, status := ffiClaimApplication(host.native)
	switch status {
	case StatusOk:
		return StreamClaimed[*ApplicationEventStream]{
			Stream: newApplicationEventStream(native),
		}, nil
	case StatusAlreadyClaimed:
		return StreamAlreadyClaimed[*ApplicationEventStream]{}, nil
	default:
		return nil, StatusError{
			Operation: "claim application events",
			Status:    status,
		}
	}
}

func (host *Host) ClaimDiagnostics() (
	StreamClaim[*DiagnosticEventStream],
	error,
) {
	host.mutex.Lock()
	defer host.mutex.Unlock()
	if host.native.pointer == nil {
		return nil, StatusError{
			Operation: "claim diagnostics",
			Status:    StatusStopped,
		}
	}
	native, status := ffiClaimDiagnostics(host.native)
	switch status {
	case StatusOk:
		return StreamClaimed[*DiagnosticEventStream]{
			Stream: newDiagnosticEventStream(native),
		}, nil
	case StatusAlreadyClaimed:
		return StreamAlreadyClaimed[*DiagnosticEventStream]{}, nil
	default:
		return nil, StatusError{
			Operation: "claim diagnostics",
			Status:    status,
		}
	}
}

func (host *Host) Stop() error {
	host.mutex.Lock()
	defer host.mutex.Unlock()
	if host.native.pointer == nil {
		return nil
	}
	status := ffiHostStop(host.native)
	if status != StatusOk && status != StatusStopped {
		return StatusError{Operation: "stop host", Status: status}
	}
	return nil
}

func (host *Host) Close() error {
	host.mutex.Lock()
	defer host.mutex.Unlock()
	if host.native.pointer == nil {
		return nil
	}
	ffiHostClose(host.native)
	host.native = nativeHost{}
	return nil
}
