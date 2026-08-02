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

type nativeResourceUpload struct {
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

func marshalPersistence(
	arena *nativeArena,
	value PersistenceConfig,
) (C.PrnsPersistenceConfig, error) {
	result := C.PrnsPersistenceConfig{
		struct_size: C.size_t(C.sizeof_PrnsPersistenceConfig),
	}
	switch persistence := value.(type) {
	case PersistenceConfigEphemeral:
		result.kind = C.PRNS_PERSISTENCE_CONFIG_KIND_EPHEMERAL
	case PersistenceConfigDirectory:
		path, err := arena.stringView(persistence.Path)
		if err != nil {
			return result, err
		}
		result.kind = C.PRNS_PERSISTENCE_CONFIG_KIND_DIRECTORY
		result.path = path
	case nil:
		result.kind = C.PRNS_PERSISTENCE_CONFIG_KIND_EPHEMERAL
	default:
		return result, ConfigError{Kind: ConfigUnknownPersistence, Field: "persistence"}
	}
	return result, nil
}

func nativeBool(value bool) C.uint8_t {
	if value {
		return 1
	}
	return 0
}

func marshalStringViews(
	arena *nativeArena,
	values []string,
) (*C.PrnsStringView, error) {
	pointer, err := arena.allocate(len(values), C.sizeof_PrnsStringView)
	if err != nil {
		return nil, err
	}
	if len(values) == 0 {
		return nil, nil
	}
	views := unsafe.Slice((*C.PrnsStringView)(pointer), len(values))
	for index, value := range values {
		views[index], err = arena.stringView(value)
		if err != nil {
			return nil, err
		}
	}
	return (*C.PrnsStringView)(pointer), nil
}

func marshalSerialLine(value SerialLineConfig) C.PrnsSerialLineConfig {
	return C.PrnsSerialLineConfig{
		struct_size: C.size_t(C.sizeof_PrnsSerialLineConfig),
		baud:        C.uint32_t(value.Baud),
		data_bits:   C.PrnsSerialDataBits(value.DataBits),
		parity:      C.PrnsSerialParity(value.Parity),
		stop_bits:   C.PrnsSerialStopBits(value.StopBits),
	}
}

func marshalRNodeRadio(value RNodeRadioConfig) C.PrnsRNodeRadioConfig {
	return C.PrnsRNodeRadioConfig{
		struct_size:      C.size_t(C.sizeof_PrnsRNodeRadioConfig),
		frequency_hz:     C.uint64_t(value.FrequencyHz),
		bandwidth_hz:     C.uint32_t(value.BandwidthHz),
		tx_power_dbm:     C.int16_t(value.TxPowerDbm),
		spreading_factor: C.uint8_t(value.SpreadingFactor),
		coding_rate:      C.uint8_t(value.CodingRate),
	}
}

func marshalInterface(
	arena *nativeArena,
	value InterfaceConfig,
) (C.PrnsInterfaceConfig, error) {
	result := C.PrnsInterfaceConfig{
		struct_size: C.size_t(C.sizeof_PrnsInterfaceConfig),
	}
	setBitrate := func(value Bitrate) error {
		kind, bits, err := marshalBitrate(value)
		if err != nil {
			return err
		}
		result.bitrate_kind = kind
		result.bitrate_bps = bits
		return nil
	}
	setStation := func(callsign *string, interval *uint64) error {
		if callsign != nil {
			value, err := arena.stringView(*callsign)
			if err != nil {
				return err
			}
			result.has_station_callsign = 1
			result.station_callsign = value
		}
		if interval != nil {
			result.has_station_interval_seconds = 1
			result.station_interval_seconds = C.uint64_t(*interval)
		}
		return nil
	}
	var err error
	switch config := value.(type) {
	case InterfaceConfigAutoLan:
		result.kind = C.PRNS_INTERFACE_KIND_AUTO_LAN
		if config.GroupId != nil {
			result.has_group_id = 1
			result.group_id, err = arena.stringView(*config.GroupId)
			if err != nil {
				return result, err
			}
		}
		if config.DiscoveryScope != nil {
			result.has_discovery_scope = 1
			result.discovery_scope = C.PrnsDiscoveryScope(*config.DiscoveryScope)
		}
		if config.DiscoveryPort != nil {
			result.has_discovery_port = 1
			result.discovery_port = C.uint16_t(*config.DiscoveryPort)
		}
		if config.DataPort != nil {
			result.has_data_port = 1
			result.data_port = C.uint16_t(*config.DataPort)
		}
		result.devices, err = marshalStringViews(arena, config.Devices)
		if err != nil {
			return result, err
		}
		result.device_count = C.size_t(len(config.Devices))
		result.ignored_devices, err = marshalStringViews(arena, config.IgnoredDevices)
		if err != nil {
			return result, err
		}
		result.ignored_device_count = C.size_t(len(config.IgnoredDevices))
		if config.MulticastAddressType != nil {
			result.has_multicast_address_type = 1
			result.multicast_address_type = C.PrnsMulticastAddressType(
				*config.MulticastAddressType,
			)
		}
	case InterfaceConfigTcpClient:
		result.kind = C.PRNS_INTERFACE_KIND_TCP_CLIENT
		result.target, err = arena.stringView(config.Target)
		if err == nil {
			err = setBitrate(config.Bitrate)
		}
	case InterfaceConfigTcpServer:
		result.kind = C.PRNS_INTERFACE_KIND_TCP_SERVER
		result.bind, err = arena.stringView(config.Bind)
		if err == nil {
			err = setBitrate(config.Bitrate)
		}
	case InterfaceConfigUdp:
		result.kind = C.PRNS_INTERFACE_KIND_UDP
		result.local, err = arena.stringView(config.Local)
		if err == nil {
			result.peer, err = arena.stringView(config.Peer)
		}
		if err == nil {
			err = setBitrate(config.Bitrate)
		}
	case InterfaceConfigSerial:
		result.kind = C.PRNS_INTERFACE_KIND_SERIAL
		result.port, err = arena.stringView(config.Port)
		result.line = marshalSerialLine(config.Line)
	case InterfaceConfigKiss:
		result.kind = C.PRNS_INTERFACE_KIND_KISS
		result.port, err = arena.stringView(config.Port)
		result.line = marshalSerialLine(config.Line)
		result.flow_control = nativeBool(config.FlowControl)
		result.preamble_millis = C.uint32_t(config.PreambleMillis)
		result.transmit_tail_millis = C.uint32_t(config.TransmitTailMillis)
		result.persistence = C.uint8_t(config.Persistence)
		result.slot_time_millis = C.uint32_t(config.SlotTimeMillis)
		if err == nil {
			err = setStation(config.StationCallsign, config.StationIntervalSeconds)
		}
	case InterfaceConfigAx25Kiss:
		result.kind = C.PRNS_INTERFACE_KIND_AX25_KISS
		result.port, err = arena.stringView(config.Port)
		result.line = marshalSerialLine(config.Line)
		result.flow_control = nativeBool(config.FlowControl)
		result.preamble_millis = C.uint32_t(config.PreambleMillis)
		result.transmit_tail_millis = C.uint32_t(config.TransmitTailMillis)
		result.persistence = C.uint8_t(config.Persistence)
		result.slot_time_millis = C.uint32_t(config.SlotTimeMillis)
		if err == nil {
			result.callsign, err = arena.stringView(config.Callsign)
		}
		result.ssid = C.uint8_t(config.Ssid)
	case InterfaceConfigRNode:
		result.kind = C.PRNS_INTERFACE_KIND_R_NODE
		result.port, err = arena.stringView(config.Port)
		result.radio = marshalRNodeRadio(config.Radio)
		result.flow_control = nativeBool(config.FlowControl)
		if err == nil {
			err = setStation(config.StationCallsign, config.StationIntervalSeconds)
		}
		if config.AirtimeLimitShortCentiPercent != nil {
			result.has_airtime_limit_short_centi_percent = 1
			result.airtime_limit_short_centi_percent = C.uint16_t(
				*config.AirtimeLimitShortCentiPercent,
			)
		}
		if config.AirtimeLimitLongCentiPercent != nil {
			result.has_airtime_limit_long_centi_percent = 1
			result.airtime_limit_long_centi_percent = C.uint16_t(
				*config.AirtimeLimitLongCentiPercent,
			)
		}
	case InterfaceConfigMultiRNode:
		result.kind = C.PRNS_INTERFACE_KIND_MULTI_R_NODE
		result.port, err = arena.stringView(config.Port)
		if err == nil {
			err = setStation(config.StationCallsign, config.StationIntervalSeconds)
		}
		var membersPointer unsafe.Pointer
		if err == nil {
			membersPointer, err = arena.allocate(
				len(config.Members),
				C.sizeof_PrnsMultiRNodeMemberConfig,
			)
		}
		if err == nil && len(config.Members) > 0 {
			members := unsafe.Slice(
				(*C.PrnsMultiRNodeMemberConfig)(membersPointer),
				len(config.Members),
			)
			for index, member := range config.Members {
				members[index].struct_size = C.size_t(C.sizeof_PrnsMultiRNodeMemberConfig)
				members[index].name, err = arena.stringView(member.Name)
				if err != nil {
					break
				}
				members[index].virtual_port = C.uint8_t(member.VirtualPort)
				members[index].radio = marshalRNodeRadio(member.Radio)
				members[index].flow_control = nativeBool(member.FlowControl)
				members[index].outgoing = nativeBool(member.Outgoing)
			}
		}
		result.members = (*C.PrnsMultiRNodeMemberConfig)(membersPointer)
		result.member_count = C.size_t(len(config.Members))
	case InterfaceConfigPipe:
		result.kind = C.PRNS_INTERFACE_KIND_PIPE
		result.command, err = marshalStringViews(arena, config.Command)
		result.command_count = C.size_t(len(config.Command))
		result.respawn_delay_millis = C.uint64_t(config.RespawnDelayMillis)
	case InterfaceConfigBackboneClient:
		result.kind = C.PRNS_INTERFACE_KIND_BACKBONE_CLIENT
		result.target, err = arena.stringView(config.Target)
		if err == nil {
			err = setBitrate(config.Bitrate)
		}
	case InterfaceConfigBackboneServer:
		result.kind = C.PRNS_INTERFACE_KIND_BACKBONE_SERVER
		result.bind, err = arena.stringView(config.Bind)
		if err == nil {
			err = setBitrate(config.Bitrate)
		}
	case InterfaceConfigI2p:
		result.kind = C.PRNS_INTERFACE_KIND_I2P
		result.peers, err = marshalStringViews(arena, config.Peers)
		result.peer_count = C.size_t(len(config.Peers))
		result.connectable = nativeBool(config.Connectable)
	case InterfaceConfigWeave:
		result.kind = C.PRNS_INTERFACE_KIND_WEAVE
		result.port, err = arena.stringView(config.Port)
	case InterfaceConfigAutomaticUsb:
		result.kind = C.PRNS_INTERFACE_KIND_AUTOMATIC_USB
	case InterfaceConfigAutomaticBluetoothLe:
		result.kind = C.PRNS_INTERFACE_KIND_AUTOMATIC_BLUETOOTH_LE
	case InterfaceConfigWebSocketClient:
		result.kind = C.PRNS_INTERFACE_KIND_WEB_SOCKET_CLIENT
		result.target, err = arena.stringView(config.Target)
	case InterfaceConfigWebSocketServer:
		result.kind = C.PRNS_INTERFACE_KIND_WEB_SOCKET_SERVER
		result.bind, err = arena.stringView(config.Bind)
	case InterfaceConfigBrowserRendezvous:
		result.kind = C.PRNS_INTERFACE_KIND_BROWSER_RENDEZVOUS
		result.url, err = arena.stringView(config.Url)
	case nil:
		err = ConfigError{Kind: ConfigUnknownInterface, Field: "interface"}
	default:
		err = ConfigError{Kind: ConfigUnknownInterface, Field: "interface"}
	}
	return result, err
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
		if destination.MaximumRequestBytes != nil {
			if *destination.MaximumRequestBytes > SafeUintMax {
				return result, ConfigError{
					Kind:  ConfigInvalidLimits,
					Field: "maximum request bytes",
				}
			}
			result.has_maximum_request_bytes = 1
			result.maximum_request_bytes = C.uint64_t(*destination.MaximumRequestBytes)
		}
		requestHandlersPointer, err := arena.allocate(
			len(destination.RequestHandlers),
			C.sizeof_PrnsRequestHandlerConfig,
		)
		if err != nil {
			return result, err
		}
		if len(destination.RequestHandlers) > 0 {
			requestHandlers := unsafe.Slice(
				(*C.PrnsRequestHandlerConfig)(requestHandlersPointer),
				len(destination.RequestHandlers),
			)
			for index, handler := range destination.RequestHandlers {
				path, pathError := arena.stringView(handler.Path)
				if pathError != nil {
					return result, pathError
				}
				var policy C.PrnsRequestPolicy
				switch handler.Policy {
				case RequestPolicyAllowNone:
					policy = C.PRNS_REQUEST_POLICY_ALLOW_NONE
				case RequestPolicyAllowAll:
					policy = C.PRNS_REQUEST_POLICY_ALLOW_ALL
				case RequestPolicyAllowList:
					policy = C.PRNS_REQUEST_POLICY_ALLOW_LIST
				default:
					return result, ConfigError{
						Kind:  ConfigInvalidRequestPolicy,
						Field: "request handler policy",
					}
				}
				requestHandlers[index] = C.PrnsRequestHandlerConfig{
					struct_size: C.size_t(C.sizeof_PrnsRequestHandlerConfig),
					path:        path,
					policy:      policy,
				}
			}
		}
		result.request_handlers = (*C.PrnsRequestHandlerConfig)(requestHandlersPointer)
		result.request_handler_count = C.size_t(len(destination.RequestHandlers))
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
	persistence, err := marshalPersistence(arena, options.Persistence)
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
		required_schema_version:  C.uint32_t(HostSchemaVersion),
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
		persistence:               persistence,
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

func copyBackendInfo(info C.PrnsBackendInfo) BackendInfo {
	capabilities := make([]Capability, int(info.capability_count))
	if len(capabilities) > 0 {
		native := unsafe.Slice(info.capabilities, len(capabilities))
		for index, value := range native {
			capabilities[index] = Capability(value)
		}
	}
	interfaceKinds := make([]InterfaceKind, int(info.interface_kind_count))
	if len(interfaceKinds) > 0 {
		native := unsafe.Slice(info.interface_kinds, len(interfaceKinds))
		for index, value := range native {
			interfaceKinds[index] = InterfaceKind(value)
		}
	}
	return BackendInfo{
		Backend:        BackendKind(info.backend),
		Capabilities:   capabilities,
		InterfaceKinds: interfaceKinds,
	}
}

func ffiBackendInfo() (BackendInfo, Status) {
	info := C.PrnsBackendInfo{
		struct_size: C.size_t(C.sizeof_PrnsBackendInfo),
	}
	status := Status(C.prns_backend_info(&info))
	return copyBackendInfo(info), status
}

func ffiHostSnapshot(host nativeHost, timeoutMillis uint32) (HostSnapshot, Status) {
	var inspection *C.PrnsHostInspection
	status := Status(C.prns_host_snapshot(
		(*C.PrnsHost)(host.pointer),
		C.uint32_t(timeoutMillis),
		&inspection,
	))
	if status != StatusOk {
		return HostSnapshot{}, status
	}
	defer C.prns_host_snapshot_release(inspection)
	value := C.PrnsHostSnapshot{
		struct_size: C.size_t(C.sizeof_PrnsHostSnapshot),
	}
	status = Status(C.prns_host_snapshot_read(inspection, &value))
	if status != StatusOk {
		return HostSnapshot{}, status
	}
	interfaces := make([]InterfaceSnapshot, int(value.interface_count))
	if len(interfaces) > 0 {
		native := unsafe.Slice(value.interfaces, len(interfaces))
		for index, item := range native {
			var interfaceID InterfaceId
			copy(interfaceID[:], copyFixed(item.interface_id, InterfaceIdLength))
			var name *string
			if item.has_name != 0 {
				decoded := string(copyStringView(item.name))
				name = &decoded
			}
			var kind *InterfaceKind
			if item.has_kind != 0 {
				decoded := InterfaceKind(item.kind)
				kind = &decoded
			}
			var failureDetail *string
			if item.has_failure_detail != 0 {
				decoded := string(copyStringView(item.failure_detail))
				failureDetail = &decoded
			}
			var rxBps *uint64
			if item.has_rx_bps != 0 {
				decoded := uint64(item.rx_bps)
				rxBps = &decoded
			}
			var txBps *uint64
			if item.has_tx_bps != 0 {
				decoded := uint64(item.tx_bps)
				txBps = &decoded
			}
			interfaces[index] = InterfaceSnapshot{
				InterfaceId:          interfaceID,
				Name:                 name,
				Kind:                 kind,
				Health:               InterfaceHealth(item.health),
				FailureDetail:        failureDetail,
				RxBytes:              uint64(item.rx_bytes),
				TxBytes:              uint64(item.tx_bytes),
				RxBps:                rxBps,
				TxBps:                txBps,
				RouteCount:           uint32(item.route_count),
				LinkCount:            uint32(item.link_count),
				TransportedLinkCount: uint32(item.transported_link_count),
			}
		}
	}
	routes := make([]RouteSnapshot, int(value.route_count))
	if len(routes) > 0 {
		native := unsafe.Slice(value.routes, len(routes))
		for index, item := range native {
			var destination DestinationHash
			copy(destination[:], copyFixed(item.destination, DestinationHashLength))
			var interfaceID InterfaceId
			copy(interfaceID[:], copyFixed(item.interface_id, InterfaceIdLength))
			var viaIdentity *IdentityHash
			if item.has_via_identity != 0 {
				decoded := IdentityHash{}
				copy(decoded[:], copyFixed(item.via_identity, IdentityHashLength))
				viaIdentity = &decoded
			}
			routes[index] = RouteSnapshot{
				Destination:         destination,
				Hops:                uint8(item.hops),
				ViaIdentity:         viaIdentity,
				InterfaceId:         interfaceID,
				LearnedAtMillis:     uint64(item.learned_at_millis),
				LastRelayedAtMillis: uint64(item.last_relayed_at_millis),
				ExpiresAtMillis:     uint64(item.expires_at_millis),
			}
		}
	}
	identities := make(
		[]DestinationIdentitySnapshot,
		int(value.destination_identity_count),
	)
	if len(identities) > 0 {
		native := unsafe.Slice(value.destination_identities, len(identities))
		for index, item := range native {
			copy(
				identities[index].Destination[:],
				copyFixed(item.destination, DestinationHashLength),
			)
			copy(
				identities[index].Identity[:],
				copyFixed(item.identity, IdentityHashLength),
			)
		}
	}
	persistence := value.persistence
	var flushCause *PersistenceFlushCause
	if persistence.has_last_flush_cause != 0 {
		decoded := PersistenceFlushCause(persistence.last_flush_cause)
		flushCause = &decoded
	}
	var failureDetail *string
	if persistence.has_last_failure_detail != 0 {
		decoded := string(copyStringView(persistence.last_failure_detail))
		failureDetail = &decoded
	}
	runtime := value.runtime
	return HostSnapshot{
		Revision:              uint64(value.revision),
		Backend:               copyBackendInfo(value.backend),
		Interfaces:            interfaces,
		Routes:                routes,
		ActiveLinkCount:       uint32(value.active_link_count),
		DestinationIdentities: identities,
		Runtime: RuntimeHealthSnapshot{
			Running:              runtime.running != 0,
			UptimeMillis:         uint64(runtime.uptime_millis),
			InterfaceCount:       uint32(runtime.interface_count),
			OnlineInterfaceCount: uint32(runtime.online_interface_count),
			RouteCount:           uint32(runtime.route_count),
			LinkCount:            uint32(runtime.link_count),
			TransportedLinkCount: uint32(runtime.transported_link_count),
			RxBytes:              uint64(runtime.rx_bytes),
			TxBytes:              uint64(runtime.tx_bytes),
			RxBps:                uint64(runtime.rx_bps),
			TxBps:                uint64(runtime.tx_bps),
		},
		Persistence: PersistenceSnapshot{
			Persistent:        persistence.persistent != 0,
			Restored:          persistence.restored != 0,
			LastFlushCause:    flushCause,
			LastFailureDetail: failureDetail,
		},
	}, status
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

func marshalResponseTimeout(
	value ResponseTimeout,
) (C.PrnsResponseTimeoutKind, C.uint64_t, error) {
	switch timeout := value.(type) {
	case ResponseTimeoutLinkDefault:
		return C.PRNS_RESPONSE_TIMEOUT_KIND_LINK_DEFAULT, 0, nil
	case ResponseTimeoutExact:
		return C.PRNS_RESPONSE_TIMEOUT_KIND_EXACT, C.uint64_t(timeout.Millis), nil
	default:
		return 0, 0, ConfigError{Kind: ConfigUnknownDestination, Field: "response timeout"}
	}
}

func marshalResourceCompression(
	value ResourceCompression,
) (C.PrnsResourceCompressionKind, error) {
	switch value.(type) {
	case ResourceCompressionAuto:
		return C.PRNS_RESOURCE_COMPRESSION_KIND_AUTO, nil
	case ResourceCompressionNever:
		return C.PRNS_RESOURCE_COMPRESSION_KIND_NEVER, nil
	default:
		return 0, ConfigError{Kind: ConfigUnknownDestination, Field: "resource compression"}
	}
}

func marshalResourceStrategy(
	value ResourceStrategy,
) (C.PrnsResourceStrategyKind, C.uint64_t, C.uint8_t, error) {
	switch strategy := value.(type) {
	case ResourceStrategyRefuse:
		return C.PRNS_RESOURCE_STRATEGY_KIND_REFUSE, 0, 0, nil
	case ResourceStrategyAccept:
		if strategy.MaximumUncompressedBytes == 0 {
			return 0, 0, 0, ConfigError{
				Kind:  ConfigUnknownDestination,
				Field: "maximum uncompressed resource bytes",
			}
		}
		var acceptCompressed C.uint8_t
		if strategy.AcceptCompressed {
			acceptCompressed = 1
		}
		return C.PRNS_RESOURCE_STRATEGY_KIND_ACCEPT,
			C.uint64_t(strategy.MaximumUncompressedBytes),
			acceptCompressed,
			nil
	default:
		return 0, 0, 0, ConfigError{Kind: ConfigUnknownDestination, Field: "resource strategy"}
	}
}

func ffiExecute(host nativeHost, value HostCommand) (nativeCommand, Status, error) {
	arena := nativeArena{}
	defer arena.close()
	var pointer *C.PrnsIssuedCommand
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
	case HostCommandAttachInterface:
		config, err := marshalInterface(&arena, command.Config)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_attach_interface(
			(*C.PrnsHost)(host.pointer),
			&config,
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
	case HostCommandEstablishLink:
		destination, err := arena.byteView(command.Destination[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_establish_link(
			(*C.PrnsHost)(host.pointer),
			destination,
			&pointer,
		))
	case HostCommandRequestPath:
		destination, err := arena.byteView(command.Destination[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_request_path(
			(*C.PrnsHost)(host.pointer),
			destination,
			&pointer,
		))
	case HostCommandIdentify:
		linkID, err := arena.byteView(command.LinkId[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		identity, err := arena.byteView(command.Identity[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_identify(
			(*C.PrnsHost)(host.pointer),
			linkID,
			identity,
			&pointer,
		))
	case HostCommandSendLinkPacket:
		linkID, err := arena.byteView(command.LinkId[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		payload, err := arena.byteView(command.Payload)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_send_link_packet(
			(*C.PrnsHost)(host.pointer),
			linkID,
			payload,
			&pointer,
		))
	case HostCommandRequest:
		linkID, err := arena.byteView(command.LinkId[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		pathHash, err := arena.byteView(command.PathHash[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		payload, err := arena.byteView(command.Payload)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		timeoutKind, timeoutMillis, err := marshalResponseTimeout(command.Timeout)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		var maximumResponseBytes C.uint64_t
		var maximumResponseBytesPointer *C.uint64_t
		if command.MaximumResponseBytes != nil {
			if *command.MaximumResponseBytes > SafeUintMax {
				return nativeCommand{}, StatusInvalidArgument, ConfigError{
					Kind:  ConfigInvalidLimits,
					Field: "maximum response bytes",
				}
			}
			maximumResponseBytes = C.uint64_t(*command.MaximumResponseBytes)
			maximumResponseBytesPointer = &maximumResponseBytes
		}
		status = Status(C.prns_host_request(
			(*C.PrnsHost)(host.pointer),
			linkID,
			pathHash,
			payload,
			timeoutKind,
			timeoutMillis,
			maximumResponseBytesPointer,
			&pointer,
		))
	case HostCommandRespond:
		linkID, err := arena.byteView(command.LinkId[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		requestID, err := arena.byteView(command.RequestId[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		payload, err := arena.byteView(command.Payload)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_respond(
			(*C.PrnsHost)(host.pointer),
			linkID,
			requestID,
			C.uint64_t(command.RequestRttMillis),
			payload,
			&pointer,
		))
	case HostCommandSendResource:
		linkID, err := arena.byteView(command.LinkId[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		payload, err := arena.byteView(command.Payload)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		var metadata *C.PrnsByteView
		if command.PackedMetadata != nil {
			view, err := arena.byteView(*command.PackedMetadata)
			if err != nil {
				return nativeCommand{}, StatusInvalidArgument, err
			}
			metadata = &view
		}
		compression, err := marshalResourceCompression(command.Compression)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_send_resource(
			(*C.PrnsHost)(host.pointer),
			linkID,
			payload,
			metadata,
			compression,
			&pointer,
		))
	case HostCommandSetLinkResourceStrategy:
		linkID, err := arena.byteView(command.LinkId[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		kind, maximum, compressed, err := marshalResourceStrategy(command.Strategy)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_set_link_resource_strategy(
			(*C.PrnsHost)(host.pointer),
			linkID,
			kind,
			maximum,
			compressed,
			&pointer,
		))
	case HostCommandSetDestinationResourceStrategy:
		destination, err := arena.byteView(command.Destination[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		kind, maximum, compressed, err := marshalResourceStrategy(command.Strategy)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_set_destination_resource_strategy(
			(*C.PrnsHost)(host.pointer),
			destination,
			kind,
			maximum,
			compressed,
			&pointer,
		))
	case HostCommandSendChannelMessage:
		linkID, err := arena.byteView(command.LinkId[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		payload, err := arena.byteView(command.Payload)
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_send_channel_message(
			(*C.PrnsHost)(host.pointer),
			linkID,
			C.uint16_t(command.MessageType),
			payload,
			&pointer,
		))
	case HostCommandAllowRequester:
		destination, err := arena.byteView(command.Destination[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		pathHash, err := arena.byteView(command.PathHash[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		identity, err := arena.byteView(command.Identity[:])
		if err != nil {
			return nativeCommand{}, StatusInvalidArgument, err
		}
		status = Status(C.prns_host_allow_requester(
			(*C.PrnsHost)(host.pointer),
			destination,
			pathHash,
			identity,
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
		(*C.PrnsIssuedCommand)(command.pointer),
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
	C.prns_command_interrupt_wait((*C.PrnsIssuedCommand)(command.pointer))
}

func ffiCommandClose(command nativeCommand) {
	C.prns_command_release((*C.PrnsIssuedCommand)(command.pointer))
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

func ffiBeginResourceUpload(
	host nativeHost,
	linkID LinkId,
	declaredLength uint64,
	packedMetadata *[]byte,
	compression ResourceCompression,
) (nativeResourceUpload, Status, error) {
	arena := nativeArena{}
	defer arena.close()
	link, err := arena.byteView(linkID[:])
	if err != nil {
		return nativeResourceUpload{}, StatusInvalidArgument, err
	}
	var metadata *C.PrnsByteView
	if packedMetadata != nil {
		view, viewError := arena.byteView(*packedMetadata)
		if viewError != nil {
			return nativeResourceUpload{}, StatusInvalidArgument, viewError
		}
		metadata = &view
	}
	kind, err := marshalResourceCompression(compression)
	if err != nil {
		return nativeResourceUpload{}, StatusInvalidArgument, err
	}
	var upload *C.PrnsResourceUpload
	status := Status(C.prns_host_begin_resource_upload(
		(*C.PrnsHost)(host.pointer),
		link,
		C.uint64_t(declaredLength),
		metadata,
		kind,
		&upload,
	))
	return nativeResourceUpload{pointer: unsafe.Pointer(upload)}, status, nil
}

func ffiResourceUploadWrite(upload nativeResourceUpload, chunk []byte) Status {
	arena := nativeArena{}
	defer arena.close()
	view, err := arena.byteView(chunk)
	if err != nil {
		return StatusInvalidArgument
	}
	return Status(C.prns_resource_upload_write(
		(*C.PrnsResourceUpload)(upload.pointer),
		view,
	))
}

func ffiResourceUploadFinish(upload nativeResourceUpload) (nativeCommand, Status) {
	var command *C.PrnsIssuedCommand
	status := Status(C.prns_resource_upload_finish(
		(*C.PrnsResourceUpload)(upload.pointer),
		&command,
	))
	return nativeCommand{pointer: unsafe.Pointer(command)}, status
}

func ffiResourceUploadAbort(upload nativeResourceUpload) {
	C.prns_resource_upload_abort((*C.PrnsResourceUpload)(upload.pointer))
}

func ffiResourceUploadClose(upload nativeResourceUpload) {
	C.prns_resource_upload_release((*C.PrnsResourceUpload)(upload.pointer))
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
