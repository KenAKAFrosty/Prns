abstract type CommandSettlement end

struct CommandSucceeded <: CommandSettlement
    outcome::CommandOutcome
end

struct CommandFailed <: CommandSettlement
    failure::CommandFailure
end

mutable struct Command
    pointer::Ptr{Cvoid}
    readiness::Base.AsyncCondition
    registration::Ptr{Cvoid}
    guard::ReentrantLock
    wait_guard::ReentrantLock
end

mutable struct ResourceUpload
    pointer::Ptr{Cvoid}
    guard::ReentrantLock
    finished::Bool
end

function begin_resource_upload(
    host::Host,
    link_id::LinkId,
    declared_length::UInt64;
    packed_metadata::Union{Nothing,Vector{UInt8}}=nothing,
    compression::ResourceCompression=ResourceCompressionAuto(),
)
    arena = NativeArena()
    try
        link = native_byte_view(arena, link_id.bytes)
        metadata = packed_metadata === nothing ? nothing :
            Ref(native_byte_view(arena, packed_metadata))
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena metadata begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_begin_resource_upload),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeByteView,
                        UInt64,
                        Ptr{NativeByteView},
                        UInt32,
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    link,
                    declared_length,
                    metadata === nothing ? C_NULL : metadata,
                    native_resource_compression(compression),
                    output,
                )
            end
        end
        checked_status(:begin_resource_upload, status)
        upload = ResourceUpload(output[], ReentrantLock(), false)
        finalizer(close, upload)
        upload
    finally
        close(arena)
    end
end

function write!(upload::ResourceUpload, chunk::AbstractVector{UInt8})
    while true
        status = lock(upload.guard) do
            upload.pointer == C_NULL &&
                throw(StatusFailure(:resource_upload, StatusStopped))
            upload.finished &&
                throw(StatusFailure(:resource_upload, StatusStopped))
            arena = NativeArena()
            try
                Status(
                    ccall(
                        native_symbol(:prns_resource_upload_write),
                        UInt32,
                        (Ptr{Cvoid}, NativeByteView),
                        upload.pointer,
                        native_byte_view(arena, chunk),
                    ),
                )
            finally
                close(arena)
            end
        end
        status == StatusOk && return nothing
        status == StatusWouldBlock ||
            throw(StatusFailure(:write_resource_upload, status))
        yield()
    end
end

function finish!(upload::ResourceUpload)
    lock(upload.guard) do
        upload.pointer == C_NULL &&
            throw(StatusFailure(:resource_upload, StatusStopped))
        upload.finished &&
            throw(StatusFailure(:resource_upload, StatusStopped))
        output = Ref{Ptr{Cvoid}}(C_NULL)
        checked_status(
            :finish_resource_upload,
            ccall(
                native_symbol(:prns_resource_upload_finish),
                UInt32,
                (Ptr{Cvoid}, Ref{Ptr{Cvoid}}),
                upload.pointer,
                output,
            ),
        )
        upload.finished = true
        Command(output[])
    end
end

function abort!(upload::ResourceUpload)
    lock(upload.guard) do
        if upload.pointer != C_NULL && !upload.finished
            ccall(
                native_symbol(:prns_resource_upload_abort),
                Cvoid,
                (Ptr{Cvoid},),
                upload.pointer,
            )
            upload.finished = true
        end
    end
    nothing
end

function Base.close(upload::ResourceUpload)
    lock(upload.guard) do
        upload.pointer == C_NULL && return nothing
        if !upload.finished
            ccall(
                native_symbol(:prns_resource_upload_abort),
                Cvoid,
                (Ptr{Cvoid},),
                upload.pointer,
            )
        end
        ccall(
            native_symbol(:prns_resource_upload_release),
            Cvoid,
            (Ptr{Cvoid},),
            upload.pointer,
        )
        upload.pointer = C_NULL
    end
    nothing
end

function send_resource(
    host::Host,
    link_id::LinkId,
    payload::AbstractVector{UInt8};
    packed_metadata::Union{Nothing,Vector{UInt8}}=nothing,
    compression::ResourceCompression=ResourceCompressionAuto(),
)
    upload = begin_resource_upload(
        host,
        link_id,
        UInt64(length(payload));
        packed_metadata=packed_metadata,
        compression=compression,
    )
    try
        write!(upload, payload)
        command = finish!(upload)
        try
            wait(command)
        finally
            close(command)
        end
    catch
        abort!(upload)
        rethrow()
    finally
        close(upload)
    end
end

function Command(pointer::Ptr{Cvoid})
    readiness, registration = try
        register_readiness(pointer, :prns_command_register_readiness)
    catch
        ccall(
            native_symbol(:prns_command_release),
            Cvoid,
            (Ptr{Cvoid},),
            pointer,
        )
        rethrow()
    end
    command = Command(
        pointer,
        readiness,
        registration,
        ReentrantLock(),
        ReentrantLock(),
    )
    finalizer(close, command)
    command
end

function command_pointer(command::Command)
    lock(command.guard) do
        command.pointer == C_NULL &&
            throw(StatusFailure(:command, StatusStopped))
        command.pointer
    end
end

function submitted_command(status::UInt32, output::Ref{Ptr{Cvoid}})
    checked_status(:submit_command, status)
    Command(output[])
end

function execute(host::Host, value::HostCommandAnnounce)
    arena = NativeArena()
    try
        destination = native_byte_view(arena, value.destination.bytes)
        native_interface = value.interface === nothing ?
            nothing :
            Ref(native_byte_view(arena, value.interface.bytes))
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena native_interface begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_announce),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeByteView,
                        Ptr{NativeByteView},
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    destination,
                    native_interface === nothing ? C_NULL : native_interface,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function execute(host::Host, value::HostCommandSendSinglePacket)
    arena = NativeArena()
    try
        destination = native_byte_view(arena, value.destination.bytes)
        payload = native_byte_view(arena, value.payload)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_send_single_packet),
                    UInt32,
                    (Ptr{Cvoid}, NativeByteView, NativeByteView, Ref{Ptr{Cvoid}}),
                    pointer,
                    destination,
                    payload,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function execute(host::Host, value::HostCommandCloseLink)
    arena = NativeArena()
    try
        link = native_byte_view(arena, value.link_id.bytes)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_close_link),
                    UInt32,
                    (Ptr{Cvoid}, NativeByteView, Ref{Ptr{Cvoid}}),
                    pointer,
                    link,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function native_bitrate(value::Bitrate)
    value isa BitrateAuto && return UInt32(BitrateKindAuto), UInt64(0)
    value isa BitrateBitsPerSecond &&
        return UInt32(BitrateKindBitsPerSecond), value.value
    throw(ArgumentError("unknown bitrate"))
end

function execute_tcp(
    symbol::Symbol,
    host::Host,
    address::String,
    bitrate::Bitrate,
)
    arena = NativeArena()
    try
        native_address = native_string_view(arena, address)
        kind, bits = native_bitrate(bitrate)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(symbol),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeStringView,
                        UInt16,
                        UInt64,
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    native_address,
                    kind,
                    bits,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

execute(host::Host, value::HostCommandAttachTcpServer) = execute_tcp(
    :prns_host_attach_tcp_server,
    host,
    value.bind,
    value.bitrate,
)

execute(host::Host, value::HostCommandAttachTcpClient) = execute_tcp(
    :prns_host_attach_tcp_client,
    host,
    value.target,
    value.bitrate,
)

function execute(host::Host, value::HostCommandAttachUdp)
    arena = NativeArena()
    try
        local_address = native_string_view(arena, getfield(value, :local))
        peer_address = native_string_view(arena, value.peer)
        kind, bits = native_bitrate(value.bitrate)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_attach_udp),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeStringView,
                        NativeStringView,
                        UInt32,
                        UInt64,
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    local_address,
                    peer_address,
                    kind,
                    bits,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function execute(host::Host, value::HostCommandAttachInterface)
    arena = NativeArena()
    try
        config = Ref(native_interface(arena, value.config))
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena config begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_attach_interface),
                    UInt32,
                    (Ptr{Cvoid}, Ref{NativeInterfaceConfig}, Ref{Ptr{Cvoid}}),
                    pointer,
                    config,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function execute(host::Host, value::HostCommandDetachInterface)
    arena = NativeArena()
    try
        interface_id = native_byte_view(arena, value.interface.bytes)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_detach_interface),
                    UInt32,
                    (Ptr{Cvoid}, NativeByteView, Ref{Ptr{Cvoid}}),
                    pointer,
                    interface_id,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function execute_destination_command(symbol::Symbol, host::Host, destination)
    arena = NativeArena()
    try
        native_destination = native_byte_view(arena, destination.bytes)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(symbol),
                    UInt32,
                    (Ptr{Cvoid}, NativeByteView, Ref{Ptr{Cvoid}}),
                    pointer,
                    native_destination,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

execute(host::Host, value::HostCommandEstablishLink) =
    execute_destination_command(
        :prns_host_establish_link,
        host,
        value.destination,
    )

execute(host::Host, value::HostCommandRequestPath) =
    execute_destination_command(
        :prns_host_request_path,
        host,
        value.destination,
    )

function execute(host::Host, value::HostCommandIdentify)
    arena = NativeArena()
    try
        link = native_byte_view(arena, value.link_id.bytes)
        identity = native_byte_view(arena, value.identity.bytes)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_identify),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeByteView,
                        NativeByteView,
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    link,
                    identity,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function execute(host::Host, value::HostCommandSendLinkPacket)
    arena = NativeArena()
    try
        link = native_byte_view(arena, value.link_id.bytes)
        payload = native_byte_view(arena, value.payload)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_send_link_packet),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeByteView,
                        NativeByteView,
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    link,
                    payload,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function native_response_timeout(value::ResponseTimeout)
    value isa ResponseTimeoutLinkDefault &&
        return UInt32(ResponseTimeoutKindLinkDefault), UInt64(0)
    value isa ResponseTimeoutExact &&
        return UInt32(ResponseTimeoutKindExact), value.millis
    throw(ArgumentError("unknown response timeout"))
end

function execute(host::Host, value::HostCommandRequest)
    arena = NativeArena()
    try
        link = native_byte_view(arena, value.link_id.bytes)
        path_hash = native_byte_view(arena, value.path_hash.bytes)
        payload = native_byte_view(arena, value.payload)
        timeout_kind, timeout_millis = native_response_timeout(value.timeout)
        if !isnothing(value.maximum_response_bytes) &&
           value.maximum_response_bytes > SAFE_UINT_MAX
            throw(ArgumentError("maximum_response_bytes must be an unsigned safe integer"))
        end
        maximum_response_bytes = isnothing(value.maximum_response_bytes) ?
            Ptr{UInt64}(C_NULL) : Ref(value.maximum_response_bytes)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena maximum_response_bytes begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_request),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeByteView,
                        NativeByteView,
                        NativeByteView,
                        UInt32,
                        UInt64,
                        Ptr{UInt64},
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    link,
                    path_hash,
                    payload,
                    timeout_kind,
                    timeout_millis,
                    maximum_response_bytes,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function execute(host::Host, value::HostCommandRespond)
    arena = NativeArena()
    try
        link = native_byte_view(arena, value.link_id.bytes)
        request_id = native_byte_view(arena, value.request_id.bytes)
        payload = native_byte_view(arena, value.payload)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_respond),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeByteView,
                        NativeByteView,
                        UInt64,
                        NativeByteView,
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    link,
                    request_id,
                    value.request_rtt_millis,
                    payload,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function native_resource_compression(value::ResourceCompression)
    value isa ResourceCompressionAuto &&
        return UInt32(ResourceCompressionKindAuto)
    value isa ResourceCompressionNever &&
        return UInt32(ResourceCompressionKindNever)
    throw(ArgumentError("unknown resource compression"))
end

function execute(host::Host, value::HostCommandSendResource)
    arena = NativeArena()
    try
        link = native_byte_view(arena, value.link_id.bytes)
        payload = native_byte_view(arena, value.payload)
        metadata = value.packed_metadata === nothing ?
            nothing :
            Ref(native_byte_view(arena, value.packed_metadata))
        compression = native_resource_compression(value.compression)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena metadata begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_send_resource),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeByteView,
                        NativeByteView,
                        Ptr{NativeByteView},
                        UInt32,
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    link,
                    payload,
                    metadata === nothing ? C_NULL : metadata,
                    compression,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function native_resource_strategy(value::ResourceStrategy)
    value isa ResourceStrategyRefuse &&
        return UInt32(ResourceStrategyKindRefuse), UInt64(0), UInt8(0)
    value isa ResourceStrategyAccept &&
        return (
            UInt32(ResourceStrategyKindAccept),
            value.maximum_uncompressed_bytes,
            UInt8(value.accept_compressed),
        )
    throw(ArgumentError("unknown resource strategy"))
end

function execute_resource_strategy(
    symbol::Symbol,
    host::Host,
    target,
    strategy::ResourceStrategy,
)
    arena = NativeArena()
    try
        native_target = native_byte_view(arena, target.bytes)
        strategy_kind, maximum_bytes, accept_compressed =
            native_resource_strategy(strategy)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(symbol),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeByteView,
                        UInt32,
                        UInt64,
                        UInt8,
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    native_target,
                    strategy_kind,
                    maximum_bytes,
                    accept_compressed,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

execute(host::Host, value::HostCommandSetLinkResourceStrategy) =
    execute_resource_strategy(
        :prns_host_set_link_resource_strategy,
        host,
        value.link_id,
        value.strategy,
    )

execute(host::Host, value::HostCommandSetDestinationResourceStrategy) =
    execute_resource_strategy(
        :prns_host_set_destination_resource_strategy,
        host,
        value.destination,
        value.strategy,
    )

function execute(host::Host, value::HostCommandSendChannelMessage)
    arena = NativeArena()
    try
        link = native_byte_view(arena, value.link_id.bytes)
        payload = native_byte_view(arena, value.payload)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_send_channel_message),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeByteView,
                        UInt32,
                        NativeByteView,
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    link,
                    value.message_type,
                    payload,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function execute(host::Host, value::HostCommandAllowRequester)
    arena = NativeArena()
    try
        destination = native_byte_view(arena, value.destination.bytes)
        path_hash = native_byte_view(arena, value.path_hash.bytes)
        identity = native_byte_view(arena, value.identity.bytes)
        output = Ref{Ptr{Cvoid}}(C_NULL)
        status = GC.@preserve arena begin
            with_host_pointer(host) do pointer
                ccall(
                    native_symbol(:prns_host_allow_requester),
                    UInt32,
                    (
                        Ptr{Cvoid},
                        NativeByteView,
                        NativeByteView,
                        NativeByteView,
                        Ref{Ptr{Cvoid}},
                    ),
                    pointer,
                    destination,
                    path_hash,
                    identity,
                    output,
                )
            end
        end
        submitted_command(status, output)
    finally
        close(arena)
    end
end

function execute_settled(host::Host, value::HostCommand)
    command = execute(host, value)
    try
        wait(command)
    finally
        close(command)
    end
end

function announce(
    host::Host,
    destination::DestinationHash;
    interface::Union{Nothing,InterfaceId}=nothing,
)
    execute_settled(host, HostCommandAnnounce(destination, interface))
end

function send_single_packet(
    host::Host,
    destination::DestinationHash,
    payload::AbstractVector{UInt8},
)
    execute_settled(
        host,
        HostCommandSendSinglePacket(destination, Vector{UInt8}(payload)),
    )
end

close_link(host::Host, link_id::LinkId) =
    execute_settled(host, HostCommandCloseLink(link_id))

function attach_tcp_server(
    host::Host,
    bind::String;
    bitrate::Bitrate=BitrateAuto(),
)
    execute_settled(host, HostCommandAttachTcpServer(bind, bitrate))
end

function attach_tcp_client(
    host::Host,
    target::String;
    bitrate::Bitrate=BitrateAuto(),
)
    execute_settled(host, HostCommandAttachTcpClient(target, bitrate))
end

function attach_udp(
    host::Host,
    local_address::String,
    peer::String;
    bitrate::Bitrate=BitrateAuto(),
)
    execute_settled(
        host,
        HostCommandAttachUdp(local_address, peer, bitrate),
    )
end

attach_interface(host::Host, config::InterfaceConfig) =
    execute_settled(host, HostCommandAttachInterface(config))

detach_interface(host::Host, interface::InterfaceId) =
    execute_settled(host, HostCommandDetachInterface(interface))

establish_link(host::Host, destination::DestinationHash) =
    execute_settled(host, HostCommandEstablishLink(destination))

request_path(host::Host, destination::DestinationHash) =
    execute_settled(host, HostCommandRequestPath(destination))

identify(host::Host, link_id::LinkId, identity::IdentityHash) =
    execute_settled(host, HostCommandIdentify(link_id, identity))

function send_link_packet(
    host::Host,
    link_id::LinkId,
    payload::AbstractVector{UInt8},
)
    execute_settled(
        host,
        HostCommandSendLinkPacket(link_id, Vector{UInt8}(payload)),
    )
end

function request(
    host::Host,
    link_id::LinkId,
    path_hash::RequestPathHash,
    payload::AbstractVector{UInt8};
    timeout::ResponseTimeout=ResponseTimeoutLinkDefault(),
    maximum_response_bytes::Union{Nothing,UInt64}=nothing,
)
    execute_settled(
        host,
        HostCommandRequest(
            link_id,
            path_hash,
            Vector{UInt8}(payload),
            timeout,
            maximum_response_bytes,
        ),
    )
end

function respond(
    host::Host,
    link_id::LinkId,
    request_id::RequestId,
    request_rtt_millis::UInt64,
    payload::AbstractVector{UInt8},
)
    execute_settled(
        host,
        HostCommandRespond(
            link_id,
            request_id,
            request_rtt_millis,
            Vector{UInt8}(payload),
        ),
    )
end

set_link_resource_strategy(
    host::Host,
    link_id::LinkId,
    strategy::ResourceStrategy,
) = execute_settled(
    host,
    HostCommandSetLinkResourceStrategy(link_id, strategy),
)

set_destination_resource_strategy(
    host::Host,
    destination::DestinationHash,
    strategy::ResourceStrategy,
) = execute_settled(
    host,
    HostCommandSetDestinationResourceStrategy(destination, strategy),
)

function send_channel_message(
    host::Host,
    link_id::LinkId,
    message_type::UInt16,
    payload::AbstractVector{UInt8},
)
    execute_settled(
        host,
        HostCommandSendChannelMessage(
            link_id,
            message_type,
            Vector{UInt8}(payload),
        ),
    )
end

function allow_requester(
    host::Host,
    destination::DestinationHash,
    path_hash::RequestPathHash,
    identity::IdentityHash,
)
    execute_settled(
        host,
        HostCommandAllowRequester(destination, path_hash, identity),
    )
end

function decode_settlement(value::NativeCommandResult)
    value.failure != 0 && return CommandFailed(
        decode_command_failure(
            CommandFailureKind(value.failure),
            copy_string(value.detail),
        ),
    )
    outcome = CommandOutcomeKind(value.outcome)
    if outcome == CommandOutcomeKindAnnounced
        return CommandSucceeded(CommandOutcomeAnnounced())
    end
    if outcome == CommandOutcomeKindPacketDelivered
        bytes = copy_view(value.value)
        evidence = DeliveryEvidenceKind(value.evidence)
        packet_hash = if evidence == DeliveryEvidenceKindResponse
            isempty(bytes) ||
                throw(StatusFailure(:decode_response_evidence, StatusBackendFailed))
            nothing
        else
            PacketHash(bytes)
        end
        return CommandSucceeded(
            CommandOutcomePacketDelivered(
                value.rtt_millis,
                evidence,
                packet_hash,
            ),
        )
    end
    if outcome == CommandOutcomeKindLinkCloseQueued
        return CommandSucceeded(CommandOutcomeLinkCloseQueued())
    end
    if outcome == CommandOutcomeKindInterfaceAttached
        return CommandSucceeded(
            CommandOutcomeInterfaceAttached(InterfaceId(copy_view(value.value))),
        )
    end
    if outcome == CommandOutcomeKindInterfaceDetached
        return CommandSucceeded(
            CommandOutcomeInterfaceDetached(InterfaceId(copy_view(value.value))),
        )
    end
    if outcome == CommandOutcomeKindLinkEstablished
        return CommandSucceeded(
            CommandOutcomeLinkEstablished(
                LinkId(copy_view(value.value)),
                value.rtt_millis,
            ),
        )
    end
    if outcome == CommandOutcomeKindPathDiscovered
        bytes = copy_view(value.value)
        length(bytes) == 1 ||
            throw(StatusFailure(:decode_path_hops, StatusBackendFailed))
        return CommandSucceeded(CommandOutcomePathDiscovered(bytes[1]))
    end
    if outcome == CommandOutcomeKindIdentified
        return CommandSucceeded(CommandOutcomeIdentified())
    end
    if outcome == CommandOutcomeKindResponseReceived
        return CommandSucceeded(
            CommandOutcomeResponseReceived(
                copy_view(value.value),
                value.rtt_millis,
            ),
        )
    end
    if outcome == CommandOutcomeKindResponseSent
        return CommandSucceeded(CommandOutcomeResponseSent(value.rtt_millis))
    end
    if outcome == CommandOutcomeKindResourceSent
        return CommandSucceeded(CommandOutcomeResourceSent())
    end
    if outcome == CommandOutcomeKindResourceStrategySet
        return CommandSucceeded(CommandOutcomeResourceStrategySet())
    end
    if outcome == CommandOutcomeKindRequesterAllowed
        return CommandSucceeded(CommandOutcomeRequesterAllowed())
    end
    throw(StatusFailure(:decode_command, StatusBackendFailed))
end

function decode_command_failure(kind::CommandFailureKind, detail::String)
    kind == CommandFailureKindNodeStopped && return CommandFailureNodeStopped()
    kind == CommandFailureKindBusy && return CommandFailureBusy()
    kind == CommandFailureKindPayloadTooLarge &&
        return CommandFailurePayloadTooLarge()
    kind == CommandFailureKindUnknownDestination &&
        return CommandFailureUnknownDestination()
    kind == CommandFailureKindNotSingleDestination &&
        return CommandFailureNotSingleDestination()
    kind == CommandFailureKindAnnounceAppDataTooLong &&
        return CommandFailureAnnounceAppDataTooLong()
    kind == CommandFailureKindUnknownInterface &&
        return CommandFailureUnknownInterface()
    kind == CommandFailureKindNoRouteToDestination &&
        return CommandFailureNoRouteToDestination()
    kind == CommandFailureKindNotDirectlyReachable &&
        return CommandFailureNotDirectlyReachable()
    kind == CommandFailureKindPacketCulled && return CommandFailurePacketCulled()
    kind == CommandFailureKindDeliveryTimedOut &&
        return CommandFailureDeliveryTimedOut()
    kind == CommandFailureKindInvalidBitrate &&
        return CommandFailureInvalidBitrate()
    kind == CommandFailureKindBindFailed &&
        return CommandFailureBindFailed(detail)
    kind == CommandFailureKindWriteFailed &&
        return CommandFailureWriteFailed(detail)
    kind == CommandFailureKindUnsupportedByBackend &&
        return CommandFailureUnsupportedByBackend()
    kind == CommandFailureKindUnknownLink && return CommandFailureUnknownLink()
    kind == CommandFailureKindLinkNotActive &&
        return CommandFailureLinkNotActive()
    kind == CommandFailureKindEntropyUnavailable &&
        return CommandFailureEntropyUnavailable()
    kind == CommandFailureKindNotLinkInitiator &&
        return CommandFailureNotLinkInitiator()
    kind == CommandFailureKindIdentityNotHeld &&
        return CommandFailureIdentityNotHeld()
    kind == CommandFailureKindUnknownRequestHandler &&
        return CommandFailureUnknownRequestHandler()
    kind == CommandFailureKindRequestPolicyNotAllowList &&
        return CommandFailureRequestPolicyNotAllowList()
    kind == CommandFailureKindRequestAllowListFull &&
        return CommandFailureRequestAllowListFull()
    kind == CommandFailureKindLinkBusy && return CommandFailureLinkBusy()
    kind == CommandFailureKindResourceTableFull &&
        return CommandFailureResourceTableFull()
    kind == CommandFailureKindResourceMetadataTooLarge &&
        return CommandFailureResourceMetadataTooLarge()
    kind == CommandFailureKindResourceRejectedByPeer &&
        return CommandFailureResourceRejectedByPeer()
    kind == CommandFailureKindResourceSequencingFailed &&
        return CommandFailureResourceSequencingFailed()
    kind == CommandFailureKindResourcePredecessorFailed &&
        return CommandFailureResourcePredecessorFailed()
    kind == CommandFailureKindChannelWindowFull &&
        return CommandFailureChannelWindowFull()
    kind == CommandFailureKindChannelUntrackable &&
        return CommandFailureChannelUntrackable()
    kind == CommandFailureKindInvalidChannelMessageType &&
        return CommandFailureInvalidChannelMessageType()
    kind == CommandFailureKindInvalidConfiguration &&
        return CommandFailureInvalidConfiguration(detail)
    kind == CommandFailureKindResourceUploadCancelled &&
        return CommandFailureResourceUploadCancelled()
    kind == CommandFailureKindResourceEarlyEof && return CommandFailureResourceEarlyEof()
    kind == CommandFailureKindResourceLengthOverrun &&
        return CommandFailureResourceLengthOverrun()
    kind == CommandFailureKindPermissionDenied &&
        return CommandFailurePermissionDenied(detail)
    kind == CommandFailureKindDeviceUnavailable &&
        return CommandFailureDeviceUnavailable(detail)
    kind == CommandFailureKindConnectFailed &&
        return CommandFailureConnectFailed(detail)
    kind == CommandFailureKindBackendFailed &&
        return CommandFailureBackendFailed(detail)
    kind == CommandFailureKindResponseTooLarge &&
        return CommandFailureResponseTooLarge()
    throw(StatusFailure(:decode_command_failure, StatusBackendFailed))
end

function Base.wait(
    command::Command;
    timeout_milliseconds::UInt32=NEVER_TIMEOUT,
)
    lock(command.wait_guard) do
        output = Ref(
            NativeCommandResult(
                sizeof(NativeCommandResult),
                0,
                0,
                0,
                0,
                NativeByteView(C_NULL, 0),
                NativeStringView(C_NULL, 0),
            ),
        )
        if timeout_milliseconds != NEVER_TIMEOUT
            checked_status(
                :wait_command,
                ccall(
                    native_symbol(:prns_command_wait),
                    UInt32,
                    (Ptr{Cvoid}, UInt32, Ref{NativeCommandResult}),
                    command_pointer(command),
                    timeout_milliseconds,
                    output,
                ),
            )
            return decode_settlement(output[])
        end
        while true
            status = Status(
                ccall(
                    native_symbol(:prns_command_wait),
                    UInt32,
                    (Ptr{Cvoid}, UInt32, Ref{NativeCommandResult}),
                    command_pointer(command),
                    UInt32(0),
                    output,
                ),
            )
            status == StatusTimedOut && (wait(command.readiness); continue)
            status == StatusOk || throw(StatusFailure(:wait_command, status))
            return decode_settlement(output[])
        end
    end
end

function interrupt_wait!(command::Command)
    lock(command.guard) do
        command.pointer == C_NULL && return nothing
        ccall(
            native_symbol(:prns_command_interrupt_wait),
            Cvoid,
            (Ptr{Cvoid},),
            command.pointer,
        )
    end
    nothing
end

function Base.close(command::Command)
    pointer, registration = lock(command.guard) do
        pointer = command.pointer
        registration = command.registration
        command.pointer = C_NULL
        command.registration = C_NULL
        (pointer, registration)
    end
    pointer == C_NULL && return nothing
    ccall(
        native_symbol(:prns_command_interrupt_wait),
        Cvoid,
        (Ptr{Cvoid},),
        pointer,
    )
    lock(command.wait_guard) do
        release_readiness(registration, command.readiness)
        ccall(
            native_symbol(:prns_command_release),
            Cvoid,
            (Ptr{Cvoid},),
            pointer,
        )
    end
    nothing
end
