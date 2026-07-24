using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace PersonalRns;

internal static class Native
{
    internal const string Library = "prns_host";
    internal const uint NeverTimeout = uint.MaxValue;

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
    internal struct HostOptions
    {
        internal nuint StructSize;
        internal uint RequiredAbi;
        internal StringView RequiredProductVersion;
        internal Limits Limits;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct Lifecycle
    {
        internal nuint StructSize;
        internal ulong Revision;
        internal LifecyclePhase Phase;
        internal uint Reason;
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
    internal static extern Status prns_host_stop(HostHandle host);

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
