namespace PersonalRns;

internal static class EventDecoder
{
    internal static ApplicationEvent Application(EventHandle @event)
    {
        return (ApplicationEventKind)Native.prns_event_kind(@event) switch
        {
            ApplicationEventKind.SingleDelivery =>
                new ApplicationEvent.SingleDelivery(
                    new DestinationHash(Bytes(@event, EventField.Destination)),
                    new InterfaceId(Bytes(@event, EventField.SourceInterface)),
                    Bytes(@event, EventField.Plaintext)
                ),
            ApplicationEventKind.LinkDelivery =>
                new ApplicationEvent.LinkDelivery(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new InterfaceId(Bytes(@event, EventField.SourceInterface)),
                    Bytes(@event, EventField.Plaintext)
                ),
            ApplicationEventKind.Request =>
                new ApplicationEvent.Request(
                    new DestinationHash(Bytes(@event, EventField.Destination)),
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new RequestId(Bytes(@event, EventField.RequestId)),
                    OptionalBytes(@event, EventField.Requester) is { } requester
                        ? new IdentityHash(requester)
                        : null,
                    new RequestPathHash(Bytes(@event, EventField.PathHash)),
                    U64(@event, EventField.RttMillis),
                    Bytes(@event, EventField.Data)
                ),
            ApplicationEventKind.Response =>
                new ApplicationEvent.Response(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new RequestId(Bytes(@event, EventField.RequestId)),
                    Bytes(@event, EventField.Data)
                ),
            ApplicationEventKind.ResponseSegment =>
                new ApplicationEvent.ResponseSegment(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new RequestId(Bytes(@event, EventField.RequestId)),
                    U64(@event, EventField.SegmentIndex),
                    U64(@event, EventField.TotalSegments),
                    Bytes(@event, EventField.Data)
                ),
            ApplicationEventKind.ResourceAvailable =>
                new ApplicationEvent.ResourceAvailable(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new ResourceHash(Bytes(@event, EventField.Hash)),
                    OptionalBytes(@event, EventField.Metadata),
                    Resource(@event)
                ),
            ApplicationEventKind.ResourceSegment =>
                new ApplicationEvent.ResourceSegment(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new ResourceHash(Bytes(@event, EventField.OriginalHash)),
                    U64(@event, EventField.SegmentIndex),
                    U64(@event, EventField.TotalSegments),
                    OptionalBytes(@event, EventField.Metadata),
                    Bytes(@event, EventField.Data)
                ),
            ApplicationEventKind.ResourceNeedsDecompression =>
                new ApplicationEvent.ResourceNeedsDecompression(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new ResourceHash(Bytes(@event, EventField.Hash)),
                    Bytes(@event, EventField.Stream),
                    U64(@event, EventField.UncompressedDataBytes)
                ),
            ApplicationEventKind.ChannelMessage =>
                new ApplicationEvent.ChannelMessage(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    checked((ushort)U64(@event, EventField.MessageType)),
                    Bytes(@event, EventField.Data)
                ),
            var kind => throw new InvalidDataException($"Unknown application event kind {kind}."),
        };
    }

    internal static DiagnosticEvent Diagnostic(EventHandle @event)
    {
        return (DiagnosticEventKind)Native.prns_event_kind(@event) switch
        {
            DiagnosticEventKind.AnnounceHeard =>
                new DiagnosticEvent.AnnounceHeard(
                    new DestinationHash(Bytes(@event, EventField.Destination)),
                    checked((byte)U64(@event, EventField.Hops)),
                    new InterfaceId(Bytes(@event, EventField.SourceInterface)),
                    Bytes(@event, EventField.AppData)
                ),
            DiagnosticEventKind.LinkEstablished =>
                new DiagnosticEvent.LinkEstablished(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    U64(@event, EventField.RttMillis)
                ),
            DiagnosticEventKind.PeerIdentified =>
                new DiagnosticEvent.PeerIdentified(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new IdentityHash(Bytes(@event, EventField.Identity))
                ),
            DiagnosticEventKind.LinkClosed =>
                new DiagnosticEvent.LinkClosed(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    LinkReason(@event)
                ),
            DiagnosticEventKind.LinkInterfaceMismatch =>
                new DiagnosticEvent.LinkInterfaceMismatch(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new InterfaceId(Bytes(@event, EventField.AttachedInterface)),
                    new InterfaceId(Bytes(@event, EventField.ArrivedOn))
                ),
            DiagnosticEventKind.ResourceAssembled =>
                new DiagnosticEvent.ResourceAssembled(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new ResourceHash(Bytes(@event, EventField.OriginalHash)),
                    U64(@event, EventField.TotalSizeBytes)
                ),
            DiagnosticEventKind.ResourceFailed =>
                new DiagnosticEvent.ResourceFailed(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    new ResourceHash(Bytes(@event, EventField.Hash)),
                    String(@event, EventField.Cause)
                ),
            DiagnosticEventKind.ResourceSendProgress =>
                new DiagnosticEvent.ResourceSendProgress(
                    new LinkId(Bytes(@event, EventField.LinkId)),
                    U64(@event, EventField.TransferredBytes),
                    U64(@event, EventField.TotalBytes),
                    U64(@event, EventField.PhysicalTransferredBytes),
                    U64(@event, EventField.SegmentIndex),
                    U64(@event, EventField.TotalSegments)
                ),
            DiagnosticEventKind.SelfRatchetRotated =>
                new DiagnosticEvent.SelfRatchetRotated(
                    new DestinationHash(Bytes(@event, EventField.Destination))
                ),
            DiagnosticEventKind.AnnounceHeldDropped =>
                new DiagnosticEvent.AnnounceHeldDropped(
                    new DestinationHash(Bytes(@event, EventField.Destination)),
                    new InterfaceId(Bytes(@event, EventField.SourceInterface)),
                    String(@event, EventField.Cause)
                ),
            DiagnosticEventKind.Delivered =>
                new DiagnosticEvent.Delivered(String(@event, EventField.Detail)),
            DiagnosticEventKind.RouteExpired =>
                new DiagnosticEvent.RouteExpired(
                    new DestinationHash(Bytes(@event, EventField.Destination))
                ),
            DiagnosticEventKind.RouteEvicted =>
                new DiagnosticEvent.RouteEvicted(
                    new DestinationHash(Bytes(@event, EventField.Destination))
                ),
            DiagnosticEventKind.RouteInterfaceGone =>
                new DiagnosticEvent.RouteInterfaceGone(
                    new DestinationHash(Bytes(@event, EventField.Destination))
                ),
            DiagnosticEventKind.RouteDropped =>
                new DiagnosticEvent.RouteDropped(
                    new DestinationHash(Bytes(@event, EventField.Destination))
                ),
            DiagnosticEventKind.BackendDiagnostic =>
                new DiagnosticEvent.BackendDiagnostic(
                    String(@event, EventField.Kind),
                    String(@event, EventField.Detail)
                ),
            DiagnosticEventKind.DiagnosticsDropped =>
                new DiagnosticEvent.DiagnosticsDropped(U128(@event, EventField.DroppedCount)),
            DiagnosticEventKind.PersistenceRestored =>
                new DiagnosticEvent.PersistenceRestored(
                    U64(@event, EventField.Routes),
                    U64(@event, EventField.DestinationIdentities),
                    U64(@event, EventField.Tunnels),
                    U64(@event, EventField.Ratchets),
                    U64(@event, EventField.Refused),
                    U64(@event, EventField.Dropped)
                ),
            DiagnosticEventKind.PersistenceFlushed =>
                new DiagnosticEvent.PersistenceFlushed(
                    PersistenceCause(@event),
                    PersistenceTarget(@event)
                ),
            DiagnosticEventKind.PersistenceFlushFailed =>
                new DiagnosticEvent.PersistenceFlushFailed(
                    PersistenceCause(@event),
                    PersistenceTarget(@event)
                ),
            var kind => throw new InvalidDataException($"Unknown diagnostic event kind {kind}."),
        };
    }

    private static PersistenceFlushCause PersistenceCause(EventHandle @event)
    {
        var raw = checked((uint)U64(@event, EventField.PersistenceCause));
        var cause = (PersistenceFlushCause)raw;
        return Enum.IsDefined(cause)
            ? cause
            : throw new InvalidDataException($"Unknown persistence flush cause {raw}.");
    }

    private static PersistenceFlushTarget PersistenceTarget(EventHandle @event)
    {
        var raw = checked((uint)U64(@event, EventField.PersistenceTarget));
        var target = (PersistenceFlushTarget)raw;
        return Enum.IsDefined(target)
            ? target
            : throw new InvalidDataException($"Unknown persistence flush target {raw}.");
    }

    private static LinkClosedReason LinkReason(EventHandle @event)
    {
        var raw = checked((uint)U64(@event, EventField.Reason));
        var reason = (LinkClosedReason)raw;
        return Enum.IsDefined(reason)
            ? reason
            : throw new InvalidDataException($"Unknown link-close reason {raw}.");
    }

    private static byte[] Bytes(EventHandle @event, EventField field)
    {
        PrnsException.ThrowIfError(Native.prns_event_bytes(@event, field, out var view));
        return NativeValue.CopyBytes(view);
    }

    private static byte[]? OptionalBytes(EventHandle @event, EventField field)
    {
        var status = Native.prns_event_bytes(@event, field, out var view);
        if (status == Status.InvalidArgument)
        {
            return null;
        }
        PrnsException.ThrowIfError(status);
        return NativeValue.CopyBytes(view);
    }

    private static string String(EventHandle @event, EventField field)
    {
        PrnsException.ThrowIfError(Native.prns_event_string(@event, field, out var view));
        return NativeValue.CopyString(view);
    }

    private static ulong U64(EventHandle @event, EventField field)
    {
        PrnsException.ThrowIfError(Native.prns_event_u64(@event, field, out var value));
        return value;
    }

    private static UInt128 U128(EventHandle @event, EventField field)
    {
        PrnsException.ThrowIfError(
            Native.prns_event_u128(@event, field, out var low, out var high)
        );
        return ((UInt128)high << 64) | low;
    }

    private static ResourceStream Resource(EventHandle @event)
    {
        var totalBytes = U64(@event, EventField.TotalBytes);
        PrnsException.ThrowIfError(
            Native.prns_event_resource_stream(@event, out var stream)
        );
        return new ResourceStream(stream, totalBytes);
    }
}
