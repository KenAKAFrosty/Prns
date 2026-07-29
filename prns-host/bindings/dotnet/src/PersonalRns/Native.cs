using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace PersonalRns;

internal static class Native
{
    internal const string Library = "prns_host";
    internal const uint NeverTimeout = uint.MaxValue;

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate void ReadinessCallback(nint context);

    [StructLayout(LayoutKind.Sequential)]
    internal struct ByteView
    {
        internal nint Data;
        internal nuint Length;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct StringView
    {
        internal nint Data;
        internal nuint Length;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct ContractInfo
    {
        internal nuint StructSize;
        internal uint Abi;
        internal uint SchemaVersion;
        internal StringView ProductVersion;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct Limits
    {
        internal nuint StructSize;
        internal nuint PendingCommands;
        internal nuint ApplicationEvents;
        internal nuint RetainedEventBytes;
        internal nuint Diagnostics;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct IdentityConfig
    {
        internal nuint StructSize;
        internal IdentityConfigKind Kind;
        internal ByteView Secret;
        internal StringView Path;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct DestinationName
    {
        internal nuint StructSize;
        internal StringView AppName;
        internal nint Aspects;
        internal nuint AspectCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct RequestHandlerConfig
    {
        internal nuint StructSize;
        internal StringView Path;
        internal RequestPolicy Policy;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct DestinationConfig
    {
        internal nuint StructSize;
        internal DestinationConfigKind Kind;
        internal DestinationName Name;
        internal DestinationIdentityConfigKind IdentityKind;
        internal IdentityConfig DedicatedIdentity;
        internal ByteView AnnounceAppData;
        internal nint RequestHandlers;
        internal nuint RequestHandlerCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct HostOptions
    {
        internal nuint StructSize;
        internal uint RequiredAbi;
        internal StringView RequiredProductVersion;
        internal Limits Limits;
        internal HostRole Role;
        internal IdentityConfig Identity;
        internal nint Destinations;
        internal nuint DestinationCount;
        internal nint RequiredCapabilities;
        internal nuint RequiredCapabilityCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct Lifecycle
    {
        internal nuint StructSize;
        internal ulong Revision;
        internal LifecyclePhase Phase;
        internal uint Reason;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct CommandResult
    {
        internal nuint StructSize;
        internal CommandOutcomeKind Outcome;
        internal CommandFailureKind Failure;
        internal DeliveryEvidenceKind Evidence;
        internal ulong RttMillis;
        internal ByteView Value;
        internal StringView Detail;
    }

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_contract_info(ref ContractInfo info);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_create(in HostOptions options, out HostHandle host);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_host_release(nint host);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_lifecycle(HostHandle host, ref Lifecycle lifecycle);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_identity_hash(HostHandle host, out ByteView hash);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern nuint prns_host_destination_count(HostHandle host);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_destination_hash(
        HostHandle host,
        nuint index,
        out ByteView hash
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern unsafe Status prns_host_announce(
        HostHandle host,
        ByteView destination,
        ByteView* interfaceId,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_send_single_packet(
        HostHandle host,
        ByteView destination,
        ByteView payload,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_close_link(
        HostHandle host,
        ByteView linkId,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_attach_tcp_server(
        HostHandle host,
        StringView bind,
        BitrateKind bitrateKind,
        ulong bitrateBps,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_attach_tcp_client(
        HostHandle host,
        StringView target,
        BitrateKind bitrateKind,
        ulong bitrateBps,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_attach_udp(
        HostHandle host,
        StringView local,
        StringView peer,
        BitrateKind bitrateKind,
        ulong bitrateBps,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_detach_interface(
        HostHandle host,
        ByteView interfaceId,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_establish_link(
        HostHandle host,
        ByteView destination,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_request_path(
        HostHandle host,
        ByteView destination,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_identify(
        HostHandle host,
        ByteView linkId,
        ByteView identity,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_send_link_packet(
        HostHandle host,
        ByteView linkId,
        ByteView payload,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_request(
        HostHandle host,
        ByteView linkId,
        ByteView pathHash,
        ByteView payload,
        ResponseTimeoutKind timeoutKind,
        ulong timeoutMillis,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_respond(
        HostHandle host,
        ByteView linkId,
        ByteView requestId,
        ulong requestRttMillis,
        ByteView payload,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern unsafe Status prns_host_send_resource(
        HostHandle host,
        ByteView linkId,
        ByteView payload,
        ByteView* packedMetadata,
        ResourceCompressionKind compressionKind,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_set_link_resource_strategy(
        HostHandle host,
        ByteView linkId,
        ResourceStrategyKind strategyKind,
        ulong maximumUncompressedBytes,
        byte acceptCompressed,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_set_destination_resource_strategy(
        HostHandle host,
        ByteView destination,
        ResourceStrategyKind strategyKind,
        ulong maximumUncompressedBytes,
        byte acceptCompressed,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_send_channel_message(
        HostHandle host,
        ByteView linkId,
        ushort messageType,
        ByteView payload,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_allow_requester(
        HostHandle host,
        ByteView destination,
        ByteView pathHash,
        ByteView identity,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_stop(HostHandle host);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_command_wait(
        CommandHandle command,
        uint timeoutMillis,
        ref CommandResult result
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_command_register_readiness(
        CommandHandle command,
        ReadinessCallback callback,
        nint context,
        out ReadinessRegistrationHandle registration
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_command_interrupt_wait(CommandHandle command);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_command_release(nint command);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_claim_application_events(
        HostHandle host,
        out EventStreamHandle stream
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_claim_diagnostics(
        HostHandle host,
        out EventStreamHandle stream
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_event_stream_register_readiness(
        EventStreamHandle stream,
        ReadinessCallback callback,
        nint context,
        out ReadinessRegistrationHandle registration
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_readiness_registration_release(nint registration);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_event_stream_interrupt_wait(EventStreamHandle stream);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_event_stream_release(nint stream);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_event_stream_next(
        EventStreamHandle stream,
        uint timeoutMillis,
        out EventHandle @event
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_event_release(nint @event);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern uint prns_event_kind(EventHandle @event);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_event_bytes(
        EventHandle @event,
        EventField field,
        out ByteView value
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_event_string(
        EventHandle @event,
        EventField field,
        out StringView value
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_event_u64(
        EventHandle @event,
        EventField field,
        out ulong value
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_event_u128(
        EventHandle @event,
        EventField field,
        out ulong low,
        out ulong high
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_event_resource_stream(
        EventHandle @event,
        out ResourceStreamHandle stream
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_resource_stream_release(nint stream);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_resource_stream_next(
        ResourceStreamHandle stream,
        nuint maximumBytes,
        out ByteView chunk,
        out byte finished
    );
}

internal sealed class CommandHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    private CommandHandle()
        : base(true) { }

    protected override bool ReleaseHandle()
    {
        Native.prns_command_release(handle);
        return true;
    }
}

internal sealed class HostHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    private HostHandle()
        : base(true) { }

    protected override bool ReleaseHandle()
    {
        Native.prns_host_release(handle);
        return true;
    }
}

internal sealed class EventStreamHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    private EventStreamHandle()
        : base(true) { }

    protected override bool ReleaseHandle()
    {
        Native.prns_event_stream_release(handle);
        return true;
    }
}

internal sealed class EventHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    private EventHandle()
        : base(true) { }

    protected override bool ReleaseHandle()
    {
        Native.prns_event_release(handle);
        return true;
    }
}

internal sealed class ReadinessRegistrationHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    private ReadinessRegistrationHandle()
        : base(true) { }

    protected override bool ReleaseHandle()
    {
        Native.prns_readiness_registration_release(handle);
        return true;
    }
}

internal sealed class ResourceStreamHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    private ResourceStreamHandle()
        : base(true) { }

    protected override bool ReleaseHandle()
    {
        Native.prns_resource_stream_release(handle);
        return true;
    }
}
