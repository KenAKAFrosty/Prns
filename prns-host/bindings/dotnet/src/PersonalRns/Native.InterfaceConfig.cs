using System.Runtime.InteropServices;

namespace PersonalRns;

internal static partial class Native
{
    [StructLayout(LayoutKind.Sequential)]
    internal struct SerialLineConfig
    {
        internal nuint StructSize;
        internal uint Baud;
        internal SerialDataBits DataBits;
        internal SerialParity Parity;
        internal SerialStopBits StopBits;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct RNodeRadioConfig
    {
        internal nuint StructSize;
        internal ulong FrequencyHz;
        internal uint BandwidthHz;
        internal short TxPowerDbm;
        internal byte SpreadingFactor;
        internal byte CodingRate;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct MultiRNodeMemberConfig
    {
        internal nuint StructSize;
        internal StringView Name;
        internal byte VirtualPort;
        internal RNodeRadioConfig Radio;
        internal byte FlowControl;
        internal byte Outgoing;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct InterfaceConfig
    {
        internal nuint StructSize;
        internal InterfaceKind Kind;
        internal byte HasGroupId;
        internal StringView GroupId;
        internal byte HasDiscoveryScope;
        internal DiscoveryScope DiscoveryScope;
        internal byte HasDiscoveryPort;
        internal ushort DiscoveryPort;
        internal byte HasDataPort;
        internal ushort DataPort;
        internal nint Devices;
        internal nuint DeviceCount;
        internal nint IgnoredDevices;
        internal nuint IgnoredDeviceCount;
        internal byte HasMulticastAddressType;
        internal MulticastAddressType MulticastAddressType;
        internal StringView Target;
        internal StringView Bind;
        internal StringView Local;
        internal StringView Peer;
        internal BitrateKind BitrateKind;
        internal ulong BitrateBps;
        internal StringView Port;
        internal SerialLineConfig Line;
        internal byte FlowControl;
        internal uint PreambleMillis;
        internal uint TransmitTailMillis;
        internal byte Persistence;
        internal uint SlotTimeMillis;
        internal byte HasStationCallsign;
        internal StringView StationCallsign;
        internal byte HasStationIntervalSeconds;
        internal ulong StationIntervalSeconds;
        internal StringView Callsign;
        internal byte Ssid;
        internal RNodeRadioConfig Radio;
        internal byte HasAirtimeLimitShortCentiPercent;
        internal ushort AirtimeLimitShortCentiPercent;
        internal byte HasAirtimeLimitLongCentiPercent;
        internal ushort AirtimeLimitLongCentiPercent;
        internal nint Members;
        internal nuint MemberCount;
        internal nint Command;
        internal nuint CommandCount;
        internal ulong RespawnDelayMillis;
        internal nint Peers;
        internal nuint PeerCount;
        internal byte Connectable;
        internal StringView Url;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct InterfaceRoutingPolicy
    {
        internal nuint StructSize;
        internal byte HasMode;
        internal PersonalRns.InterfaceMode Mode;
        internal byte HasGravity;
        internal long Gravity;
        internal byte HasRecursivePathRequests;
        internal byte RecursivePathRequests;
        internal byte HasAnnouncesFromInternal;
        internal byte AnnouncesFromInternal;
        internal byte HasAnnouncesToInternal;
        internal byte AnnouncesToInternal;
    }

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_host_attach_interface(
        HostHandle host,
        in InterfaceConfig config,
        nint routing,
        out CommandHandle command
    );
}
