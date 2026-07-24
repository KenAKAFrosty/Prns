package prns

/*
#cgo pkg-config: personal-rns
#include <stdlib.h>
#include <string.h>
#include <prns_host.h>
*/
import "C"

import (
	"unsafe"
)

const nativeNeverTimeout = ^uint32(0)

type nativeHost struct {
	pointer unsafe.Pointer
}

type nativeCommand struct {
	pointer unsafe.Pointer
}

type nativeEventStream struct {
	pointer unsafe.Pointer
}

type nativeEvent struct {
	pointer unsafe.Pointer
}

type nativeResourceStream struct {
	pointer unsafe.Pointer
}

type nativeAllocation struct {
	pointer unsafe.Pointer
	size    int
}

type nativeArena struct {
	allocations []nativeAllocation
}

func (arena *nativeArena) allocate(count int, size uintptr) (unsafe.Pointer, error) {
	if count == 0 {
		return nil, nil
	}
	pointer := C.calloc(C.size_t(count), C.size_t(size))
	if pointer == nil {
		return nil, ConfigError{Kind: ConfigAllocationFailed, Field: "native memory"}
	}
	arena.allocations = append(
		arena.allocations,
		nativeAllocation{pointer: pointer, size: count * int(size)},
	)
	return pointer, nil
}

func (arena *nativeArena) byteView(value []byte) (C.PrnsByteView, error) {
	if len(value) == 0 {
		return C.PrnsByteView{}, nil
	}
	pointer, err := arena.allocate(len(value), 1)
	if err != nil {
		return C.PrnsByteView{}, err
	}
	C.memcpy(pointer, unsafe.Pointer(&value[0]), C.size_t(len(value)))
	return C.PrnsByteView{
		data:   (*C.uint8_t)(pointer),
		length: C.size_t(len(value)),
	}, nil
}

func (arena *nativeArena) stringView(value string) (C.PrnsStringView, error) {
	view, err := arena.byteView([]byte(value))
	return C.PrnsStringView{
		data:   view.data,
		length: view.length,
	}, err
}

func (arena *nativeArena) close() {
	for index := len(arena.allocations) - 1; index >= 0; index-- {
		allocation := arena.allocations[index]
		C.memset(allocation.pointer, 0, C.size_t(allocation.size))
		C.free(allocation.pointer)
	}
	arena.allocations = nil
}

func marshalIdentity(
	arena *nativeArena,
	value IdentityConfig,
) (C.PrnsIdentityConfig, error) {
	result := C.PrnsIdentityConfig{
		struct_size: C.size_t(C.sizeof_PrnsIdentityConfig),
	}
	switch identity := value.(type) {
	case IdentityConfigExisting:
		secret, err := arena.byteView(identity.Secret[:])
		if err != nil {
			return result, err
		}
		result.kind = C.PRNS_IDENTITY_CONFIG_KIND_EXISTING
		result.secret = secret
	case IdentityConfigGenerateEphemeral:
		result.kind = C.PRNS_IDENTITY_CONFIG_KIND_GENERATE_EPHEMERAL
	case IdentityConfigLoadOrCreate:
		path, err := arena.stringView(identity.Path)
		if err != nil {
			return result, err
		}
		result.kind = C.PRNS_IDENTITY_CONFIG_KIND_LOAD_OR_CREATE
		result.path = path
	case nil:
		return result, ConfigError{Kind: ConfigMissingIdentity, Field: "identity"}
	default:
		return result, ConfigError{Kind: ConfigUnknownIdentity, Field: "identity"}
	}
	return result, nil
}

func marshalDestinationName(
	arena *nativeArena,
	value DestinationName,
) (C.PrnsDestinationName, error) {
	appName, err := arena.stringView(value.AppName)
	if err != nil {
		return C.PrnsDestinationName{}, err
	}
	aspectsPointer, err := arena.allocate(
		len(value.Aspects),
		C.sizeof_PrnsStringView,
	)
	if err != nil {
		return C.PrnsDestinationName{}, err
	}
	if len(value.Aspects) > 0 {
		aspects := unsafe.Slice(
			(*C.PrnsStringView)(aspectsPointer),
			len(value.Aspects),
		)
		for index, value := range value.Aspects {
			aspects[index], err = arena.stringView(value)
			if err != nil {
				return C.PrnsDestinationName{}, err
			}
		}
	}
	return C.PrnsDestinationName{
		struct_size:  C.size_t(C.sizeof_PrnsDestinationName),
		app_name:     appName,
		aspects:      (*C.PrnsStringView)(aspectsPointer),
		aspect_count: C.size_t(len(value.Aspects)),
	}, nil
}

func marshalDestinationIdentity(
	arena *nativeArena,
	value DestinationIdentityConfig,
) (C.PrnsDestinationIdentityConfigKind, C.PrnsIdentityConfig, error) {
	switch identity := value.(type) {
	case DestinationIdentityConfigHostIdentity:
		return C.PRNS_DESTINATION_IDENTITY_CONFIG_KIND_HOST_IDENTITY,
			C.PrnsIdentityConfig{}, nil
	case DestinationIdentityConfigDedicatedIdentity:
		native, err := marshalIdentity(arena, identity.Identity)
		return C.PRNS_DESTINATION_IDENTITY_CONFIG_KIND_DEDICATED_IDENTITY,
			native, err
	default:
		return 0, C.PrnsIdentityConfig{},
			ConfigError{
				Kind:  ConfigUnknownDestinationIdentity,
				Field: "destination identity",
			}
	}
}

func marshalDestination(
	arena *nativeArena,
	value DestinationConfig,
) (C.PrnsDestinationConfig, error) {
	result := C.PrnsDestinationConfig{
		struct_size: C.size_t(C.sizeof_PrnsDestinationConfig),
	}
	switch destination := value.(type) {
	case DestinationConfigPlain:
		name, err := marshalDestinationName(arena, destination.Name)
		if err != nil {
			return result, err
		}
		result.kind = C.PRNS_DESTINATION_CONFIG_KIND_PLAIN
		result.name = name
	case DestinationConfigSingle:
		name, err := marshalDestinationName(arena, destination.Name)
		if err != nil {
			return result, err
		}
		identityKind, identity, err := marshalDestinationIdentity(
			arena,
			destination.Identity,
		)
		if err != nil {
			return result, err
		}
		result.kind = C.PRNS_DESTINATION_CONFIG_KIND_SINGLE
		result.name = name
		result.identity_kind = identityKind
		result.dedicated_identity = identity
		if destination.AnnounceAppData != nil {
			result.announce_app_data, err = arena.byteView(
				*destination.AnnounceAppData,
			)
			if err != nil {
				return result, err
			}
		}
	default:
		return result, ConfigError{
			Kind:  ConfigUnknownDestination,
			Field: "destination",
		}
	}
	return result, nil
}

func marshalHostOptions(
	arena *nativeArena,
	options HostOptions,
) (C.PrnsHostOptions, error) {
	identity, err := marshalIdentity(arena, options.Identity)
	if err != nil {
		return C.PrnsHostOptions{}, err
	}
	destinationsPointer, err := arena.allocate(
		len(options.Destinations),
		C.sizeof_PrnsDestinationConfig,
	)
	if err != nil {
		return C.PrnsHostOptions{}, err
	}
	if len(options.Destinations) > 0 {
		destinations := unsafe.Slice(
			(*C.PrnsDestinationConfig)(destinationsPointer),
			len(options.Destinations),
		)
		for index, value := range options.Destinations {
			destinations[index], err = marshalDestination(arena, value)
			if err != nil {
				return C.PrnsHostOptions{}, err
			}
		}
	}
	capabilitiesPointer, err := arena.allocate(
		len(options.RequiredCapabilities),
		unsafe.Sizeof(C.PrnsCapability(0)),
	)
	if err != nil {
		return C.PrnsHostOptions{}, err
	}
	if len(options.RequiredCapabilities) > 0 {
		capabilities := unsafe.Slice(
			(*C.PrnsCapability)(capabilitiesPointer),
			len(options.RequiredCapabilities),
		)
		for index, value := range options.RequiredCapabilities {
			capabilities[index] = C.PrnsCapability(value)
		}
	}
	version, err := arena.stringView(ProductVersion)
	if err != nil {
		return C.PrnsHostOptions{}, err
	}
	limits := options.Limits
	if limits.PendingCommands < 1 ||
		limits.ApplicationEvents < 1 ||
		limits.RetainedEventBytes < 1 ||
		limits.Diagnostics < 1 {
		return C.PrnsHostOptions{}, ConfigError{
			Kind:  ConfigInvalidLimits,
			Field: "limits",
		}
	}
	return C.PrnsHostOptions{
		struct_size:              C.size_t(C.sizeof_PrnsHostOptions),
		required_abi:             C.uint32_t(HostContractABI),
		required_product_version: version,
		limits: C.PrnsLimits{
			struct_size:          C.size_t(C.sizeof_PrnsLimits),
			pending_commands:     C.size_t(limits.PendingCommands),
			application_events:   C.size_t(limits.ApplicationEvents),
			retained_event_bytes: C.size_t(limits.RetainedEventBytes),
			diagnostics:          C.size_t(limits.Diagnostics),
		},
		role:                      C.PrnsHostRole(options.Role),
		identity:                  identity,
		destinations:              (*C.PrnsDestinationConfig)(destinationsPointer),
		destination_count:         C.size_t(len(options.Destinations)),
		required_capabilities:     (*C.PrnsCapability)(capabilitiesPointer),
		required_capability_count: C.size_t(len(options.RequiredCapabilities)),
	}, nil
}

func ffiContractInfo() (uint32, uint32, string, Status) {
	info := C.PrnsContractInfo{
		struct_size: C.size_t(C.sizeof_PrnsContractInfo),
	}
	status := Status(C.prns_contract_info(&info))
	return uint32(info.abi),
		uint32(info.schema_version),
		string(copyStringView(info.product_version)),
		status
}

func ffiCreate(options HostOptions) (nativeHost, Status, error) {
	arena := nativeArena{}
	defer arena.close()
	nativeOptions, err := marshalHostOptions(&arena, options)
	if err != nil {
		return nativeHost{}, StatusInvalidArgument, err
	}
	var pointer *C.PrnsHost
	status := Status(C.prns_host_create(&nativeOptions, &pointer))
	return nativeHost{pointer: unsafe.Pointer(pointer)}, status, nil
}

func ffiHostClose(host nativeHost) {
	C.prns_host_release((*C.PrnsHost)(host.pointer))
}

func ffiHostStop(host nativeHost) Status {
	return Status(C.prns_host_stop((*C.PrnsHost)(host.pointer)))
}

func ffiIdentityHash(host nativeHost) (IdentityHash, Status) {
	var view C.PrnsByteView
	status := Status(C.prns_host_identity_hash((*C.PrnsHost)(host.pointer), &view))
	return IdentityHash(copyFixed(view, IdentityHashLength)), status
}

func ffiDestinationHashes(host nativeHost) ([]DestinationHash, Status) {
	count := int(C.prns_host_destination_count((*C.PrnsHost)(host.pointer)))
	values := make([]DestinationHash, count)
	for index := range values {
		var view C.PrnsByteView
		status := Status(C.prns_host_destination_hash(
			(*C.PrnsHost)(host.pointer),
			C.size_t(index),
			&view,
		))
		if status != StatusOk {
			return nil, status
		}
		values[index] = DestinationHash(copyFixed(view, DestinationHashLength))
	}
	return values, StatusOk
}

func marshalBitrate(value Bitrate) (C.PrnsBitrateKind, C.uint64_t, error) {
	switch bitrate := value.(type) {
	case BitrateAuto:
		return C.PRNS_BITRATE_KIND_AUTO, 0, nil
	case BitrateBitsPerSecond:
		return C.PRNS_BITRATE_KIND_BITS_PER_SECOND, C.uint64_t(bitrate.Value), nil
	default:
		return 0, 0, ConfigError{Kind: ConfigUnknownDestination, Field: "bitrate"}
	}
}

func ffiExecute(host nativeHost, value HostCommand) (nativeCommand, Status, error) {
	arena := nativeArena{}
	defer arena.close()
	var pointer *C.PrnsCommand
	var status Status
	switch command := value.(type) {
	case HostCommandAnnounce:
		destination, err := arena.byteView(command.Destination[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		var nativeInterface *C.PrnsByteView
		if command.Interface != nil {
			view, err := arena.byteView(command.Interface[:])
			if err != nil {
				return nativeCommand{}, StatusInvalidArgument, err
			}
			nativeInterface = &view
		}
		status = Status(C.prns_host_announce(
			(*C.PrnsHost)(host.pointer),
			destination,
			nativeInterface,
			&pointer,
		))
	case HostCommandSendSinglePacket:
		destination, err := arena.byteView(command.Destination[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		payload, err := arena.byteView(command.Payload)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_send_single_packet(
			(*C.PrnsHost)(host.pointer),
			destination,
			payload,
			&pointer,
		))
	case HostCommandCloseLink:
		linkID, err := arena.byteView(command.LinkId[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_close_link(
			(*C.PrnsHost)(host.pointer),
			linkID,
			&pointer,
		))
	case HostCommandAttachTcpServer:
		bind, err := arena.stringView(command.Bind)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		kind, bits, err := marshalBitrate(command.Bitrate)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_attach_tcp_server(
			(*C.PrnsHost)(host.pointer),
			bind,
			kind,
			bits,
			&pointer,
		))
	case HostCommandAttachTcpClient:
		target, err := arena.stringView(command.Target)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		kind, bits, err := marshalBitrate(command.Bitrate)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_attach_tcp_client(
			(*C.PrnsHost)(host.pointer),
			target,
			kind,
			bits,
			&pointer,
		))
	case HostCommandAttachUdp:
		local, err := arena.stringView(command.Local)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		peer, err := arena.stringView(command.Peer)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		kind, bits, err := marshalBitrate(command.Bitrate)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_attach_udp(
			(*C.PrnsHost)(host.pointer),
			local,
			peer,
			kind,
			bits,
			&pointer,
		))
	case HostCommandDetachInterface:
		interfaceID, err := arena.byteView(command.Interface[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_detach_interface(
			(*C.PrnsHost)(host.pointer),
			interfaceID,
			&pointer,
		))
	default:
		return nativeCommand{}, StatusInvalidArgument,
			ConfigError{Kind: ConfigUnknownDestination, Field: "command"}
	}
	return nativeCommand{pointer: unsafe.Pointer(pointer)}, status, nil
}

type nativeCommandResult struct {
	outcome   CommandOutcomeKind
	failure   CommandFailureKind
	evidence  DeliveryEvidenceKind
	rttMillis uint64
	value     []byte
	detail    string
}

func ffiCommandWait(command nativeCommand) (nativeCommandResult, Status) {
	result := C.PrnsCommandResult{
		struct_size: C.size_t(C.sizeof_PrnsCommandResult),
	}
	status := Status(C.prns_command_wait(
		(*C.PrnsCommand)(command.pointer),
		C.uint32_t(nativeNeverTimeout),
		&result,
	))
	return nativeCommandResult{
		outcome:   CommandOutcomeKind(result.outcome),
		failure:   CommandFailureKind(result.failure),
		evidence:  DeliveryEvidenceKind(result.evidence),
		rttMillis: uint64(result.rtt_millis),
		value:     copyByteView(result.value),
		detail:    string(copyStringView(result.detail)),
	}, status
}

func ffiCommandInterrupt(command nativeCommand) {
	C.prns_command_interrupt_wait((*C.PrnsCommand)(command.pointer))
}

func ffiCommandClose(command nativeCommand) {
	C.prns_command_release((*C.PrnsCommand)(command.pointer))
}

func ffiClaimApplication(host nativeHost) (nativeEventStream, Status) {
	var pointer *C.PrnsEventStream
	status := Status(C.prns_host_claim_application_events(
		(*C.PrnsHost)(host.pointer),
		&pointer,
	))
	return nativeEventStream{pointer: unsafe.Pointer(pointer)}, status
}

func ffiClaimDiagnostics(host nativeHost) (nativeEventStream, Status) {
	var pointer *C.PrnsEventStream
	status := Status(C.prns_host_claim_diagnostics(
		(*C.PrnsHost)(host.pointer),
		&pointer,
	))
	return nativeEventStream{pointer: unsafe.Pointer(pointer)}, status
}

func ffiEventNext(stream nativeEventStream) (nativeEvent, Status) {
	var pointer *C.PrnsEvent
	status := Status(C.prns_event_stream_next(
		(*C.PrnsEventStream)(stream.pointer),
		C.uint32_t(nativeNeverTimeout),
		&pointer,
	))
	return nativeEvent{pointer: unsafe.Pointer(pointer)}, status
}

func ffiEventStreamInterrupt(stream nativeEventStream) {
	C.prns_event_stream_interrupt_wait((*C.PrnsEventStream)(stream.pointer))
}

func ffiEventStreamClose(stream nativeEventStream) {
	C.prns_event_stream_release((*C.PrnsEventStream)(stream.pointer))
}

func ffiEventClose(event nativeEvent) {
	C.prns_event_release((*C.PrnsEvent)(event.pointer))
}

func ffiEventKind(event nativeEvent) uint32 {
	return uint32(C.prns_event_kind((*C.PrnsEvent)(event.pointer)))
}

func ffiEventBytes(event nativeEvent, field EventField) ([]byte, Status) {
	var view C.PrnsByteView
	status := Status(C.prns_event_bytes(
		(*C.PrnsEvent)(event.pointer),
		C.PrnsEventField(field),
		&view,
	))
	return copyByteView(view), status
}

func ffiEventString(event nativeEvent, field EventField) (string, Status) {
	var view C.PrnsStringView
	status := Status(C.prns_event_string(
		(*C.PrnsEvent)(event.pointer),
		C.PrnsEventField(field),
		&view,
	))
	return string(copyStringView(view)), status
}

func ffiEventU64(event nativeEvent, field EventField) (uint64, Status) {
	var value C.uint64_t
	status := Status(C.prns_event_u64(
		(*C.PrnsEvent)(event.pointer),
		C.PrnsEventField(field),
		&value,
	))
	return uint64(value), status
}

func ffiEventU128(event nativeEvent, field EventField) (UInt128, Status) {
	var low C.uint64_t
	var high C.uint64_t
	status := Status(C.prns_event_u128(
		(*C.PrnsEvent)(event.pointer),
		C.PrnsEventField(field),
		&low,
		&high,
	))
	return UInt128{Low: uint64(low), High: uint64(high)}, status
}

func ffiEventResource(event nativeEvent) (nativeResourceStream, Status) {
	var pointer *C.PrnsResourceStream
	status := Status(C.prns_event_resource_stream(
		(*C.PrnsEvent)(event.pointer),
		&pointer,
	))
	return nativeResourceStream{pointer: unsafe.Pointer(pointer)}, status
}

func ffiResourceNext(
	stream nativeResourceStream,
	maximumBytes int,
) ([]byte, bool, Status) {
	var view C.PrnsByteView
	var finished C.uint8_t
	status := Status(C.prns_resource_stream_next(
		(*C.PrnsResourceStream)(stream.pointer),
		C.size_t(maximumBytes),
		&view,
		&finished,
	))
	return copyByteView(view), finished != 0, status
}

func ffiResourceClose(stream nativeResourceStream) {
	C.prns_resource_stream_release((*C.PrnsResourceStream)(stream.pointer))
}

func copyByteView(view C.PrnsByteView) []byte {
	if view.length == 0 {
		return []byte{}
	}
	return C.GoBytes(unsafe.Pointer(view.data), C.int(view.length))
}

func copyStringView(view C.PrnsStringView) []byte {
	if view.length == 0 {
		return []byte{}
	}
	return C.GoBytes(unsafe.Pointer(view.data), C.int(view.length))
}

func copyFixed(view C.PrnsByteView, length int) []byte {
	value := copyByteView(view)
	if len(value) != length {
		return make([]byte, length)
	}
	return value
}
