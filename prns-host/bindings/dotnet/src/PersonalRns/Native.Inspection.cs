using System.Runtime.InteropServices;

namespace PersonalRns;

internal static partial class Native
{
    [StructLayout(LayoutKind.Sequential)]
    internal struct BackendInfo
    {
        internal nuint StructSize;
        internal BackendKind Backend;
        internal nint Capabilities;
        internal nuint CapabilityCount;
        internal nint InterfaceKinds;
        internal nuint InterfaceKindCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct InterfaceSnapshot
    {
        internal nuint StructSize;
        internal ByteView InterfaceId;
        internal byte HasName;
        internal StringView Name;
        internal byte HasKind;
        internal InterfaceKind Kind;
        internal InterfaceHealth Health;
        internal byte HasFailureDetail;
        internal StringView FailureDetail;
        internal ulong RxBytes;
        internal ulong TxBytes;
        internal byte HasRxBps;
        internal ulong RxBps;
        internal byte HasTxBps;
        internal ulong TxBps;
        internal uint RouteCount;
        internal uint LinkCount;
        internal uint TransportedLinkCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct RouteSnapshot
    {
        internal nuint StructSize;
        internal ByteView Destination;
        internal byte Hops;
        internal byte HasViaIdentity;
        internal ByteView ViaIdentity;
        internal ByteView InterfaceId;
        internal ulong LearnedAtMillis;
        internal ulong LastRelayedAtMillis;
        internal ulong ExpiresAtMillis;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct DestinationIdentitySnapshot
    {
        internal nuint StructSize;
        internal ByteView Destination;
        internal ByteView Identity;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct RuntimeHealthSnapshot
    {
        internal nuint StructSize;
        internal byte Running;
        internal ulong UptimeMillis;
        internal uint InterfaceCount;
        internal uint OnlineInterfaceCount;
        internal uint RouteCount;
        internal uint LinkCount;
        internal uint TransportedLinkCount;
        internal ulong RxBytes;
        internal ulong TxBytes;
        internal ulong RxBps;
        internal ulong TxBps;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct PersistenceSnapshot
    {
        internal nuint StructSize;
        internal byte Persistent;
        internal byte Restored;
        internal byte HasLastFlushCause;
        internal PersistenceFlushCause LastFlushCause;
        internal byte HasLastFailureDetail;
        internal StringView LastFailureDetail;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct HostSnapshot
    {
        internal nuint StructSize;
        internal ulong Revision;
        internal BackendInfo Backend;
        internal nint Interfaces;
        internal nuint InterfaceCount;
        internal nint Routes;
        internal nuint RouteCount;
        internal uint ActiveLinkCount;
        internal nint DestinationIdentities;
        internal nuint DestinationIdentityCount;
        internal RuntimeHealthSnapshot Runtime;
        internal PersistenceSnapshot Persistence;
    }

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_backend_info(ref BackendInfo info);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_snapshot(
        HostHandle host,
        uint timeoutMillis,
        out nint snapshot
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_snapshot_read(
        nint snapshot,
        ref HostSnapshot value
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_host_snapshot_release(nint snapshot);
}
