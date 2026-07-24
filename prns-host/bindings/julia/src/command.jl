abstract type CommandSettlement end

struct CommandSucceeded <: CommandSettlement
    outcome::CommandOutcome
end

struct CommandFailed <: CommandSettlement
    failure::CommandFailure
end

mutable struct Command
    pointer::Ptr{Cvoid}
    guard::ReentrantLock
    wait_guard::ReentrantLock
end

function Command(pointer::Ptr{Cvoid})
    command = Command(pointer, ReentrantLock(), ReentrantLock())
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
                        UInt32,
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
        decode_settlement(output[])
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
    pointer = lock(command.guard) do
        pointer = command.pointer
        command.pointer = C_NULL
        pointer
    end
    pointer == C_NULL && return nothing
    ccall(
        native_symbol(:prns_command_interrupt_wait),
        Cvoid,
        (Ptr{Cvoid},),
        pointer,
    )
    lock(command.wait_guard) do
        ccall(
            native_symbol(:prns_command_release),
            Cvoid,
            (Ptr{Cvoid},),
            pointer,
        )
    end
    nothing
end
