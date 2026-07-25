const NEVER_TIMEOUT = typemax(UInt32)
const EVENT_WAIT_SLICE_MILLIS = UInt32(50)
const NATIVE_LIBRARY = Ref{Ptr{Cvoid}}(C_NULL)

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

struct NativeDestinationConfig
    struct_size::Csize_t
    kind::UInt32
    name::NativeDestinationName
    identity_kind::UInt32
    dedicated_identity::NativeIdentityConfig
    announce_app_data::NativeByteView
    request_handlers::Ptr{NativeRequestHandlerConfig}
    request_handler_count::Csize_t
end

struct NativeHostOptions
    struct_size::Csize_t
    required_abi::UInt32
    required_product_version::NativeStringView
    limits::NativeLimits
    role::UInt32
    identity::NativeIdentityConfig
    destinations::Ptr{NativeDestinationConfig}
    destination_count::Csize_t
    required_capabilities::Ptr{UInt32}
    required_capability_count::Csize_t
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
end

NativeArena() = NativeArena(
    Vector{UInt8}[],
    Vector{NativeStringView}[],
    Vector{NativeRequestHandlerConfig}[],
)

function Base.close(arena::NativeArena)
    foreach(buffer -> fill!(buffer, 0), arena.buffers)
    empty!(arena.buffers)
    empty!(arena.string_arrays)
    empty!(arena.request_handler_arrays)
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
