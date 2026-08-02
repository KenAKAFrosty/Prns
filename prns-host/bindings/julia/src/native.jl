const NEVER_TIMEOUT = typemax(UInt32)
const NATIVE_LIBRARY = Ref{Ptr{Cvoid}}(C_NULL)

function signal_async_condition(context::Ptr{Cvoid})::Cvoid
    ccall(:uv_async_send, Cint, (Ptr{Cvoid},), context)
    nothing
end

readiness_callback() =
    @cfunction(signal_async_condition, Cvoid, (Ptr{Cvoid},))

function register_readiness(pointer::Ptr{Cvoid}, symbol::Symbol)
    readiness = Base.AsyncCondition()
    registration = Ref{Ptr{Cvoid}}(C_NULL)
    status = Status(
        ccall(
            native_symbol(symbol),
            UInt32,
            (Ptr{Cvoid}, Ptr{Cvoid}, Ptr{Cvoid}, Ref{Ptr{Cvoid}}),
            pointer,
            readiness_callback(),
            Base.unsafe_convert(Ptr{Cvoid}, readiness),
            registration,
        ),
    )
    if status != StatusOk
        close(readiness)
        throw(StatusFailure(:register_readiness, status))
    end
    readiness, registration[]
end

function release_readiness(
    registration::Ptr{Cvoid},
    readiness::Base.AsyncCondition,
)
    ccall(
        native_symbol(:prns_readiness_registration_release),
        Cvoid,
        (Ptr{Cvoid},),
        registration,
    )
    close(readiness)
    nothing
end

struct NativeByteView
    data::Ptr{UInt8}
    length::Csize_t
end

const NativeStringView = NativeByteView

struct NativeContractInfo
    struct_size::Csize_t
    abi::UInt32
    schema_version::UInt32
    product_version::NativeStringView
end

struct NativeLimits
    struct_size::Csize_t
    pending_commands::Csize_t
    application_events::Csize_t
    retained_event_bytes::Csize_t
    diagnostics::Csize_t
end

struct NativeIdentityConfig
    struct_size::Csize_t
    kind::UInt32
    secret::NativeByteView
    path::NativeStringView
end

struct NativePersistenceConfig
    struct_size::Csize_t
    kind::UInt32
    path::NativeStringView
end

struct NativeDestinationName
    struct_size::Csize_t
    app_name::NativeStringView
    aspects::Ptr{NativeStringView}
    aspect_count::Csize_t
end

struct NativeRequestHandlerConfig
    struct_size::Csize_t
    path::NativeStringView
    policy::UInt32
end

struct NativeSerialLineConfig
    struct_size::Csize_t
    baud::UInt32
    data_bits::UInt32
    parity::UInt32
    stop_bits::UInt32
end

struct NativeRNodeRadioConfig
    struct_size::Csize_t
    frequency_hz::UInt64
    bandwidth_hz::UInt32
    tx_power_dbm::Int16
    spreading_factor::UInt8
    coding_rate::UInt8
end

struct NativeMultiRNodeMemberConfig
    struct_size::Csize_t
    name::NativeStringView
    virtual_port::UInt8
    radio::NativeRNodeRadioConfig
    flow_control::UInt8
    outgoing::UInt8
end

struct NativeInterfaceConfig
    struct_size::Csize_t
    kind::UInt32
    has_group_id::UInt8
    group_id::NativeStringView
    has_discovery_scope::UInt8
    discovery_scope::UInt32
    has_discovery_port::UInt8
    discovery_port::UInt16
    has_data_port::UInt8
    data_port::UInt16
    devices::Ptr{NativeStringView}
    device_count::Csize_t
    ignored_devices::Ptr{NativeStringView}
    ignored_device_count::Csize_t
    has_multicast_address_type::UInt8
    multicast_address_type::UInt32
    target::NativeStringView
    bind::NativeStringView
    var"local"::NativeStringView
    peer::NativeStringView
    bitrate_kind::UInt32
    bitrate_bps::UInt64
    port::NativeStringView
    line::NativeSerialLineConfig
    flow_control::UInt8
    preamble_millis::UInt32
    transmit_tail_millis::UInt32
    persistence::UInt8
    slot_time_millis::UInt32
    has_station_callsign::UInt8
    station_callsign::NativeStringView
    has_station_interval_seconds::UInt8
    station_interval_seconds::UInt64
    callsign::NativeStringView
    ssid::UInt8
    radio::NativeRNodeRadioConfig
    has_airtime_limit_short_centi_percent::UInt8
    airtime_limit_short_centi_percent::UInt16
    has_airtime_limit_long_centi_percent::UInt8
    airtime_limit_long_centi_percent::UInt16
    members::Ptr{NativeMultiRNodeMemberConfig}
    member_count::Csize_t
    command::Ptr{NativeStringView}
    command_count::Csize_t
    respawn_delay_millis::UInt64
    peers::Ptr{NativeStringView}
    peer_count::Csize_t
    connectable::UInt8
    url::NativeStringView
end

struct NativeBackendInfo
    struct_size::Csize_t
    backend::UInt32
    capabilities::Ptr{UInt32}
    capability_count::Csize_t
    interface_kinds::Ptr{UInt32}
    interface_kind_count::Csize_t
end

struct NativeInterfaceSnapshot
    struct_size::Csize_t
    interface_id::NativeByteView
    has_name::UInt8
    name::NativeStringView
    has_kind::UInt8
    kind::UInt32
    health::UInt32
    has_failure_detail::UInt8
    failure_detail::NativeStringView
    rx_bytes::UInt64
    tx_bytes::UInt64
    has_rx_bps::UInt8
    rx_bps::UInt64
    has_tx_bps::UInt8
    tx_bps::UInt64
    route_count::UInt32
    link_count::UInt32
    transported_link_count::UInt32
end

struct NativeRouteSnapshot
    struct_size::Csize_t
    destination::NativeByteView
    hops::UInt8
    has_via_identity::UInt8
    via_identity::NativeByteView
    interface_id::NativeByteView
    learned_at_millis::UInt64
    last_relayed_at_millis::UInt64
    expires_at_millis::UInt64
end

struct NativeDestinationIdentitySnapshot
    struct_size::Csize_t
    destination::NativeByteView
    identity::NativeByteView
end

struct NativeRuntimeHealthSnapshot
    struct_size::Csize_t
    running::UInt8
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

struct NativePersistenceSnapshot
    struct_size::Csize_t
    persistent::UInt8
    restored::UInt8
    has_last_flush_cause::UInt8
    last_flush_cause::UInt32
    has_last_failure_detail::UInt8
    last_failure_detail::NativeStringView
end

struct NativeHostSnapshot
    struct_size::Csize_t
    revision::UInt64
    backend::NativeBackendInfo
    interfaces::Ptr{NativeInterfaceSnapshot}
    interface_count::Csize_t
    routes::Ptr{NativeRouteSnapshot}
    route_count::Csize_t
    active_link_count::UInt32
    destination_identities::Ptr{NativeDestinationIdentitySnapshot}
    destination_identity_count::Csize_t
    runtime::NativeRuntimeHealthSnapshot
    persistence::NativePersistenceSnapshot
end

function NativeInterfaceConfig(;
    kind,
    group_id=nothing,
    discovery_scope=nothing,
    discovery_port=nothing,
    data_port=nothing,
    devices=Ptr{NativeStringView}(C_NULL),
    device_count=0,
    ignored_devices=Ptr{NativeStringView}(C_NULL),
    ignored_device_count=0,
    multicast_address_type=nothing,
    target=NativeStringView(C_NULL, 0),
    bind=NativeStringView(C_NULL, 0),
    local_address=NativeStringView(C_NULL, 0),
    peer=NativeStringView(C_NULL, 0),
    bitrate_kind=UInt32(0),
    bitrate_bps=UInt64(0),
    port=NativeStringView(C_NULL, 0),
    line=NativeSerialLineConfig(0, 0, 0, 0, 0),
    flow_control=false,
    preamble_millis=UInt32(0),
    transmit_tail_millis=UInt32(0),
    persistence=UInt8(0),
    slot_time_millis=UInt32(0),
    station_callsign=nothing,
    station_interval_seconds=nothing,
    callsign=NativeStringView(C_NULL, 0),
    ssid=UInt8(0),
    radio=NativeRNodeRadioConfig(0, 0, 0, 0, 0, 0),
    airtime_limit_short_centi_percent=nothing,
    airtime_limit_long_centi_percent=nothing,
    members=Ptr{NativeMultiRNodeMemberConfig}(C_NULL),
    member_count=0,
    command=Ptr{NativeStringView}(C_NULL),
    command_count=0,
    respawn_delay_millis=UInt64(0),
    peers=Ptr{NativeStringView}(C_NULL),
    peer_count=0,
    connectable=false,
    url=NativeStringView(C_NULL, 0),
)
    empty_string = NativeStringView(C_NULL, 0)
    NativeInterfaceConfig(
        sizeof(NativeInterfaceConfig),
        UInt32(kind),
        UInt8(group_id !== nothing),
        something(group_id, empty_string),
        UInt8(discovery_scope !== nothing),
        discovery_scope === nothing ? 0 : UInt32(discovery_scope),
        UInt8(discovery_port !== nothing),
        something(discovery_port, UInt16(0)),
        UInt8(data_port !== nothing),
        something(data_port, UInt16(0)),
        devices,
        device_count,
        ignored_devices,
        ignored_device_count,
        UInt8(multicast_address_type !== nothing),
        multicast_address_type === nothing ? 0 : UInt32(multicast_address_type),
        target,
        bind,
        local_address,
        peer,
        bitrate_kind,
        bitrate_bps,
        port,
        line,
        UInt8(flow_control),
        preamble_millis,
        transmit_tail_millis,
        persistence,
        slot_time_millis,
        UInt8(station_callsign !== nothing),
        something(station_callsign, empty_string),
        UInt8(station_interval_seconds !== nothing),
        something(station_interval_seconds, UInt64(0)),
        callsign,
        ssid,
        radio,
        UInt8(airtime_limit_short_centi_percent !== nothing),
        something(airtime_limit_short_centi_percent, UInt16(0)),
        UInt8(airtime_limit_long_centi_percent !== nothing),
        something(airtime_limit_long_centi_percent, UInt16(0)),
        members,
        member_count,
        command,
        command_count,
        respawn_delay_millis,
        peers,
        peer_count,
        UInt8(connectable),
        url,
    )
end

struct NativeDestinationConfig
    struct_size::Csize_t
    kind::UInt32
    name::NativeDestinationName
    identity_kind::UInt32
    dedicated_identity::NativeIdentityConfig
    announce_app_data::NativeByteView
    request_handlers::Ptr{NativeRequestHandlerConfig}
    request_handler_count::Csize_t
    has_maximum_request_bytes::UInt8
    maximum_request_bytes::UInt64
end

struct NativeHostOptions
    struct_size::Csize_t
    required_abi::UInt32
    required_schema_version::UInt32
    required_product_version::NativeStringView
    limits::NativeLimits
    role::UInt32
    identity::NativeIdentityConfig
    destinations::Ptr{NativeDestinationConfig}
    destination_count::Csize_t
    required_capabilities::Ptr{UInt32}
    required_capability_count::Csize_t
    persistence::NativePersistenceConfig
end

struct NativeCommandResult
    struct_size::Csize_t
    outcome::UInt32
    failure::UInt32
    evidence::UInt32
    rtt_millis::UInt64
    value::NativeByteView
    detail::NativeStringView
end

mutable struct NativeArena
    buffers::Vector{Vector{UInt8}}
    string_arrays::Vector{Vector{NativeStringView}}
    request_handler_arrays::Vector{Vector{NativeRequestHandlerConfig}}
    member_arrays::Vector{Vector{NativeMultiRNodeMemberConfig}}
end

NativeArena() = NativeArena(
    Vector{UInt8}[],
    Vector{NativeStringView}[],
    Vector{NativeRequestHandlerConfig}[],
    Vector{NativeMultiRNodeMemberConfig}[],
)

function Base.close(arena::NativeArena)
    foreach(buffer -> fill!(buffer, 0), arena.buffers)
    empty!(arena.buffers)
    empty!(arena.string_arrays)
    empty!(arena.request_handler_arrays)
    empty!(arena.member_arrays)
    nothing
end

function artifact_native_library()
    artifacts_toml = normpath(joinpath(@__DIR__, "..", "Artifacts.toml"))
    isfile(artifacts_toml) || return ""
    PkgArtifacts.ensure_artifact_installed(
        "personal_rns",
        artifacts_toml,
    )
    hash = Artifacts.artifact_hash("personal_rns", artifacts_toml)
    hash === nothing && return ""
    artifact = Artifacts.artifact_path(hash)
    roots = [artifact]
    append!(
        roots,
        filter(isdir, readdir(artifact; join=true)),
    )
    for root in roots
        for name in ("libprns_host.so", "libprns_host.dylib", "prns_host.dll")
            candidate = joinpath(root, "lib", name)
            isfile(candidate) && return candidate
        end
    end
    ""
end

function native_library()
    NATIVE_LIBRARY[] != C_NULL && return NATIVE_LIBRARY[]
    configured = get(ENV, "PRNS_HOST_LIBRARY", "")
    candidate = configured
    isempty(candidate) && (candidate = artifact_native_library())
    isempty(candidate) &&
        (candidate = Libdl.find_library(["prns_host", "libprns_host"]))
    isempty(candidate) && throw(NativeLibraryUnavailable())
    NATIVE_LIBRARY[] = Libdl.dlopen(candidate)
    NATIVE_LIBRARY[]
end

native_symbol(name::Symbol) = Libdl.dlsym(native_library(), name)

struct NativeLibraryUnavailable <: Exception end

Base.showerror(io::IO, ::NativeLibraryUnavailable) =
    print(io, "Personal RNS native library is unavailable")

struct StatusFailure <: Exception
    operation::Symbol
    status::Status
end

function Base.showerror(io::IO, failure::StatusFailure)
    print(io, "Personal RNS ", failure.operation, " failed with ", failure.status)
end

function checked_status(operation::Symbol, raw::UInt32)
    status = Status(raw)
    status == StatusOk || throw(StatusFailure(operation, status))
    status
end

function native_byte_view(arena::NativeArena, value)
    bytes = UInt8[item for item in value]
    push!(arena.buffers, bytes)
    data_pointer = isempty(bytes) ? Ptr{UInt8}(C_NULL) : pointer(bytes)
    NativeByteView(data_pointer, length(bytes))
end

function native_optional_byte_view(
    arena::NativeArena,
    value::Union{Nothing,AbstractVector{UInt8}},
)
    value === nothing && return NativeByteView(C_NULL, 0)
    bytes = isempty(value) ? UInt8[0] : Vector{UInt8}(value)
    push!(arena.buffers, bytes)
    NativeByteView(pointer(bytes), length(value))
end

function native_string_view(arena::NativeArena, value::AbstractString)
    native_byte_view(arena, Vector{UInt8}(codeunits(value)))
end

function copy_view(view::NativeByteView)
    view.length == 0 && return UInt8[]
    unsafe_wrap(Vector{UInt8}, view.data, Int(view.length)) |> copy
end

copy_string(view::NativeStringView) = String(copy_view(view))

function native_identity(arena::NativeArena, value::IdentityConfig)
    if value isa IdentityConfigExisting
        return NativeIdentityConfig(
            sizeof(NativeIdentityConfig),
            UInt32(IdentityConfigKindExisting),
            native_byte_view(arena, value.secret.bytes),
            NativeStringView(C_NULL, 0),
        )
    end
    if value isa IdentityConfigGenerateEphemeral
        return NativeIdentityConfig(
            sizeof(NativeIdentityConfig),
            UInt32(IdentityConfigKindGenerateEphemeral),
            NativeByteView(C_NULL, 0),
            NativeStringView(C_NULL, 0),
        )
    end
    if value isa IdentityConfigLoadOrCreate
        return NativeIdentityConfig(
            sizeof(NativeIdentityConfig),
            UInt32(IdentityConfigKindLoadOrCreate),
            NativeByteView(C_NULL, 0),
            native_string_view(arena, value.path),
        )
    end
    throw(ArgumentError("unknown identity configuration"))
end

function native_persistence(arena::NativeArena, value::PersistenceConfig)
    if value isa PersistenceConfigEphemeral
        return NativePersistenceConfig(
            sizeof(NativePersistenceConfig),
            UInt32(PersistenceConfigKindEphemeral),
            NativeStringView(C_NULL, 0),
        )
    end
    if value isa PersistenceConfigDirectory
        return NativePersistenceConfig(
            sizeof(NativePersistenceConfig),
            UInt32(PersistenceConfigKindDirectory),
            native_string_view(arena, value.path),
        )
    end
    throw(ArgumentError("unknown persistence configuration"))
end

function native_string_array(arena::NativeArena, values)
    array = NativeStringView[
        native_string_view(arena, value) for value in values
    ]
    push!(arena.string_arrays, array)
    isempty(array) ? Ptr{NativeStringView}(C_NULL) : pointer(array)
end

function native_serial_line(value::SerialLineConfig)
    NativeSerialLineConfig(
        sizeof(NativeSerialLineConfig),
        value.baud,
        UInt32(value.data_bits),
        UInt32(value.parity),
        UInt32(value.stop_bits),
    )
end

function native_radio(value::RNodeRadioConfig)
    NativeRNodeRadioConfig(
        sizeof(NativeRNodeRadioConfig),
        value.frequency_hz,
        value.bandwidth_hz,
        value.tx_power_dbm,
        value.spreading_factor,
        value.coding_rate,
    )
end

function native_interface(arena::NativeArena, value::InterfaceConfig)
    if value isa InterfaceConfigAutoLan
        devices = native_string_array(arena, value.devices)
        ignored = native_string_array(arena, value.ignored_devices)
        return NativeInterfaceConfig(
            kind=InterfaceKindAutoLan,
            group_id=value.group_id === nothing ? nothing :
                native_string_view(arena, value.group_id),
            discovery_scope=value.discovery_scope,
            discovery_port=value.discovery_port,
            data_port=value.data_port,
            devices=devices,
            device_count=length(value.devices),
            ignored_devices=ignored,
            ignored_device_count=length(value.ignored_devices),
            multicast_address_type=value.multicast_address_type,
        )
    end
    if value isa InterfaceConfigTcpClient
        kind, bits = native_bitrate(value.bitrate)
        return NativeInterfaceConfig(
            kind=InterfaceKindTcpClient,
            target=native_string_view(arena, value.target),
            bitrate_kind=kind,
            bitrate_bps=bits,
        )
    end
    if value isa InterfaceConfigTcpServer
        kind, bits = native_bitrate(value.bitrate)
        return NativeInterfaceConfig(
            kind=InterfaceKindTcpServer,
            bind=native_string_view(arena, value.bind),
            bitrate_kind=kind,
            bitrate_bps=bits,
        )
    end
    if value isa InterfaceConfigUdp
        kind, bits = native_bitrate(value.bitrate)
        return NativeInterfaceConfig(
            kind=InterfaceKindUdp,
            local_address=native_string_view(arena, getfield(value, :local)),
            peer=native_string_view(arena, value.peer),
            bitrate_kind=kind,
            bitrate_bps=bits,
        )
    end
    if value isa InterfaceConfigSerial
        return NativeInterfaceConfig(
            kind=InterfaceKindSerial,
            port=native_string_view(arena, value.port),
            line=native_serial_line(value.line),
        )
    end
    if value isa InterfaceConfigKiss
        return NativeInterfaceConfig(
            kind=InterfaceKindKiss,
            port=native_string_view(arena, value.port),
            line=native_serial_line(value.line),
            flow_control=value.flow_control,
            preamble_millis=value.preamble_millis,
            transmit_tail_millis=value.transmit_tail_millis,
            persistence=value.persistence,
            slot_time_millis=value.slot_time_millis,
            station_callsign=value.station_callsign === nothing ? nothing :
                native_string_view(arena, value.station_callsign),
            station_interval_seconds=value.station_interval_seconds,
        )
    end
    if value isa InterfaceConfigAx25Kiss
        return NativeInterfaceConfig(
            kind=InterfaceKindAx25Kiss,
            port=native_string_view(arena, value.port),
            line=native_serial_line(value.line),
            flow_control=value.flow_control,
            preamble_millis=value.preamble_millis,
            transmit_tail_millis=value.transmit_tail_millis,
            persistence=value.persistence,
            slot_time_millis=value.slot_time_millis,
            callsign=native_string_view(arena, value.callsign),
            ssid=value.ssid,
        )
    end
    if value isa InterfaceConfigRNode
        return NativeInterfaceConfig(
            kind=InterfaceKindRNode,
            port=native_string_view(arena, value.port),
            radio=native_radio(value.radio),
            flow_control=value.flow_control,
            station_callsign=value.station_callsign === nothing ? nothing :
                native_string_view(arena, value.station_callsign),
            station_interval_seconds=value.station_interval_seconds,
            airtime_limit_short_centi_percent=
                value.airtime_limit_short_centi_percent,
            airtime_limit_long_centi_percent=
                value.airtime_limit_long_centi_percent,
        )
    end
    if value isa InterfaceConfigMultiRNode
        members = NativeMultiRNodeMemberConfig[
            NativeMultiRNodeMemberConfig(
                sizeof(NativeMultiRNodeMemberConfig),
                native_string_view(arena, member.name),
                member.virtual_port,
                native_radio(member.radio),
                UInt8(member.flow_control),
                UInt8(member.outgoing),
            )
            for member in value.members
        ]
        push!(arena.member_arrays, members)
        return NativeInterfaceConfig(
            kind=InterfaceKindMultiRNode,
            port=native_string_view(arena, value.port),
            station_callsign=value.station_callsign === nothing ? nothing :
                native_string_view(arena, value.station_callsign),
            station_interval_seconds=value.station_interval_seconds,
            members=isempty(members) ? Ptr{NativeMultiRNodeMemberConfig}(C_NULL) :
                pointer(members),
            member_count=length(members),
        )
    end
    if value isa InterfaceConfigPipe
        command = native_string_array(arena, value.command)
        return NativeInterfaceConfig(
            kind=InterfaceKindPipe,
            command=command,
            command_count=length(value.command),
            respawn_delay_millis=value.respawn_delay_millis,
        )
    end
    if value isa InterfaceConfigBackboneClient
        kind, bits = native_bitrate(value.bitrate)
        return NativeInterfaceConfig(
            kind=InterfaceKindBackboneClient,
            target=native_string_view(arena, value.target),
            bitrate_kind=kind,
            bitrate_bps=bits,
        )
    end
    if value isa InterfaceConfigBackboneServer
        kind, bits = native_bitrate(value.bitrate)
        return NativeInterfaceConfig(
            kind=InterfaceKindBackboneServer,
            bind=native_string_view(arena, value.bind),
            bitrate_kind=kind,
            bitrate_bps=bits,
        )
    end
    if value isa InterfaceConfigI2p
        peers = native_string_array(arena, value.peers)
        return NativeInterfaceConfig(
            kind=InterfaceKindI2p,
            peers=peers,
            peer_count=length(value.peers),
            connectable=value.connectable,
        )
    end
    if value isa InterfaceConfigWeave
        return NativeInterfaceConfig(
            kind=InterfaceKindWeave,
            port=native_string_view(arena, value.port),
        )
    end
    if value isa InterfaceConfigAutomaticUsb
        return NativeInterfaceConfig(kind=InterfaceKindAutomaticUsb)
    end
    if value isa InterfaceConfigAutomaticBluetoothLe
        return NativeInterfaceConfig(kind=InterfaceKindAutomaticBluetoothLe)
    end
    if value isa InterfaceConfigWebSocketClient
        return NativeInterfaceConfig(
            kind=InterfaceKindWebSocketClient,
            target=native_string_view(arena, value.target),
        )
    end
    if value isa InterfaceConfigWebSocketServer
        return NativeInterfaceConfig(
            kind=InterfaceKindWebSocketServer,
            bind=native_string_view(arena, value.bind),
        )
    end
    if value isa InterfaceConfigBrowserRendezvous
        return NativeInterfaceConfig(
            kind=InterfaceKindBrowserRendezvous,
            url=native_string_view(arena, value.url),
        )
    end
    throw(ArgumentError("unknown interface configuration"))
end

function native_array(pointer::Ptr{Value}, count::Csize_t) where Value
    count == 0 && return Value[]
    copy(unsafe_wrap(Vector{Value}, pointer, Int(count)))
end

function decode_backend_info(value::NativeBackendInfo)
    BackendInfo(
        BackendKind(value.backend),
        Capability[
            Capability(item)
            for item in native_array(value.capabilities, value.capability_count)
        ],
        InterfaceKind[
            InterfaceKind(item)
            for item in native_array(value.interface_kinds, value.interface_kind_count)
        ],
    )
end

function decode_host_snapshot(value::NativeHostSnapshot)
    interfaces = InterfaceSnapshot[
        InterfaceSnapshot(
            InterfaceId(copy_view(item.interface_id)),
            item.has_name == 0 ? nothing : copy_string(item.name),
            item.has_kind == 0 ? nothing : InterfaceKind(item.kind),
            InterfaceHealth(item.health),
            item.has_failure_detail == 0 ? nothing :
                copy_string(item.failure_detail),
            item.rx_bytes,
            item.tx_bytes,
            item.has_rx_bps == 0 ? nothing : item.rx_bps,
            item.has_tx_bps == 0 ? nothing : item.tx_bps,
            item.route_count,
            item.link_count,
            item.transported_link_count,
        )
        for item in native_array(value.interfaces, value.interface_count)
    ]
    routes = RouteSnapshot[
        RouteSnapshot(
            DestinationHash(copy_view(item.destination)),
            item.hops,
            item.has_via_identity == 0 ? nothing :
                IdentityHash(copy_view(item.via_identity)),
            InterfaceId(copy_view(item.interface_id)),
            item.learned_at_millis,
            item.last_relayed_at_millis,
            item.expires_at_millis,
        )
        for item in native_array(value.routes, value.route_count)
    ]
    identities = DestinationIdentitySnapshot[
        DestinationIdentitySnapshot(
            DestinationHash(copy_view(item.destination)),
            IdentityHash(copy_view(item.identity)),
        )
        for item in native_array(
            value.destination_identities,
            value.destination_identity_count,
        )
    ]
    runtime = value.runtime
    persistence = value.persistence
    HostSnapshot(
        value.revision,
        decode_backend_info(value.backend),
        interfaces,
        routes,
        value.active_link_count,
        identities,
        RuntimeHealthSnapshot(
            runtime.running != 0,
            runtime.uptime_millis,
            runtime.interface_count,
            runtime.online_interface_count,
            runtime.route_count,
            runtime.link_count,
            runtime.transported_link_count,
            runtime.rx_bytes,
            runtime.tx_bytes,
            runtime.rx_bps,
            runtime.tx_bps,
        ),
        PersistenceSnapshot(
            persistence.persistent != 0,
            persistence.restored != 0,
            persistence.has_last_flush_cause == 0 ? nothing :
                PersistenceFlushCause(persistence.last_flush_cause),
            persistence.has_last_failure_detail == 0 ? nothing :
                copy_string(persistence.last_failure_detail),
        ),
    )
end

function native_destination_name(arena::NativeArena, value::DestinationName)
    aspects = NativeStringView[
        native_string_view(arena, aspect) for aspect in value.aspects
    ]
    push!(arena.string_arrays, aspects)
    NativeDestinationName(
        sizeof(NativeDestinationName),
        native_string_view(arena, value.app_name),
        isempty(aspects) ? C_NULL : pointer(aspects),
        length(aspects),
    )
end

function native_destination_identity(
    arena::NativeArena,
    value::DestinationIdentityConfig,
)
    if value isa DestinationIdentityConfigHostIdentity
        return (
            UInt32(DestinationIdentityConfigKindHostIdentity),
            NativeIdentityConfig(
                sizeof(NativeIdentityConfig),
                0,
                NativeByteView(C_NULL, 0),
                NativeStringView(C_NULL, 0),
            ),
        )
    end
    if value isa DestinationIdentityConfigDedicatedIdentity
        return (
            UInt32(DestinationIdentityConfigKindDedicatedIdentity),
            native_identity(arena, value.identity),
        )
    end
    throw(ArgumentError("unknown destination identity configuration"))
end

function native_destination(arena::NativeArena, value::DestinationConfig)
    if value isa DestinationConfigPlain
        return NativeDestinationConfig(
            sizeof(NativeDestinationConfig),
            UInt32(DestinationConfigKindPlain),
            native_destination_name(arena, value.name),
            0,
            NativeIdentityConfig(
                sizeof(NativeIdentityConfig),
                0,
                NativeByteView(C_NULL, 0),
                NativeStringView(C_NULL, 0),
            ),
            NativeByteView(C_NULL, 0),
            C_NULL,
            0,
            0,
            0,
        )
    end
    if value isa DestinationConfigSingle
        identity_kind, identity = native_destination_identity(
            arena,
            value.identity,
        )
        request_handlers = NativeRequestHandlerConfig[
            NativeRequestHandlerConfig(
                sizeof(NativeRequestHandlerConfig),
                native_string_view(arena, handler.path),
                UInt32(handler.policy),
            )
            for handler in value.request_handlers
        ]
        if !isnothing(value.maximum_request_bytes) &&
           value.maximum_request_bytes > SAFE_UINT_MAX
            throw(ArgumentError("maximum_request_bytes must be an unsigned safe integer"))
        end
        push!(arena.request_handler_arrays, request_handlers)
        return NativeDestinationConfig(
            sizeof(NativeDestinationConfig),
            UInt32(DestinationConfigKindSingle),
            native_destination_name(arena, value.name),
            identity_kind,
            identity,
            native_optional_byte_view(arena, value.announce_app_data),
            isempty(request_handlers) ? C_NULL : pointer(request_handlers),
            length(request_handlers),
            UInt8(!isnothing(value.maximum_request_bytes)),
            something(value.maximum_request_bytes, UInt64(0)),
        )
    end
    throw(ArgumentError("unknown destination configuration"))
end

function native_host_options(arena::NativeArena, value)
    destinations = NativeDestinationConfig[
        native_destination(arena, destination)
        for destination in value.destinations
    ]
    capabilities = UInt32[
        UInt32(capability) for capability in value.required_capabilities
    ]
    NativeHostOptions(
        sizeof(NativeHostOptions),
        HOST_CONTRACT_ABI,
        HOST_SCHEMA_VERSION,
        native_string_view(arena, PRODUCT_VERSION),
        NativeLimits(
            sizeof(NativeLimits),
            value.limits.pending_commands,
            value.limits.application_events,
            value.limits.retained_event_bytes,
            value.limits.diagnostics,
        ),
        UInt32(value.role),
        native_identity(arena, value.identity),
        isempty(destinations) ? C_NULL : pointer(destinations),
        length(destinations),
        isempty(capabilities) ? C_NULL : pointer(capabilities),
        length(capabilities),
        native_persistence(arena, value.persistence),
    ), destinations, capabilities
end

function verify_contract()
    output = Ref(
        NativeContractInfo(
            sizeof(NativeContractInfo),
            0,
            0,
            NativeStringView(C_NULL, 0),
        ),
    )
    checked_status(
        :contract_info,
        ccall(
            native_symbol(:prns_contract_info),
            UInt32,
            (Ref{NativeContractInfo},),
            output,
        ),
    )
    actual = output[]
    actual.abi == HOST_CONTRACT_ABI ||
        throw(StatusFailure(:contract_info, StatusContractMismatch))
    actual.schema_version == HOST_SCHEMA_VERSION ||
        throw(StatusFailure(:contract_info, StatusContractMismatch))
    copy_string(actual.product_version) == PRODUCT_VERSION ||
        throw(StatusFailure(:contract_info, StatusContractMismatch))
    nothing
end
