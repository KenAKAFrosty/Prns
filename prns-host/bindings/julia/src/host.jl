struct Limits
    pending_commands::Int
    application_events::Int
    retained_event_bytes::Int
    diagnostics::Int

    function Limits(
        pending_commands,
        application_events,
        retained_event_bytes,
        diagnostics,
    )
        values = (
            pending_commands,
            application_events,
            retained_event_bytes,
            diagnostics,
        )
        all(value -> value > 0, values) ||
            throw(ArgumentError("host limits must be positive"))
        new(values...)
    end
end

balanced_limits() = Limits(
    BALANCED_PENDING_COMMANDS,
    BALANCED_APPLICATION_EVENTS,
    BALANCED_RETAINED_EVENT_BYTES,
    BALANCED_DIAGNOSTICS,
)

struct HostOptions
    role::HostRole
    identity::IdentityConfig
    destinations::Vector{DestinationConfig}
    required_capabilities::Vector{Capability}
    limits::Limits
    persistence::PersistenceConfig
end

HostOptions(role, identity, destinations, required_capabilities, limits) =
    HostOptions(
        role,
        identity,
        destinations,
        required_capabilities,
        limits,
        PersistenceConfigEphemeral(),
    )

function ephemeral_endpoint(
    destinations::Vector{DestinationConfig}=DestinationConfig[];
    required_capabilities::Vector{Capability}=Capability[],
)
    HostOptions(
        HostRoleEndpoint,
        IdentityConfigGenerateEphemeral(),
        destinations,
        required_capabilities,
        balanced_limits(),
    )
end

function persistent_endpoint(
    root::AbstractString,
    destinations::Vector{DestinationConfig}=DestinationConfig[];
    required_capabilities::Vector{Capability}=Capability[],
)
    HostOptions(
        HostRoleEndpoint,
        IdentityConfigLoadOrCreate(joinpath(root, "identity")),
        destinations,
        required_capabilities,
        balanced_limits(),
        PersistenceConfigDirectory(joinpath(root, "state")),
    )
end

mutable struct Host
    pointer::Ptr{Cvoid}
    guard::ReentrantLock
    identity::IdentityHash
    destinations::Vector{DestinationHash}
end

function Host(options::HostOptions)
    verify_contract()
    arena = NativeArena()
    output = Ref{Ptr{Cvoid}}(C_NULL)
    try
        native_options, native_destinations, native_capabilities =
            native_host_options(arena, options)
        status = GC.@preserve arena native_destinations native_capabilities begin
            ccall(
                native_symbol(:prns_host_create),
                UInt32,
                (Ref{NativeHostOptions}, Ref{Ptr{Cvoid}}),
                native_options,
                output,
            )
        end
        checked_status(:create_host, status)
    finally
        close(arena)
    end
    host = Host(
        output[],
        ReentrantLock(),
        IdentityHash(zeros(UInt8, IDENTITY_HASH_LENGTH)),
        DestinationHash[],
    )
    try
        host.identity = read_identity_hash(host)
        host.destinations = read_destination_hashes(host)
    catch
        close(host)
        rethrow()
    end
    finalizer(close, host)
    host
end

function with_host_pointer(run, host::Host)
    lock(host.guard) do
        host.pointer == C_NULL && throw(StatusFailure(:host, StatusStopped))
        run(host.pointer)
    end
end

function read_identity_hash(host::Host)
    output = Ref(NativeByteView(C_NULL, 0))
    with_host_pointer(host) do pointer
        checked_status(
            :identity_hash,
            ccall(
                native_symbol(:prns_host_identity_hash),
                UInt32,
                (Ptr{Cvoid}, Ref{NativeByteView}),
                pointer,
                output,
            ),
        )
    end
    IdentityHash(copy_view(output[]))
end

function read_destination_hashes(host::Host)
    with_host_pointer(host) do pointer
        count = ccall(
            native_symbol(:prns_host_destination_count),
            Csize_t,
            (Ptr{Cvoid},),
            pointer,
        )
        map(0:Int(count)-1) do index
            output = Ref(NativeByteView(C_NULL, 0))
            checked_status(
                :destination_hash,
                ccall(
                    native_symbol(:prns_host_destination_hash),
                    UInt32,
                    (Ptr{Cvoid}, Csize_t, Ref{NativeByteView}),
                    pointer,
                    index,
                    output,
                ),
            )
            DestinationHash(copy_view(output[]))
        end
    end
end

identity_hash(host::Host) = host.identity
destination_hashes(host::Host) = copy(host.destinations)

function backend_info(host::Host)
    with_host_pointer(host) do _
        output = Ref(
            NativeBackendInfo(
                sizeof(NativeBackendInfo),
                0,
                C_NULL,
                0,
                C_NULL,
                0,
            ),
        )
        checked_status(
            :backend_info,
            ccall(
                native_symbol(:prns_backend_info),
                UInt32,
                (Ref{NativeBackendInfo},),
                output,
            ),
        )
        decode_backend_info(output[])
    end
end

function snapshot(host::Host; timeout_millis::UInt32=UInt32(5_000))
    with_host_pointer(host) do pointer
        inspection = Ref{Ptr{Cvoid}}(C_NULL)
        checked_status(
            :capture_snapshot,
            ccall(
                native_symbol(:prns_host_snapshot),
                UInt32,
                (Ptr{Cvoid}, UInt32, Ref{Ptr{Cvoid}}),
                pointer,
                timeout_millis,
                inspection,
            ),
        )
        try
            output = Ref(
                NativeHostSnapshot(
                    sizeof(NativeHostSnapshot),
                    0,
                    NativeBackendInfo(0, 0, C_NULL, 0, C_NULL, 0),
                    C_NULL,
                    0,
                    C_NULL,
                    0,
                    0,
                    C_NULL,
                    0,
                    NativeRuntimeHealthSnapshot(
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                    ),
                    NativePersistenceSnapshot(0, 0, 0, 0, 0, 0, NativeStringView(C_NULL, 0)),
                ),
            )
            checked_status(
                :read_snapshot,
                ccall(
                    native_symbol(:prns_host_snapshot_read),
                    UInt32,
                    (Ptr{Cvoid}, Ref{NativeHostSnapshot}),
                    inspection[],
                    output,
                ),
            )
            decode_host_snapshot(output[])
        finally
            ccall(
                native_symbol(:prns_host_snapshot_release),
                Cvoid,
                (Ptr{Cvoid},),
                inspection[],
            )
        end
    end
end

function stop!(host::Host)
    status = with_host_pointer(host) do pointer
        Status(
            ccall(
                native_symbol(:prns_host_stop),
                UInt32,
                (Ptr{Cvoid},),
                pointer,
            )
        )
    end
    status in (StatusOk, StatusStopped) ||
        throw(StatusFailure(:stop_host, status))
    nothing
end

function Base.close(host::Host)
    lock(host.guard) do
        host.pointer == C_NULL && return nothing
        ccall(
            native_symbol(:prns_host_release),
            Cvoid,
            (Ptr{Cvoid},),
            host.pointer,
        )
        host.pointer = C_NULL
    end
    nothing
end
