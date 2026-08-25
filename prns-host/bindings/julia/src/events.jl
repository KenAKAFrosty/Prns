abstract type StreamClaim{T} end

struct StreamClaimed{T} <: StreamClaim{T}
    stream::T
end

struct StreamAlreadyClaimed{T} <: StreamClaim{T} end

abstract type OwnedEventStream end

mutable struct ApplicationEventStream <: OwnedEventStream
    pointer::Ptr{Cvoid}
    readiness::Base.AsyncCondition
    registration::Ptr{Cvoid}
    guard::ReentrantLock
    wait_guard::ReentrantLock
end

mutable struct DiagnosticEventStream <: OwnedEventStream
    pointer::Ptr{Cvoid}
    readiness::Base.AsyncCondition
    registration::Ptr{Cvoid}
    guard::ReentrantLock
    wait_guard::ReentrantLock
end

function owned_event_stream(::Type{T}, pointer::Ptr{Cvoid}) where {T<:OwnedEventStream}
    readiness, registration = try
        register_readiness(pointer, :prns_event_stream_register_readiness)
    catch
        ccall(
            native_symbol(:prns_event_stream_release),
            Cvoid,
            (Ptr{Cvoid},),
            pointer,
        )
        rethrow()
    end
    stream = T(
        pointer,
        readiness,
        registration,
        ReentrantLock(),
        ReentrantLock(),
    )
    finalizer(close, stream)
    stream
end

function claim_stream(host::Host, symbol::Symbol, ::Type{T}) where {T<:OwnedEventStream}
    output = Ref{Ptr{Cvoid}}(C_NULL)
    status = Status(
        with_host_pointer(host) do pointer
            ccall(
                native_symbol(symbol),
                UInt32,
                (Ptr{Cvoid}, Ref{Ptr{Cvoid}}),
                pointer,
                output,
            )
        end,
    )
    status == StatusAlreadyClaimed && return StreamAlreadyClaimed{T}()
    status == StatusOk || throw(StatusFailure(:claim_stream, status))
    StreamClaimed{T}(owned_event_stream(T, output[]))
end

claim_application_events(host::Host) = claim_stream(
    host,
    :prns_host_claim_application_events,
    ApplicationEventStream,
)

claim_diagnostics(host::Host) = claim_stream(
    host,
    :prns_host_claim_diagnostics,
    DiagnosticEventStream,
)

application_events(host::Host) = claim_application_events(host)
diagnostics(host::Host) = claim_diagnostics(host)

function stream_pointer(stream::OwnedEventStream)
    lock(stream.guard) do
        stream.pointer == C_NULL && throw(EOFError())
        stream.pointer
    end
end

function next_event(stream::OwnedEventStream)
    lock(stream.wait_guard) do
        output = Ref{Ptr{Cvoid}}(C_NULL)
        while true
            status = Status(
                ccall(
                    native_symbol(:prns_event_stream_next),
                    UInt32,
                    (Ptr{Cvoid}, UInt32, Ref{Ptr{Cvoid}}),
                    stream_pointer(stream),
                    UInt32(0),
                    output,
                ),
            )
            status == StatusWouldBlock && (wait(stream.readiness); continue)
            status == StatusStopped && throw(EOFError())
            status == StatusOk || throw(StatusFailure(:next_event, status))
            return output[]
        end
    end
end

function interrupt_wait!(stream::OwnedEventStream)
    lock(stream.guard) do
        stream.pointer == C_NULL && return nothing
        ccall(
            native_symbol(:prns_event_stream_interrupt_wait),
            Cvoid,
            (Ptr{Cvoid},),
            stream.pointer,
        )
    end
    nothing
end

function Base.close(stream::OwnedEventStream)
    pointer, registration = lock(stream.guard) do
        pointer = stream.pointer
        registration = stream.registration
        stream.pointer = C_NULL
        stream.registration = C_NULL
        (pointer, registration)
    end
    pointer == C_NULL && return nothing
    ccall(
        native_symbol(:prns_event_stream_interrupt_wait),
        Cvoid,
        (Ptr{Cvoid},),
        pointer,
    )
    lock(stream.wait_guard) do
        release_readiness(registration, stream.readiness)
        ccall(
            native_symbol(:prns_event_stream_release),
            Cvoid,
            (Ptr{Cvoid},),
            pointer,
        )
    end
    nothing
end

function event_kind(event::Ptr{Cvoid})
    ccall(
        native_symbol(:prns_event_kind),
        UInt32,
        (Ptr{Cvoid},),
        event,
    )
end

function event_bytes(event::Ptr{Cvoid}, field::EventField)
    output = Ref(NativeByteView(C_NULL, 0))
    checked_status(
        :event_bytes,
        ccall(
            native_symbol(:prns_event_bytes),
            UInt32,
            (Ptr{Cvoid}, UInt32, Ref{NativeByteView}),
            event,
            UInt32(field),
            output,
        ),
    )
    copy_view(output[])
end

function optional_event_bytes(event::Ptr{Cvoid}, field::EventField)
    output = Ref(NativeByteView(C_NULL, 0))
    status = Status(
        ccall(
            native_symbol(:prns_event_bytes),
            UInt32,
            (Ptr{Cvoid}, UInt32, Ref{NativeByteView}),
            event,
            UInt32(field),
            output,
        ),
    )
    status == StatusInvalidArgument && return nothing
    status == StatusOk || throw(StatusFailure(:event_bytes, status))
    copy_view(output[])
end

function event_string(event::Ptr{Cvoid}, field::EventField)
    output = Ref(NativeStringView(C_NULL, 0))
    checked_status(
        :event_string,
        ccall(
            native_symbol(:prns_event_string),
            UInt32,
            (Ptr{Cvoid}, UInt32, Ref{NativeStringView}),
            event,
            UInt32(field),
            output,
        ),
    )
    copy_string(output[])
end

function event_u64(event::Ptr{Cvoid}, field::EventField)
    output = Ref{UInt64}(0)
    checked_status(
        :event_u64,
        ccall(
            native_symbol(:prns_event_u64),
            UInt32,
            (Ptr{Cvoid}, UInt32, Ref{UInt64}),
            event,
            UInt32(field),
            output,
        ),
    )
    output[]
end

function event_u128(event::Ptr{Cvoid}, field::EventField)
    low = Ref{UInt64}(0)
    high = Ref{UInt64}(0)
    checked_status(
        :event_u128,
        ccall(
            native_symbol(:prns_event_u128),
            UInt32,
            (Ptr{Cvoid}, UInt32, Ref{UInt64}, Ref{UInt64}),
            event,
            UInt32(field),
            low,
            high,
        ),
    )
    UInt128(low[]) | UInt128(high[]) << 64
end

function optional_identity_hash(event::Ptr{Cvoid})
    value = optional_event_bytes(event, EventFieldRequester)
    value === nothing ? nothing : IdentityHash(value)
end

mutable struct NativeResourceStream <: ResourceStream
    pointer::Ptr{Cvoid}
    total_bytes::UInt64
    guard::ReentrantLock
end

function NativeResourceStream(pointer::Ptr{Cvoid}, total_bytes::UInt64)
    stream = NativeResourceStream(pointer, total_bytes, ReentrantLock())
    finalizer(close, stream)
    stream
end

function event_resource_stream(event::Ptr{Cvoid})
    output = Ref{Ptr{Cvoid}}(C_NULL)
    checked_status(
        :resource_stream,
        ccall(
            native_symbol(:prns_event_resource_stream),
            UInt32,
            (Ptr{Cvoid}, Ref{Ptr{Cvoid}}),
            event,
            output,
        ),
    )
    NativeResourceStream(
        output[],
        event_u64(event, EventFieldTotalBytes),
    )
end

function next!(
    stream::NativeResourceStream;
    maximum_bytes::Integer=64 * 1024,
)
    maximum_bytes > 0 || throw(ArgumentError("maximum_bytes must be positive"))
    lock(stream.guard) do
        stream.pointer == C_NULL && throw(EOFError())
        output = Ref(NativeByteView(C_NULL, 0))
        finished = Ref{UInt8}(0)
        checked_status(
            :resource_next,
            ccall(
                native_symbol(:prns_resource_stream_next),
                UInt32,
                (Ptr{Cvoid}, Csize_t, Ref{NativeByteView}, Ref{UInt8}),
                stream.pointer,
                maximum_bytes,
                output,
                finished,
            ),
        )
        (bytes=copy_view(output[]), finished=finished[] != 0)
    end
end

function Base.close(stream::NativeResourceStream)
    lock(stream.guard) do
        stream.pointer == C_NULL && return nothing
        ccall(
            native_symbol(:prns_resource_stream_release),
            Cvoid,
            (Ptr{Cvoid},),
            stream.pointer,
        )
        stream.pointer = C_NULL
    end
    nothing
end

function decode_application_event(event::Ptr{Cvoid})
    kind = ApplicationEventKind(event_kind(event))
    if kind == ApplicationEventKindSingleDelivery
        return ApplicationEventSingleDelivery(
            DestinationHash(event_bytes(event, EventFieldDestination)),
            InterfaceId(event_bytes(event, EventFieldSourceInterface)),
            event_bytes(event, EventFieldPlaintext),
        )
    end
    kind == ApplicationEventKindLinkDelivery &&
        return ApplicationEventLinkDelivery(
            LinkId(event_bytes(event, EventFieldLinkId)),
            InterfaceId(event_bytes(event, EventFieldSourceInterface)),
            event_bytes(event, EventFieldPlaintext),
        )
    kind == ApplicationEventKindRequest && return ApplicationEventRequest(
        DestinationHash(event_bytes(event, EventFieldDestination)),
        LinkId(event_bytes(event, EventFieldLinkId)),
        RequestId(event_bytes(event, EventFieldRequestId)),
        optional_identity_hash(event),
        RequestPathHash(event_bytes(event, EventFieldPathHash)),
        event_u64(event, EventFieldRttMillis),
        event_bytes(event, EventFieldData),
    )
    kind == ApplicationEventKindResponse && return ApplicationEventResponse(
        LinkId(event_bytes(event, EventFieldLinkId)),
        RequestId(event_bytes(event, EventFieldRequestId)),
        event_bytes(event, EventFieldData),
    )
    kind == ApplicationEventKindResponseSegment &&
        return ApplicationEventResponseSegment(
            LinkId(event_bytes(event, EventFieldLinkId)),
            RequestId(event_bytes(event, EventFieldRequestId)),
            event_u64(event, EventFieldSegmentIndex),
            event_u64(event, EventFieldTotalSegments),
            event_bytes(event, EventFieldData),
        )
    kind == ApplicationEventKindResourceAvailable &&
        return ApplicationEventResourceAvailable(
            LinkId(event_bytes(event, EventFieldLinkId)),
            ResourceHash(event_bytes(event, EventFieldHash)),
            optional_event_bytes(event, EventFieldMetadata),
            event_resource_stream(event),
        )
    kind == ApplicationEventKindResourceSegment &&
        return ApplicationEventResourceSegment(
            LinkId(event_bytes(event, EventFieldLinkId)),
            ResourceHash(event_bytes(event, EventFieldOriginalHash)),
            event_u64(event, EventFieldSegmentIndex),
            event_u64(event, EventFieldTotalSegments),
            optional_event_bytes(event, EventFieldMetadata),
            event_bytes(event, EventFieldData),
        )
    kind == ApplicationEventKindResourceNeedsDecompression &&
        return ApplicationEventResourceNeedsDecompression(
            LinkId(event_bytes(event, EventFieldLinkId)),
            ResourceHash(event_bytes(event, EventFieldHash)),
            event_bytes(event, EventFieldStream),
            event_u64(event, EventFieldUncompressedDataBytes),
        )
    kind == ApplicationEventKindChannelMessage &&
        return ApplicationEventChannelMessage(
            LinkId(event_bytes(event, EventFieldLinkId)),
            UInt16(event_u64(event, EventFieldMessageType)),
            event_bytes(event, EventFieldData),
        )
    throw(StatusFailure(:decode_application_event, StatusBackendFailed))
end

function decode_diagnostic_event(event::Ptr{Cvoid})
    kind = DiagnosticEventKind(event_kind(event))
    kind == DiagnosticEventKindAnnounceHeard &&
        return DiagnosticEventAnnounceHeard(
            DestinationHash(event_bytes(event, EventFieldDestination)),
            UInt8(event_u64(event, EventFieldHops)),
            InterfaceId(event_bytes(event, EventFieldSourceInterface)),
            event_bytes(event, EventFieldAppData),
        )
    kind == DiagnosticEventKindLinkEstablished &&
        return DiagnosticEventLinkEstablished(
            LinkId(event_bytes(event, EventFieldLinkId)),
            event_u64(event, EventFieldRttMillis),
        )
    kind == DiagnosticEventKindPeerIdentified &&
        return DiagnosticEventPeerIdentified(
            LinkId(event_bytes(event, EventFieldLinkId)),
            IdentityHash(event_bytes(event, EventFieldIdentity)),
        )
    kind == DiagnosticEventKindLinkClosed && return DiagnosticEventLinkClosed(
        LinkId(event_bytes(event, EventFieldLinkId)),
        LinkClosedReason(event_u64(event, EventFieldReason)),
    )
    kind == DiagnosticEventKindLinkInterfaceMismatch &&
        return DiagnosticEventLinkInterfaceMismatch(
            LinkId(event_bytes(event, EventFieldLinkId)),
            InterfaceId(event_bytes(event, EventFieldAttachedInterface)),
            InterfaceId(event_bytes(event, EventFieldArrivedOn)),
        )
    kind == DiagnosticEventKindResourceAssembled &&
        return DiagnosticEventResourceAssembled(
            LinkId(event_bytes(event, EventFieldLinkId)),
            ResourceHash(event_bytes(event, EventFieldOriginalHash)),
            event_u64(event, EventFieldTotalSizeBytes),
        )
    kind == DiagnosticEventKindResourceFailed &&
        return DiagnosticEventResourceFailed(
            LinkId(event_bytes(event, EventFieldLinkId)),
            ResourceHash(event_bytes(event, EventFieldHash)),
            event_string(event, EventFieldCause),
        )
    kind == DiagnosticEventKindResourceSendProgress &&
        return DiagnosticEventResourceSendProgress(
            LinkId(event_bytes(event, EventFieldLinkId)),
            event_u64(event, EventFieldTransferredBytes),
            event_u64(event, EventFieldTotalBytes),
            event_u64(event, EventFieldPhysicalTransferredBytes),
            event_u64(event, EventFieldSegmentIndex),
            event_u64(event, EventFieldTotalSegments),
        )
    kind == DiagnosticEventKindSelfRatchetRotated &&
        return DiagnosticEventSelfRatchetRotated(
            DestinationHash(event_bytes(event, EventFieldDestination)),
        )
    kind == DiagnosticEventKindAnnounceHeldDropped &&
        return DiagnosticEventAnnounceHeldDropped(
            DestinationHash(event_bytes(event, EventFieldDestination)),
            InterfaceId(event_bytes(event, EventFieldSourceInterface)),
            event_string(event, EventFieldCause),
        )
    kind == DiagnosticEventKindDelivered && return DiagnosticEventDelivered(
        event_string(event, EventFieldDetail),
    )
    kind == DiagnosticEventKindRouteExpired &&
        return DiagnosticEventRouteExpired(
            DestinationHash(event_bytes(event, EventFieldDestination)),
        )
    kind == DiagnosticEventKindRouteEvicted &&
        return DiagnosticEventRouteEvicted(
            DestinationHash(event_bytes(event, EventFieldDestination)),
        )
    kind == DiagnosticEventKindRouteInterfaceGone &&
        return DiagnosticEventRouteInterfaceGone(
            DestinationHash(event_bytes(event, EventFieldDestination)),
        )
    kind == DiagnosticEventKindRouteDropped &&
        return DiagnosticEventRouteDropped(
            DestinationHash(event_bytes(event, EventFieldDestination)),
        )
    kind == DiagnosticEventKindBackendDiagnostic &&
        return DiagnosticEventBackendDiagnostic(
            event_string(event, EventFieldKind),
            event_string(event, EventFieldDetail),
        )
    kind == DiagnosticEventKindDiagnosticsDropped &&
        return DiagnosticEventDiagnosticsDropped(
            event_u128(event, EventFieldDroppedCount),
        )
    kind == DiagnosticEventKindPersistenceRestored &&
        return DiagnosticEventPersistenceRestored(
            event_u64(event, EventFieldRoutes),
            event_u64(event, EventFieldDestinationIdentities),
            event_u64(event, EventFieldTunnels),
            event_u64(event, EventFieldRatchets),
            event_u64(event, EventFieldRefused),
            event_u64(event, EventFieldDropped),
        )
    if kind == DiagnosticEventKindPersistenceFlushed ||
       kind == DiagnosticEventKindPersistenceFlushFailed
        cause = PersistenceFlushCause(
            UInt32(event_u64(event, EventFieldPersistenceCause)),
        )
        target = PersistenceFlushTarget(
            UInt32(event_u64(event, EventFieldPersistenceTarget)),
        )
        kind == DiagnosticEventKindPersistenceFlushed &&
            return DiagnosticEventPersistenceFlushed(cause, target)
        return DiagnosticEventPersistenceFlushFailed(cause, target)
    end
    throw(StatusFailure(:decode_diagnostic_event, StatusBackendFailed))
end

function decoded_event(stream::OwnedEventStream, decoder)
    event = next_event(stream)
    try
        decoder(event)
    finally
        ccall(
            native_symbol(:prns_event_release),
            Cvoid,
            (Ptr{Cvoid},),
            event,
        )
    end
end

next!(stream::ApplicationEventStream) =
    decoded_event(stream, decode_application_event)

next!(stream::DiagnosticEventStream) =
    decoded_event(stream, decode_diagnostic_event)

function Base.iterate(stream::OwnedEventStream, state=nothing)
    try
        (next!(stream), nothing)
    catch failure
        failure isa EOFError && return nothing
        rethrow()
    end
end
