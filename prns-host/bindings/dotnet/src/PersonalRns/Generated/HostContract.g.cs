#nullable enable

using System.Collections.Immutable;

namespace PersonalRns;

public static class HostContract
{
    public const uint Abi = 1;
    public const uint SchemaVersion = 1;
    public const string ProductVersion = "0.3.7";
    public const int DestinationHashLength = 16;
    public const int IdentityHashLength = 16;
    public const int InterfaceIdLength = 8;
    public const int LinkIdLength = 16;
    public const int PacketHashLength = 32;
    public const int RequestIdLength = 16;
    public const int RequestPathHashLength = 16;
    public const int ResourceHashLength = 32;
    public const int IdentitySecretLength = 64;
    public const long SafeIntMin = -9007199254740991;
    public const long SafeIntMax = 9007199254740991;
    public const ulong SafeUintMax = 9007199254740991;
    public const int BalancedPendingCommands = 256;
    public const int BalancedApplicationEvents = 1024;
    public const int BalancedRetainedEventBytes = 8388608;
    public const int BalancedDiagnostics = 1024;
}

public enum Status : uint
{
    Ok = 0,
    InvalidArgument = 1,
    ContractMismatch = 2,
    InvalidHandle = 3,
    NotReady = 4,
    AlreadyClaimed = 5,
    WouldBlock = 6,
    TimedOut = 7,
    QueueFull = 8,
    Stopped = 9,
    BackendFailed = 10,
    Panic = 11,
    Interrupted = 12,
    Unsupported = 13,
    PermissionDenied = 14,
    Unavailable = 15,
}

public enum BackendKind : uint
{
    Native = 1,
    Browser = 2,
    Cooperative = 3,
}

public enum Capability : uint
{
    Loopback = 1,
    TcpClient = 2,
    TcpServer = 3,
    Udp = 4,
    Serial = 5,
    Usb = 6,
    Bluetooth = 7,
    Wifi = 8,
    WebSocket = 9,
    BrowserRendezvous = 10,
    I2p = 11,
    Weave = 12,
    SuppliedPipe = 13,
}

public enum InterfaceKind : uint
{
    AutoLan = 1,
    TcpClient = 2,
    TcpServer = 3,
    Udp = 4,
    Serial = 5,
    Kiss = 6,
    Ax25Kiss = 7,
    RNode = 8,
    MultiRNode = 9,
    Pipe = 10,
    BackboneClient = 11,
    BackboneServer = 12,
    I2p = 13,
    Weave = 14,
    AutomaticUsb = 15,
    AutomaticBluetoothLe = 16,
    WebSocketClient = 17,
    WebSocketServer = 18,
    BrowserRendezvous = 19,
}

public enum InterfaceMode : uint
{
    Full = 1,
    PointToPoint = 2,
    AccessPoint = 3,
    Roaming = 4,
    Boundary = 5,
    Gateway = 6,
    Internal = 7,
}

public enum WebSocketFramingSelection : uint
{
    RawPacket = 1,
    Hdlc = 2,
    Kiss = 3,
    Auto = 4,
}

public enum InterfaceHealth : uint
{
    Initializing = 1,
    Connected = 2,
    Degraded = 3,
    Reconnecting = 4,
    Failed = 5,
    Disconnected = 6,
    Disabled = 7,
    Unknown = 8,
}

public enum DiscoveryScope : uint
{
    Link = 1,
    Admin = 2,
    Site = 3,
    Organization = 4,
    Global = 5,
}

public enum MulticastAddressType : uint
{
    Temporary = 1,
    Permanent = 2,
}

public enum SerialDataBits : uint
{
    Five = 5,
    Six = 6,
    Seven = 7,
    Eight = 8,
}

public enum SerialParity : uint
{
    None = 1,
    Even = 2,
    Odd = 3,
}

public enum SerialStopBits : uint
{
    One = 1,
    Two = 2,
}

public enum HostRole : uint
{
    Endpoint = 1,
    Transport = 2,
}

public enum IdentityConfigKind : uint
{
    Existing = 1,
    GenerateEphemeral = 2,
    LoadOrCreate = 3,
}

public enum PersistenceConfigKind : uint
{
    Ephemeral = 1,
    Directory = 2,
}

public enum DestinationConfigKind : uint
{
    Plain = 1,
    Single = 2,
}

public enum DestinationIdentityConfigKind : uint
{
    HostIdentity = 1,
    DedicatedIdentity = 2,
}

public enum BitrateKind : uint
{
    Auto = 1,
    BitsPerSecond = 2,
}

public enum ResponseTimeoutKind : uint
{
    LinkDefault = 1,
    Exact = 2,
}

public enum ResourceCompressionKind : uint
{
    Auto = 1,
    Never = 2,
}

public enum ResourceStrategyKind : uint
{
    Refuse = 1,
    Accept = 2,
}

public enum RequestPolicy : uint
{
    AllowNone = 1,
    AllowAll = 2,
    AllowList = 3,
}

public enum CommandOutcomeKind : uint
{
    Announced = 1,
    PacketDelivered = 2,
    LinkCloseQueued = 3,
    InterfaceAttached = 4,
    InterfaceDetached = 5,
    LinkEstablished = 6,
    PathDiscovered = 7,
    Identified = 8,
    ResponseReceived = 9,
    ResponseSent = 10,
    ResourceSent = 11,
    ResourceStrategySet = 12,
    RequesterAllowed = 13,
}

public enum CommandFailureKind : uint
{
    NodeStopped = 1,
    Busy = 2,
    PayloadTooLarge = 3,
    UnknownDestination = 4,
    NotSingleDestination = 5,
    AnnounceAppDataTooLong = 6,
    UnknownInterface = 7,
    NoRouteToDestination = 8,
    NotDirectlyReachable = 9,
    PacketCulled = 10,
    DeliveryTimedOut = 11,
    InvalidBitrate = 12,
    BindFailed = 13,
    WriteFailed = 14,
    UnsupportedByBackend = 15,
    UnknownLink = 16,
    LinkNotActive = 17,
    EntropyUnavailable = 18,
    NotLinkInitiator = 19,
    IdentityNotHeld = 20,
    UnknownRequestHandler = 21,
    RequestPolicyNotAllowList = 22,
    RequestAllowListFull = 23,
    LinkBusy = 24,
    ResourceTableFull = 25,
    ResourceMetadataTooLarge = 26,
    ResourceRejectedByPeer = 27,
    ResourceSequencingFailed = 28,
    ResourcePredecessorFailed = 29,
    ChannelWindowFull = 30,
    ChannelUntrackable = 31,
    InvalidChannelMessageType = 32,
    InvalidConfiguration = 33,
    ResourceUploadCancelled = 34,
    ResourceEarlyEof = 35,
    ResourceLengthOverrun = 36,
    PermissionDenied = 37,
    DeviceUnavailable = 38,
    ConnectFailed = 39,
    BackendFailed = 40,
    ResponseTooLarge = 41,
}

public enum DeliveryEvidenceKind : uint
{
    ExplicitProof = 1,
    ImplicitProof = 2,
    Response = 3,
}

public enum LifecyclePhase : uint
{
    Starting = 1,
    Running = 2,
    Stopping = 3,
    Stopped = 4,
    Failed = 5,
}

public enum StopReason : uint
{
    Requested = 1,
    BackendExited = 2,
}

public enum LinkClosedReason : uint
{
    Timeout = 1,
    PeerClosed = 2,
    MalformedRtt = 3,
}

public enum ApplicationEventKind : uint
{
    SingleDelivery = 100,
    Request = 101,
    Response = 102,
    ResponseSegment = 103,
    ResourceAvailable = 104,
    ResourceSegment = 105,
    ResourceNeedsDecompression = 106,
    ChannelMessage = 107,
    LinkDelivery = 108,
}

public enum DiagnosticEventKind : uint
{
    AnnounceHeard = 200,
    LinkEstablished = 201,
    PeerIdentified = 202,
    LinkClosed = 203,
    LinkInterfaceMismatch = 204,
    ResourceAssembled = 205,
    ResourceFailed = 206,
    ResourceSendProgress = 207,
    SelfRatchetRotated = 208,
    AnnounceHeldDropped = 209,
    Delivered = 210,
    RouteExpired = 211,
    RouteEvicted = 212,
    RouteInterfaceGone = 213,
    RouteDropped = 214,
    BackendDiagnostic = 215,
    DiagnosticsDropped = 216,
    PersistenceRestored = 217,
    PersistenceFlushed = 218,
    PersistenceFlushFailed = 219,
}

public enum PersistenceFlushCause : uint
{
    Startup = 1,
    Interval = 2,
    RouteChange = 3,
    RatchetRotation = 4,
    Shutdown = 5,
}

public enum PersistenceFlushTarget : uint
{
    RoutingState = 1,
    Ratchets = 2,
}

public enum EventField : uint
{
    Destination = 1,
    SourceInterface = 2,
    Plaintext = 3,
    LinkId = 4,
    RequestId = 5,
    Requester = 6,
    PathHash = 7,
    RttMillis = 8,
    Data = 9,
    SegmentIndex = 10,
    TotalSegments = 11,
    Hash = 12,
    OriginalHash = 13,
    Metadata = 14,
    TotalBytes = 15,
    StreamId = 16,
    UncompressedDataBytes = 17,
    MessageType = 18,
    Identity = 19,
    Reason = 20,
    AttachedInterface = 21,
    ArrivedOn = 22,
    TotalSizeBytes = 23,
    Cause = 24,
    TransferredBytes = 25,
    PhysicalTransferredBytes = 26,
    Detail = 27,
    Kind = 28,
    DroppedCount = 29,
    Hops = 30,
    Stream = 31,
    Routes = 32,
    DestinationIdentities = 33,
    Tunnels = 34,
    Ratchets = 35,
    Refused = 36,
    Dropped = 37,
    PersistenceCause = 38,
    PersistenceTarget = 39,
    AppData = 40,
}

public readonly struct DestinationHash : IEquatable<DestinationHash>
{
    private static readonly byte[] Zero = new byte[HostContract.DestinationHashLength];
    private readonly byte[]? _bytes;

    public DestinationHash(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length != HostContract.DestinationHashLength)
        {
            throw new ArgumentException(
                $"Expected exactly {HostContract.DestinationHashLength} bytes.",
                nameof(bytes)
            );
        }
        _bytes = bytes.ToArray();
    }

    public ReadOnlySpan<byte> Span => _bytes ?? Zero;

    public bool Equals(DestinationHash other) => Span.SequenceEqual(other.Span);

    public override bool Equals(object? value) => value is DestinationHash other && Equals(other);

    public override int GetHashCode()
    {
        var hash = new HashCode();
        foreach (var value in Span)
        {
            hash.Add(value);
        }
        return hash.ToHashCode();
    }

    public static bool operator ==(DestinationHash left, DestinationHash right) => left.Equals(right);
    public static bool operator !=(DestinationHash left, DestinationHash right) => !left.Equals(right);
}

public readonly struct IdentityHash : IEquatable<IdentityHash>
{
    private static readonly byte[] Zero = new byte[HostContract.IdentityHashLength];
    private readonly byte[]? _bytes;

    public IdentityHash(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length != HostContract.IdentityHashLength)
        {
            throw new ArgumentException(
                $"Expected exactly {HostContract.IdentityHashLength} bytes.",
                nameof(bytes)
            );
        }
        _bytes = bytes.ToArray();
    }

    public ReadOnlySpan<byte> Span => _bytes ?? Zero;

    public bool Equals(IdentityHash other) => Span.SequenceEqual(other.Span);

    public override bool Equals(object? value) => value is IdentityHash other && Equals(other);

    public override int GetHashCode()
    {
        var hash = new HashCode();
        foreach (var value in Span)
        {
            hash.Add(value);
        }
        return hash.ToHashCode();
    }

    public static bool operator ==(IdentityHash left, IdentityHash right) => left.Equals(right);
    public static bool operator !=(IdentityHash left, IdentityHash right) => !left.Equals(right);
}

public readonly struct InterfaceId : IEquatable<InterfaceId>
{
    private static readonly byte[] Zero = new byte[HostContract.InterfaceIdLength];
    private readonly byte[]? _bytes;

    public InterfaceId(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length != HostContract.InterfaceIdLength)
        {
            throw new ArgumentException(
                $"Expected exactly {HostContract.InterfaceIdLength} bytes.",
                nameof(bytes)
            );
        }
        _bytes = bytes.ToArray();
    }

    public ReadOnlySpan<byte> Span => _bytes ?? Zero;

    public bool Equals(InterfaceId other) => Span.SequenceEqual(other.Span);

    public override bool Equals(object? value) => value is InterfaceId other && Equals(other);

    public override int GetHashCode()
    {
        var hash = new HashCode();
        foreach (var value in Span)
        {
            hash.Add(value);
        }
        return hash.ToHashCode();
    }

    public static bool operator ==(InterfaceId left, InterfaceId right) => left.Equals(right);
    public static bool operator !=(InterfaceId left, InterfaceId right) => !left.Equals(right);
}

public readonly struct LinkId : IEquatable<LinkId>
{
    private static readonly byte[] Zero = new byte[HostContract.LinkIdLength];
    private readonly byte[]? _bytes;

    public LinkId(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length != HostContract.LinkIdLength)
        {
            throw new ArgumentException(
                $"Expected exactly {HostContract.LinkIdLength} bytes.",
                nameof(bytes)
            );
        }
        _bytes = bytes.ToArray();
    }

    public ReadOnlySpan<byte> Span => _bytes ?? Zero;

    public bool Equals(LinkId other) => Span.SequenceEqual(other.Span);

    public override bool Equals(object? value) => value is LinkId other && Equals(other);

    public override int GetHashCode()
    {
        var hash = new HashCode();
        foreach (var value in Span)
        {
            hash.Add(value);
        }
        return hash.ToHashCode();
    }

    public static bool operator ==(LinkId left, LinkId right) => left.Equals(right);
    public static bool operator !=(LinkId left, LinkId right) => !left.Equals(right);
}

public readonly struct PacketHash : IEquatable<PacketHash>
{
    private static readonly byte[] Zero = new byte[HostContract.PacketHashLength];
    private readonly byte[]? _bytes;

    public PacketHash(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length != HostContract.PacketHashLength)
        {
            throw new ArgumentException(
                $"Expected exactly {HostContract.PacketHashLength} bytes.",
                nameof(bytes)
            );
        }
        _bytes = bytes.ToArray();
    }

    public ReadOnlySpan<byte> Span => _bytes ?? Zero;

    public bool Equals(PacketHash other) => Span.SequenceEqual(other.Span);

    public override bool Equals(object? value) => value is PacketHash other && Equals(other);

    public override int GetHashCode()
    {
        var hash = new HashCode();
        foreach (var value in Span)
        {
            hash.Add(value);
        }
        return hash.ToHashCode();
    }

    public static bool operator ==(PacketHash left, PacketHash right) => left.Equals(right);
    public static bool operator !=(PacketHash left, PacketHash right) => !left.Equals(right);
}

public readonly struct RequestId : IEquatable<RequestId>
{
    private static readonly byte[] Zero = new byte[HostContract.RequestIdLength];
    private readonly byte[]? _bytes;

    public RequestId(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length != HostContract.RequestIdLength)
        {
            throw new ArgumentException(
                $"Expected exactly {HostContract.RequestIdLength} bytes.",
                nameof(bytes)
            );
        }
        _bytes = bytes.ToArray();
    }

    public ReadOnlySpan<byte> Span => _bytes ?? Zero;

    public bool Equals(RequestId other) => Span.SequenceEqual(other.Span);

    public override bool Equals(object? value) => value is RequestId other && Equals(other);

    public override int GetHashCode()
    {
        var hash = new HashCode();
        foreach (var value in Span)
        {
            hash.Add(value);
        }
        return hash.ToHashCode();
    }

    public static bool operator ==(RequestId left, RequestId right) => left.Equals(right);
    public static bool operator !=(RequestId left, RequestId right) => !left.Equals(right);
}

public readonly struct RequestPathHash : IEquatable<RequestPathHash>
{
    private static readonly byte[] Zero = new byte[HostContract.RequestPathHashLength];
    private readonly byte[]? _bytes;

    public RequestPathHash(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length != HostContract.RequestPathHashLength)
        {
            throw new ArgumentException(
                $"Expected exactly {HostContract.RequestPathHashLength} bytes.",
                nameof(bytes)
            );
        }
        _bytes = bytes.ToArray();
    }

    public ReadOnlySpan<byte> Span => _bytes ?? Zero;

    public bool Equals(RequestPathHash other) => Span.SequenceEqual(other.Span);

    public override bool Equals(object? value) => value is RequestPathHash other && Equals(other);

    public override int GetHashCode()
    {
        var hash = new HashCode();
        foreach (var value in Span)
        {
            hash.Add(value);
        }
        return hash.ToHashCode();
    }

    public static bool operator ==(RequestPathHash left, RequestPathHash right) => left.Equals(right);
    public static bool operator !=(RequestPathHash left, RequestPathHash right) => !left.Equals(right);
}

public readonly struct ResourceHash : IEquatable<ResourceHash>
{
    private static readonly byte[] Zero = new byte[HostContract.ResourceHashLength];
    private readonly byte[]? _bytes;

    public ResourceHash(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length != HostContract.ResourceHashLength)
        {
            throw new ArgumentException(
                $"Expected exactly {HostContract.ResourceHashLength} bytes.",
                nameof(bytes)
            );
        }
        _bytes = bytes.ToArray();
    }

    public ReadOnlySpan<byte> Span => _bytes ?? Zero;

    public bool Equals(ResourceHash other) => Span.SequenceEqual(other.Span);

    public override bool Equals(object? value) => value is ResourceHash other && Equals(other);

    public override int GetHashCode()
    {
        var hash = new HashCode();
        foreach (var value in Span)
        {
            hash.Add(value);
        }
        return hash.ToHashCode();
    }

    public static bool operator ==(ResourceHash left, ResourceHash right) => left.Equals(right);
    public static bool operator !=(ResourceHash left, ResourceHash right) => !left.Equals(right);
}

public sealed class IdentitySecret : IDisposable
{
    private byte[]? _bytes;

    public IdentitySecret(ReadOnlySpan<byte> bytes)
    {
        if (bytes.Length != HostContract.IdentitySecretLength)
        {
            throw new ArgumentException(
                $"Expected exactly {HostContract.IdentitySecretLength} bytes.",
                nameof(bytes)
            );
        }
        _bytes = bytes.ToArray();
    }

    public ReadOnlySpan<byte> Span => _bytes ?? throw new ObjectDisposedException(GetType().Name);

    ~IdentitySecret()
    {
        Dispose();
    }

    public void Dispose()
    {
        var bytes = Interlocked.Exchange(ref _bytes, null);
        if (bytes is not null)
        {
            System.Security.Cryptography.CryptographicOperations.ZeroMemory(bytes);
        }
        GC.SuppressFinalize(this);
    }
}

public sealed record DestinationName(string AppName, ImmutableArray<string> Aspects);

public sealed record RequestHandlerConfig(string Path, RequestPolicy Policy);

public sealed record SerialLineConfig(uint Baud, SerialDataBits DataBits, SerialParity Parity, SerialStopBits StopBits);

public sealed record RNodeRadioConfig(ulong FrequencyHz, uint BandwidthHz, short TxPowerDbm, byte SpreadingFactor, byte CodingRate);

public sealed record MultiRNodeMemberConfig(string Name, byte VirtualPort, RNodeRadioConfig Radio, bool FlowControl, bool Outgoing);

public sealed record InterfaceRoutingPolicy(InterfaceMode? Mode, long? Gravity, bool? RecursivePathRequests, bool? AnnouncesFromInternal, bool? AnnouncesToInternal);

public sealed record BackendInfo(BackendKind Backend, ImmutableArray<Capability> Capabilities, ImmutableArray<InterfaceKind> InterfaceKinds);

public sealed record InterfaceSnapshot(InterfaceId InterfaceId, string? Name, InterfaceKind? Kind, InterfaceHealth Health, string? FailureDetail, ulong RxBytes, ulong TxBytes, ulong? RxBps, ulong? TxBps, uint RouteCount, uint LinkCount, uint TransportedLinkCount);

public sealed record RouteSnapshot(DestinationHash Destination, byte Hops, IdentityHash? ViaIdentity, InterfaceId InterfaceId, ulong LearnedAtMillis, ulong LastRouteActivityAtMillis, ulong ExpiresAtMillis);

public sealed record DestinationIdentitySnapshot(DestinationHash Destination, IdentityHash Identity);

public sealed record RuntimeHealthSnapshot(bool Running, ulong UptimeMillis, uint InterfaceCount, uint OnlineInterfaceCount, uint RouteCount, uint LinkCount, uint TransportedLinkCount, ulong RxBytes, ulong TxBytes, ulong RxBps, ulong TxBps);

public sealed record PersistenceSnapshot(bool Persistent, bool Restored, PersistenceFlushCause? LastFlushCause, string? LastFailureDetail);

public sealed record HostSnapshot(ulong Revision, BackendInfo Backend, ImmutableArray<InterfaceSnapshot> Interfaces, ImmutableArray<RouteSnapshot> Routes, uint ActiveLinkCount, ImmutableArray<DestinationIdentitySnapshot> DestinationIdentities, RuntimeHealthSnapshot Runtime, PersistenceSnapshot Persistence);

public abstract record IdentityConfig
{
    private protected IdentityConfig() { }

    public sealed record Existing(
        IdentitySecret Secret
    ) : IdentityConfig;
    public sealed record GenerateEphemeral() : IdentityConfig;
    public sealed record LoadOrCreate(
        string Path
    ) : IdentityConfig;

    public TResult Match<TResult>(
        Func<IdentityConfig.Existing, TResult> existing,
        Func<IdentityConfig.GenerateEphemeral, TResult> generateEphemeral,
        Func<IdentityConfig.LoadOrCreate, TResult> loadOrCreate
    ) =>
        this switch
        {
            Existing value => existing(value),
            GenerateEphemeral value => generateEphemeral(value),
            LoadOrCreate value => loadOrCreate(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record PersistenceConfig
{
    private protected PersistenceConfig() { }

    public sealed record Ephemeral() : PersistenceConfig;
    public sealed record Directory(
        string Path
    ) : PersistenceConfig;

    public TResult Match<TResult>(
        Func<PersistenceConfig.Ephemeral, TResult> ephemeral,
        Func<PersistenceConfig.Directory, TResult> directory
    ) =>
        this switch
        {
            Ephemeral value => ephemeral(value),
            Directory value => directory(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record InterfaceConfig
{
    private protected InterfaceConfig() { }

    public sealed record AutoLan(
        string? GroupId,
        DiscoveryScope? DiscoveryScope,
        ushort? DiscoveryPort,
        ushort? DataPort,
        ImmutableArray<string> Devices,
        ImmutableArray<string> IgnoredDevices,
        MulticastAddressType? MulticastAddressType
    ) : InterfaceConfig;
    public sealed record TcpClient(
        string Target,
        Bitrate Bitrate
    ) : InterfaceConfig;
    public sealed record TcpServer(
        string Bind,
        Bitrate Bitrate
    ) : InterfaceConfig;
    public sealed record Udp(
        string Local,
        string Peer,
        Bitrate Bitrate
    ) : InterfaceConfig;
    public sealed record Serial(
        string Port,
        SerialLineConfig Line
    ) : InterfaceConfig;
    public sealed record Kiss(
        string Port,
        SerialLineConfig Line,
        bool FlowControl,
        uint PreambleMillis,
        uint TransmitTailMillis,
        byte Persistence,
        uint SlotTimeMillis,
        string? StationCallsign,
        ulong? StationIntervalSeconds
    ) : InterfaceConfig;
    public sealed record Ax25Kiss(
        string Port,
        SerialLineConfig Line,
        bool FlowControl,
        uint PreambleMillis,
        uint TransmitTailMillis,
        byte Persistence,
        uint SlotTimeMillis,
        string Callsign,
        byte Ssid
    ) : InterfaceConfig;
    public sealed record RNode(
        string Port,
        RNodeRadioConfig Radio,
        bool FlowControl,
        string? StationCallsign,
        ulong? StationIntervalSeconds,
        ushort? AirtimeLimitShortCentiPercent,
        ushort? AirtimeLimitLongCentiPercent
    ) : InterfaceConfig;
    public sealed record MultiRNode(
        string Port,
        string? StationCallsign,
        ulong? StationIntervalSeconds,
        ImmutableArray<MultiRNodeMemberConfig> Members
    ) : InterfaceConfig;
    public sealed record Pipe(
        ImmutableArray<string> Command,
        ulong RespawnDelayMillis
    ) : InterfaceConfig;
    public sealed record BackboneClient(
        string Target,
        Bitrate Bitrate
    ) : InterfaceConfig;
    public sealed record BackboneServer(
        string Bind,
        Bitrate Bitrate
    ) : InterfaceConfig;
    public sealed record I2p(
        ImmutableArray<string> Peers,
        bool Connectable
    ) : InterfaceConfig;
    public sealed record Weave(
        string Port
    ) : InterfaceConfig;
    public sealed record AutomaticUsb() : InterfaceConfig;
    public sealed record AutomaticBluetoothLe() : InterfaceConfig;
    public sealed record WebSocketClient(
        string Target,
        WebSocketFramingSelection Framing
    ) : InterfaceConfig;
    public sealed record WebSocketServer(
        string Bind,
        WebSocketFramingSelection Framing
    ) : InterfaceConfig;
    public sealed record BrowserRendezvous(
        string Url
    ) : InterfaceConfig;

    public TResult Match<TResult>(
        Func<InterfaceConfig.AutoLan, TResult> autoLan,
        Func<InterfaceConfig.TcpClient, TResult> tcpClient,
        Func<InterfaceConfig.TcpServer, TResult> tcpServer,
        Func<InterfaceConfig.Udp, TResult> udp,
        Func<InterfaceConfig.Serial, TResult> serial,
        Func<InterfaceConfig.Kiss, TResult> kiss,
        Func<InterfaceConfig.Ax25Kiss, TResult> ax25Kiss,
        Func<InterfaceConfig.RNode, TResult> rNode,
        Func<InterfaceConfig.MultiRNode, TResult> multiRNode,
        Func<InterfaceConfig.Pipe, TResult> pipe,
        Func<InterfaceConfig.BackboneClient, TResult> backboneClient,
        Func<InterfaceConfig.BackboneServer, TResult> backboneServer,
        Func<InterfaceConfig.I2p, TResult> i2p,
        Func<InterfaceConfig.Weave, TResult> weave,
        Func<InterfaceConfig.AutomaticUsb, TResult> automaticUsb,
        Func<InterfaceConfig.AutomaticBluetoothLe, TResult> automaticBluetoothLe,
        Func<InterfaceConfig.WebSocketClient, TResult> webSocketClient,
        Func<InterfaceConfig.WebSocketServer, TResult> webSocketServer,
        Func<InterfaceConfig.BrowserRendezvous, TResult> browserRendezvous
    ) =>
        this switch
        {
            AutoLan value => autoLan(value),
            TcpClient value => tcpClient(value),
            TcpServer value => tcpServer(value),
            Udp value => udp(value),
            Serial value => serial(value),
            Kiss value => kiss(value),
            Ax25Kiss value => ax25Kiss(value),
            RNode value => rNode(value),
            MultiRNode value => multiRNode(value),
            Pipe value => pipe(value),
            BackboneClient value => backboneClient(value),
            BackboneServer value => backboneServer(value),
            I2p value => i2p(value),
            Weave value => weave(value),
            AutomaticUsb value => automaticUsb(value),
            AutomaticBluetoothLe value => automaticBluetoothLe(value),
            WebSocketClient value => webSocketClient(value),
            WebSocketServer value => webSocketServer(value),
            BrowserRendezvous value => browserRendezvous(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record DestinationIdentityConfig
{
    private protected DestinationIdentityConfig() { }

    public sealed record HostIdentity() : DestinationIdentityConfig;
    public sealed record DedicatedIdentity(
        IdentityConfig Identity
    ) : DestinationIdentityConfig;

    public TResult Match<TResult>(
        Func<DestinationIdentityConfig.HostIdentity, TResult> hostIdentity,
        Func<DestinationIdentityConfig.DedicatedIdentity, TResult> dedicatedIdentity
    ) =>
        this switch
        {
            HostIdentity value => hostIdentity(value),
            DedicatedIdentity value => dedicatedIdentity(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record Bitrate
{
    private protected Bitrate() { }

    public sealed record Auto() : Bitrate;
    public sealed record BitsPerSecond(
        ulong Value
    ) : Bitrate;

    public TResult Match<TResult>(
        Func<Bitrate.Auto, TResult> auto,
        Func<Bitrate.BitsPerSecond, TResult> bitsPerSecond
    ) =>
        this switch
        {
            Auto value => auto(value),
            BitsPerSecond value => bitsPerSecond(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record ResponseTimeout
{
    private protected ResponseTimeout() { }

    public sealed record LinkDefault() : ResponseTimeout;
    public sealed record Exact(
        ulong Millis
    ) : ResponseTimeout;

    public TResult Match<TResult>(
        Func<ResponseTimeout.LinkDefault, TResult> linkDefault,
        Func<ResponseTimeout.Exact, TResult> exact
    ) =>
        this switch
        {
            LinkDefault value => linkDefault(value),
            Exact value => exact(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record ResourceCompression
{
    private protected ResourceCompression() { }

    public sealed record Auto() : ResourceCompression;
    public sealed record Never() : ResourceCompression;

    public TResult Match<TResult>(
        Func<ResourceCompression.Auto, TResult> auto,
        Func<ResourceCompression.Never, TResult> never
    ) =>
        this switch
        {
            Auto value => auto(value),
            Never value => never(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record ResourceStrategy
{
    private protected ResourceStrategy() { }

    public sealed record Refuse() : ResourceStrategy;
    public sealed record Accept(
        ulong MaximumUncompressedBytes,
        bool AcceptCompressed
    ) : ResourceStrategy;

    public TResult Match<TResult>(
        Func<ResourceStrategy.Refuse, TResult> refuse,
        Func<ResourceStrategy.Accept, TResult> accept
    ) =>
        this switch
        {
            Refuse value => refuse(value),
            Accept value => accept(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record DestinationConfig
{
    private protected DestinationConfig() { }

    public sealed record Plain(
        DestinationName Name
    ) : DestinationConfig;
    public sealed record Single(
        DestinationName Name,
        DestinationIdentityConfig Identity,
        ReadOnlyMemory<byte>? AnnounceAppData,
        ulong? MaximumRequestBytes,
        ImmutableArray<RequestHandlerConfig> RequestHandlers
    ) : DestinationConfig;

    public TResult Match<TResult>(
        Func<DestinationConfig.Plain, TResult> plain,
        Func<DestinationConfig.Single, TResult> single
    ) =>
        this switch
        {
            Plain value => plain(value),
            Single value => single(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record HostCommand
{
    private protected HostCommand() { }

    public sealed record Announce(
        DestinationHash Destination,
        InterfaceId? Interface
    ) : HostCommand;
    public sealed record SendSinglePacket(
        DestinationHash Destination,
        ReadOnlyMemory<byte> Payload
    ) : HostCommand;
    public sealed record CloseLink(
        LinkId LinkId
    ) : HostCommand;
    public sealed record AttachTcpServer(
        string Bind,
        Bitrate Bitrate
    ) : HostCommand;
    public sealed record AttachTcpClient(
        string Target,
        Bitrate Bitrate
    ) : HostCommand;
    public sealed record AttachUdp(
        string Local,
        string Peer,
        Bitrate Bitrate
    ) : HostCommand;
    public sealed record DetachInterface(
        InterfaceId Interface
    ) : HostCommand;
    public sealed record EstablishLink(
        DestinationHash Destination
    ) : HostCommand;
    public sealed record RequestPath(
        DestinationHash Destination
    ) : HostCommand;
    public sealed record Identify(
        LinkId LinkId,
        IdentityHash Identity
    ) : HostCommand;
    public sealed record SendLinkPacket(
        LinkId LinkId,
        ReadOnlyMemory<byte> Payload
    ) : HostCommand;
    public sealed record Request(
        LinkId LinkId,
        RequestPathHash PathHash,
        ReadOnlyMemory<byte> Payload,
        ResponseTimeout Timeout,
        ulong? MaximumResponseBytes
    ) : HostCommand;
    public sealed record Respond(
        LinkId LinkId,
        RequestId RequestId,
        ulong RequestRttMillis,
        ReadOnlyMemory<byte> Payload
    ) : HostCommand;
    public sealed record SendResource(
        LinkId LinkId,
        ReadOnlyMemory<byte> Payload,
        ReadOnlyMemory<byte>? PackedMetadata,
        ResourceCompression Compression
    ) : HostCommand;
    public sealed record SetLinkResourceStrategy(
        LinkId LinkId,
        ResourceStrategy Strategy
    ) : HostCommand;
    public sealed record SetDestinationResourceStrategy(
        DestinationHash Destination,
        ResourceStrategy Strategy
    ) : HostCommand;
    public sealed record SendChannelMessage(
        LinkId LinkId,
        ushort MessageType,
        ReadOnlyMemory<byte> Payload
    ) : HostCommand;
    public sealed record AllowRequester(
        DestinationHash Destination,
        RequestPathHash PathHash,
        IdentityHash Identity
    ) : HostCommand;
    public sealed record AttachInterface(
        InterfaceConfig Config,
        InterfaceRoutingPolicy? Routing
    ) : HostCommand;

    public TResult Match<TResult>(
        Func<HostCommand.Announce, TResult> announce,
        Func<HostCommand.SendSinglePacket, TResult> sendSinglePacket,
        Func<HostCommand.CloseLink, TResult> closeLink,
        Func<HostCommand.AttachTcpServer, TResult> attachTcpServer,
        Func<HostCommand.AttachTcpClient, TResult> attachTcpClient,
        Func<HostCommand.AttachUdp, TResult> attachUdp,
        Func<HostCommand.DetachInterface, TResult> detachInterface,
        Func<HostCommand.EstablishLink, TResult> establishLink,
        Func<HostCommand.RequestPath, TResult> requestPath,
        Func<HostCommand.Identify, TResult> identify,
        Func<HostCommand.SendLinkPacket, TResult> sendLinkPacket,
        Func<HostCommand.Request, TResult> request,
        Func<HostCommand.Respond, TResult> respond,
        Func<HostCommand.SendResource, TResult> sendResource,
        Func<HostCommand.SetLinkResourceStrategy, TResult> setLinkResourceStrategy,
        Func<HostCommand.SetDestinationResourceStrategy, TResult> setDestinationResourceStrategy,
        Func<HostCommand.SendChannelMessage, TResult> sendChannelMessage,
        Func<HostCommand.AllowRequester, TResult> allowRequester,
        Func<HostCommand.AttachInterface, TResult> attachInterface
    ) =>
        this switch
        {
            Announce value => announce(value),
            SendSinglePacket value => sendSinglePacket(value),
            CloseLink value => closeLink(value),
            AttachTcpServer value => attachTcpServer(value),
            AttachTcpClient value => attachTcpClient(value),
            AttachUdp value => attachUdp(value),
            DetachInterface value => detachInterface(value),
            EstablishLink value => establishLink(value),
            RequestPath value => requestPath(value),
            Identify value => identify(value),
            SendLinkPacket value => sendLinkPacket(value),
            Request value => request(value),
            Respond value => respond(value),
            SendResource value => sendResource(value),
            SetLinkResourceStrategy value => setLinkResourceStrategy(value),
            SetDestinationResourceStrategy value => setDestinationResourceStrategy(value),
            SendChannelMessage value => sendChannelMessage(value),
            AllowRequester value => allowRequester(value),
            AttachInterface value => attachInterface(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record CommandOutcome
{
    private protected CommandOutcome() { }

    public sealed record Announced() : CommandOutcome;
    public sealed record PacketDelivered(
        ulong RttMillis,
        DeliveryEvidenceKind Evidence,
        PacketHash? PacketHash
    ) : CommandOutcome;
    public sealed record LinkCloseQueued() : CommandOutcome;
    public sealed record InterfaceAttached(
        InterfaceId Interface
    ) : CommandOutcome;
    public sealed record InterfaceDetached(
        InterfaceId Interface
    ) : CommandOutcome;
    public sealed record LinkEstablished(
        LinkId LinkId,
        ulong RttMillis
    ) : CommandOutcome;
    public sealed record PathDiscovered(
        byte Hops
    ) : CommandOutcome;
    public sealed record Identified() : CommandOutcome;
    public sealed record ResponseReceived(
        ReadOnlyMemory<byte> Data,
        ulong RttMillis
    ) : CommandOutcome;
    public sealed record ResponseSent(
        ulong RttMillis
    ) : CommandOutcome;
    public sealed record ResourceSent() : CommandOutcome;
    public sealed record ResourceStrategySet() : CommandOutcome;
    public sealed record RequesterAllowed() : CommandOutcome;

    public TResult Match<TResult>(
        Func<CommandOutcome.Announced, TResult> announced,
        Func<CommandOutcome.PacketDelivered, TResult> packetDelivered,
        Func<CommandOutcome.LinkCloseQueued, TResult> linkCloseQueued,
        Func<CommandOutcome.InterfaceAttached, TResult> interfaceAttached,
        Func<CommandOutcome.InterfaceDetached, TResult> interfaceDetached,
        Func<CommandOutcome.LinkEstablished, TResult> linkEstablished,
        Func<CommandOutcome.PathDiscovered, TResult> pathDiscovered,
        Func<CommandOutcome.Identified, TResult> identified,
        Func<CommandOutcome.ResponseReceived, TResult> responseReceived,
        Func<CommandOutcome.ResponseSent, TResult> responseSent,
        Func<CommandOutcome.ResourceSent, TResult> resourceSent,
        Func<CommandOutcome.ResourceStrategySet, TResult> resourceStrategySet,
        Func<CommandOutcome.RequesterAllowed, TResult> requesterAllowed
    ) =>
        this switch
        {
            Announced value => announced(value),
            PacketDelivered value => packetDelivered(value),
            LinkCloseQueued value => linkCloseQueued(value),
            InterfaceAttached value => interfaceAttached(value),
            InterfaceDetached value => interfaceDetached(value),
            LinkEstablished value => linkEstablished(value),
            PathDiscovered value => pathDiscovered(value),
            Identified value => identified(value),
            ResponseReceived value => responseReceived(value),
            ResponseSent value => responseSent(value),
            ResourceSent value => resourceSent(value),
            ResourceStrategySet value => resourceStrategySet(value),
            RequesterAllowed value => requesterAllowed(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record CommandFailure
{
    private protected CommandFailure() { }

    public sealed record NodeStopped() : CommandFailure;
    public sealed record Busy() : CommandFailure;
    public sealed record PayloadTooLarge() : CommandFailure;
    public sealed record UnknownDestination() : CommandFailure;
    public sealed record NotSingleDestination() : CommandFailure;
    public sealed record AnnounceAppDataTooLong() : CommandFailure;
    public sealed record UnknownInterface() : CommandFailure;
    public sealed record NoRouteToDestination() : CommandFailure;
    public sealed record NotDirectlyReachable() : CommandFailure;
    public sealed record PacketCulled() : CommandFailure;
    public sealed record DeliveryTimedOut() : CommandFailure;
    public sealed record InvalidBitrate() : CommandFailure;
    public sealed record BindFailed(
        string Detail
    ) : CommandFailure;
    public sealed record WriteFailed(
        string Detail
    ) : CommandFailure;
    public sealed record UnsupportedByBackend() : CommandFailure;
    public sealed record UnknownLink() : CommandFailure;
    public sealed record LinkNotActive() : CommandFailure;
    public sealed record EntropyUnavailable() : CommandFailure;
    public sealed record NotLinkInitiator() : CommandFailure;
    public sealed record IdentityNotHeld() : CommandFailure;
    public sealed record UnknownRequestHandler() : CommandFailure;
    public sealed record RequestPolicyNotAllowList() : CommandFailure;
    public sealed record RequestAllowListFull() : CommandFailure;
    public sealed record LinkBusy() : CommandFailure;
    public sealed record ResourceTableFull() : CommandFailure;
    public sealed record ResourceMetadataTooLarge() : CommandFailure;
    public sealed record ResourceRejectedByPeer() : CommandFailure;
    public sealed record ResourceSequencingFailed() : CommandFailure;
    public sealed record ResourcePredecessorFailed() : CommandFailure;
    public sealed record ChannelWindowFull() : CommandFailure;
    public sealed record ChannelUntrackable() : CommandFailure;
    public sealed record InvalidChannelMessageType() : CommandFailure;
    public sealed record InvalidConfiguration(
        string Detail
    ) : CommandFailure;
    public sealed record ResourceUploadCancelled() : CommandFailure;
    public sealed record ResourceEarlyEof() : CommandFailure;
    public sealed record ResourceLengthOverrun() : CommandFailure;
    public sealed record PermissionDenied(
        string Detail
    ) : CommandFailure;
    public sealed record DeviceUnavailable(
        string Detail
    ) : CommandFailure;
    public sealed record ConnectFailed(
        string Detail
    ) : CommandFailure;
    public sealed record BackendFailed(
        string Detail
    ) : CommandFailure;
    public sealed record ResponseTooLarge() : CommandFailure;

    public TResult Match<TResult>(
        Func<CommandFailure.NodeStopped, TResult> nodeStopped,
        Func<CommandFailure.Busy, TResult> busy,
        Func<CommandFailure.PayloadTooLarge, TResult> payloadTooLarge,
        Func<CommandFailure.UnknownDestination, TResult> unknownDestination,
        Func<CommandFailure.NotSingleDestination, TResult> notSingleDestination,
        Func<CommandFailure.AnnounceAppDataTooLong, TResult> announceAppDataTooLong,
        Func<CommandFailure.UnknownInterface, TResult> unknownInterface,
        Func<CommandFailure.NoRouteToDestination, TResult> noRouteToDestination,
        Func<CommandFailure.NotDirectlyReachable, TResult> notDirectlyReachable,
        Func<CommandFailure.PacketCulled, TResult> packetCulled,
        Func<CommandFailure.DeliveryTimedOut, TResult> deliveryTimedOut,
        Func<CommandFailure.InvalidBitrate, TResult> invalidBitrate,
        Func<CommandFailure.BindFailed, TResult> bindFailed,
        Func<CommandFailure.WriteFailed, TResult> writeFailed,
        Func<CommandFailure.UnsupportedByBackend, TResult> unsupportedByBackend,
        Func<CommandFailure.UnknownLink, TResult> unknownLink,
        Func<CommandFailure.LinkNotActive, TResult> linkNotActive,
        Func<CommandFailure.EntropyUnavailable, TResult> entropyUnavailable,
        Func<CommandFailure.NotLinkInitiator, TResult> notLinkInitiator,
        Func<CommandFailure.IdentityNotHeld, TResult> identityNotHeld,
        Func<CommandFailure.UnknownRequestHandler, TResult> unknownRequestHandler,
        Func<CommandFailure.RequestPolicyNotAllowList, TResult> requestPolicyNotAllowList,
        Func<CommandFailure.RequestAllowListFull, TResult> requestAllowListFull,
        Func<CommandFailure.LinkBusy, TResult> linkBusy,
        Func<CommandFailure.ResourceTableFull, TResult> resourceTableFull,
        Func<CommandFailure.ResourceMetadataTooLarge, TResult> resourceMetadataTooLarge,
        Func<CommandFailure.ResourceRejectedByPeer, TResult> resourceRejectedByPeer,
        Func<CommandFailure.ResourceSequencingFailed, TResult> resourceSequencingFailed,
        Func<CommandFailure.ResourcePredecessorFailed, TResult> resourcePredecessorFailed,
        Func<CommandFailure.ChannelWindowFull, TResult> channelWindowFull,
        Func<CommandFailure.ChannelUntrackable, TResult> channelUntrackable,
        Func<CommandFailure.InvalidChannelMessageType, TResult> invalidChannelMessageType,
        Func<CommandFailure.InvalidConfiguration, TResult> invalidConfiguration,
        Func<CommandFailure.ResourceUploadCancelled, TResult> resourceUploadCancelled,
        Func<CommandFailure.ResourceEarlyEof, TResult> resourceEarlyEof,
        Func<CommandFailure.ResourceLengthOverrun, TResult> resourceLengthOverrun,
        Func<CommandFailure.PermissionDenied, TResult> permissionDenied,
        Func<CommandFailure.DeviceUnavailable, TResult> deviceUnavailable,
        Func<CommandFailure.ConnectFailed, TResult> connectFailed,
        Func<CommandFailure.BackendFailed, TResult> backendFailed,
        Func<CommandFailure.ResponseTooLarge, TResult> responseTooLarge
    ) =>
        this switch
        {
            NodeStopped value => nodeStopped(value),
            Busy value => busy(value),
            PayloadTooLarge value => payloadTooLarge(value),
            UnknownDestination value => unknownDestination(value),
            NotSingleDestination value => notSingleDestination(value),
            AnnounceAppDataTooLong value => announceAppDataTooLong(value),
            UnknownInterface value => unknownInterface(value),
            NoRouteToDestination value => noRouteToDestination(value),
            NotDirectlyReachable value => notDirectlyReachable(value),
            PacketCulled value => packetCulled(value),
            DeliveryTimedOut value => deliveryTimedOut(value),
            InvalidBitrate value => invalidBitrate(value),
            BindFailed value => bindFailed(value),
            WriteFailed value => writeFailed(value),
            UnsupportedByBackend value => unsupportedByBackend(value),
            UnknownLink value => unknownLink(value),
            LinkNotActive value => linkNotActive(value),
            EntropyUnavailable value => entropyUnavailable(value),
            NotLinkInitiator value => notLinkInitiator(value),
            IdentityNotHeld value => identityNotHeld(value),
            UnknownRequestHandler value => unknownRequestHandler(value),
            RequestPolicyNotAllowList value => requestPolicyNotAllowList(value),
            RequestAllowListFull value => requestAllowListFull(value),
            LinkBusy value => linkBusy(value),
            ResourceTableFull value => resourceTableFull(value),
            ResourceMetadataTooLarge value => resourceMetadataTooLarge(value),
            ResourceRejectedByPeer value => resourceRejectedByPeer(value),
            ResourceSequencingFailed value => resourceSequencingFailed(value),
            ResourcePredecessorFailed value => resourcePredecessorFailed(value),
            ChannelWindowFull value => channelWindowFull(value),
            ChannelUntrackable value => channelUntrackable(value),
            InvalidChannelMessageType value => invalidChannelMessageType(value),
            InvalidConfiguration value => invalidConfiguration(value),
            ResourceUploadCancelled value => resourceUploadCancelled(value),
            ResourceEarlyEof value => resourceEarlyEof(value),
            ResourceLengthOverrun value => resourceLengthOverrun(value),
            PermissionDenied value => permissionDenied(value),
            DeviceUnavailable value => deviceUnavailable(value),
            ConnectFailed value => connectFailed(value),
            BackendFailed value => backendFailed(value),
            ResponseTooLarge value => responseTooLarge(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record ApplicationEvent
{
    private protected ApplicationEvent() { }

    public sealed record SingleDelivery(
        DestinationHash Destination,
        InterfaceId SourceInterface,
        ReadOnlyMemory<byte> Plaintext
    ) : ApplicationEvent;
    public sealed record Request(
        DestinationHash Destination,
        LinkId LinkId,
        RequestId RequestId,
        IdentityHash? Requester,
        RequestPathHash PathHash,
        ulong RttMillis,
        ReadOnlyMemory<byte> Data
    ) : ApplicationEvent;
    public sealed record Response(
        LinkId LinkId,
        RequestId RequestId,
        ReadOnlyMemory<byte> Data
    ) : ApplicationEvent;
    public sealed record ResponseSegment(
        LinkId LinkId,
        RequestId RequestId,
        ulong SegmentIndex,
        ulong TotalSegments,
        ReadOnlyMemory<byte> Data
    ) : ApplicationEvent;
    public sealed record ResourceAvailable(
        LinkId LinkId,
        ResourceHash Hash,
        ReadOnlyMemory<byte>? Metadata,
        ResourceStream Resource
    ) : ApplicationEvent;
    public sealed record ResourceSegment(
        LinkId LinkId,
        ResourceHash OriginalHash,
        ulong SegmentIndex,
        ulong TotalSegments,
        ReadOnlyMemory<byte>? Metadata,
        ReadOnlyMemory<byte> Data
    ) : ApplicationEvent;
    public sealed record ResourceNeedsDecompression(
        LinkId LinkId,
        ResourceHash Hash,
        ReadOnlyMemory<byte> Stream,
        ulong UncompressedDataBytes
    ) : ApplicationEvent;
    public sealed record ChannelMessage(
        LinkId LinkId,
        ushort MessageType,
        ReadOnlyMemory<byte> Data
    ) : ApplicationEvent;
    public sealed record LinkDelivery(
        LinkId LinkId,
        InterfaceId SourceInterface,
        ReadOnlyMemory<byte> Plaintext
    ) : ApplicationEvent;

    public TResult Match<TResult>(
        Func<ApplicationEvent.SingleDelivery, TResult> singleDelivery,
        Func<ApplicationEvent.Request, TResult> request,
        Func<ApplicationEvent.Response, TResult> response,
        Func<ApplicationEvent.ResponseSegment, TResult> responseSegment,
        Func<ApplicationEvent.ResourceAvailable, TResult> resourceAvailable,
        Func<ApplicationEvent.ResourceSegment, TResult> resourceSegment,
        Func<ApplicationEvent.ResourceNeedsDecompression, TResult> resourceNeedsDecompression,
        Func<ApplicationEvent.ChannelMessage, TResult> channelMessage,
        Func<ApplicationEvent.LinkDelivery, TResult> linkDelivery
    ) =>
        this switch
        {
            SingleDelivery value => singleDelivery(value),
            Request value => request(value),
            Response value => response(value),
            ResponseSegment value => responseSegment(value),
            ResourceAvailable value => resourceAvailable(value),
            ResourceSegment value => resourceSegment(value),
            ResourceNeedsDecompression value => resourceNeedsDecompression(value),
            ChannelMessage value => channelMessage(value),
            LinkDelivery value => linkDelivery(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record DiagnosticEvent
{
    private protected DiagnosticEvent() { }

    public sealed record AnnounceHeard(
        DestinationHash Destination,
        byte Hops,
        InterfaceId SourceInterface,
        ReadOnlyMemory<byte> AppData
    ) : DiagnosticEvent;
    public sealed record LinkEstablished(
        LinkId LinkId,
        ulong RttMillis
    ) : DiagnosticEvent;
    public sealed record PeerIdentified(
        LinkId LinkId,
        IdentityHash Identity
    ) : DiagnosticEvent;
    public sealed record LinkClosed(
        LinkId LinkId,
        LinkClosedReason Reason
    ) : DiagnosticEvent;
    public sealed record LinkInterfaceMismatch(
        LinkId LinkId,
        InterfaceId AttachedInterface,
        InterfaceId ArrivedOn
    ) : DiagnosticEvent;
    public sealed record ResourceAssembled(
        LinkId LinkId,
        ResourceHash OriginalHash,
        ulong TotalSizeBytes
    ) : DiagnosticEvent;
    public sealed record ResourceFailed(
        LinkId LinkId,
        ResourceHash Hash,
        string Cause
    ) : DiagnosticEvent;
    public sealed record ResourceSendProgress(
        LinkId LinkId,
        ulong TransferredBytes,
        ulong TotalBytes,
        ulong PhysicalTransferredBytes,
        ulong SegmentIndex,
        ulong TotalSegments
    ) : DiagnosticEvent;
    public sealed record SelfRatchetRotated(
        DestinationHash Destination
    ) : DiagnosticEvent;
    public sealed record AnnounceHeldDropped(
        DestinationHash Destination,
        InterfaceId SourceInterface,
        string Cause
    ) : DiagnosticEvent;
    public sealed record Delivered(
        string Detail
    ) : DiagnosticEvent;
    public sealed record RouteExpired(
        DestinationHash Destination
    ) : DiagnosticEvent;
    public sealed record RouteEvicted(
        DestinationHash Destination
    ) : DiagnosticEvent;
    public sealed record RouteInterfaceGone(
        DestinationHash Destination
    ) : DiagnosticEvent;
    public sealed record RouteDropped(
        DestinationHash Destination
    ) : DiagnosticEvent;
    public sealed record BackendDiagnostic(
        string Kind,
        string Detail
    ) : DiagnosticEvent;
    public sealed record DiagnosticsDropped(
        UInt128 Count
    ) : DiagnosticEvent;
    public sealed record PersistenceRestored(
        ulong Routes,
        ulong DestinationIdentities,
        ulong Tunnels,
        ulong Ratchets,
        ulong Refused,
        ulong Dropped
    ) : DiagnosticEvent;
    public sealed record PersistenceFlushed(
        PersistenceFlushCause Cause,
        PersistenceFlushTarget Target
    ) : DiagnosticEvent;
    public sealed record PersistenceFlushFailed(
        PersistenceFlushCause Cause,
        PersistenceFlushTarget Target
    ) : DiagnosticEvent;

    public TResult Match<TResult>(
        Func<DiagnosticEvent.AnnounceHeard, TResult> announceHeard,
        Func<DiagnosticEvent.LinkEstablished, TResult> linkEstablished,
        Func<DiagnosticEvent.PeerIdentified, TResult> peerIdentified,
        Func<DiagnosticEvent.LinkClosed, TResult> linkClosed,
        Func<DiagnosticEvent.LinkInterfaceMismatch, TResult> linkInterfaceMismatch,
        Func<DiagnosticEvent.ResourceAssembled, TResult> resourceAssembled,
        Func<DiagnosticEvent.ResourceFailed, TResult> resourceFailed,
        Func<DiagnosticEvent.ResourceSendProgress, TResult> resourceSendProgress,
        Func<DiagnosticEvent.SelfRatchetRotated, TResult> selfRatchetRotated,
        Func<DiagnosticEvent.AnnounceHeldDropped, TResult> announceHeldDropped,
        Func<DiagnosticEvent.Delivered, TResult> delivered,
        Func<DiagnosticEvent.RouteExpired, TResult> routeExpired,
        Func<DiagnosticEvent.RouteEvicted, TResult> routeEvicted,
        Func<DiagnosticEvent.RouteInterfaceGone, TResult> routeInterfaceGone,
        Func<DiagnosticEvent.RouteDropped, TResult> routeDropped,
        Func<DiagnosticEvent.BackendDiagnostic, TResult> backendDiagnostic,
        Func<DiagnosticEvent.DiagnosticsDropped, TResult> diagnosticsDropped,
        Func<DiagnosticEvent.PersistenceRestored, TResult> persistenceRestored,
        Func<DiagnosticEvent.PersistenceFlushed, TResult> persistenceFlushed,
        Func<DiagnosticEvent.PersistenceFlushFailed, TResult> persistenceFlushFailed
    ) =>
        this switch
        {
            AnnounceHeard value => announceHeard(value),
            LinkEstablished value => linkEstablished(value),
            PeerIdentified value => peerIdentified(value),
            LinkClosed value => linkClosed(value),
            LinkInterfaceMismatch value => linkInterfaceMismatch(value),
            ResourceAssembled value => resourceAssembled(value),
            ResourceFailed value => resourceFailed(value),
            ResourceSendProgress value => resourceSendProgress(value),
            SelfRatchetRotated value => selfRatchetRotated(value),
            AnnounceHeldDropped value => announceHeldDropped(value),
            Delivered value => delivered(value),
            RouteExpired value => routeExpired(value),
            RouteEvicted value => routeEvicted(value),
            RouteInterfaceGone value => routeInterfaceGone(value),
            RouteDropped value => routeDropped(value),
            BackendDiagnostic value => backendDiagnostic(value),
            DiagnosticsDropped value => diagnosticsDropped(value),
            PersistenceRestored value => persistenceRestored(value),
            PersistenceFlushed value => persistenceFlushed(value),
            PersistenceFlushFailed value => persistenceFlushFailed(value),
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

internal static class RawHostProtocolContract
{
    internal static readonly string[] OperationNames =
    [
        "contractInfo",
        "backendInfo",
        "hostCreate",
        "hostRelease",
        "hostLifecycle",
        "hostSnapshot",
        "hostSnapshotRead",
        "hostSnapshotRelease",
        "hostIdentityHash",
        "hostDestinationCount",
        "hostDestinationHash",
        "hostAttachSuppliedPipe",
        "suppliedPipeClaimAttachment",
        "suppliedPipeNextOpenRequest",
        "suppliedPipeRegisterReadiness",
        "suppliedPipeInterruptWait",
        "suppliedPipeRelease",
        "suppliedPipeOpenRequestProvide",
        "suppliedPipeOpenRequestDecline",
        "suppliedPipeOpenRequestRelease",
        "hostBeginResourceUpload",
        "resourceUploadWrite",
        "resourceUploadIsWritable",
        "resourceUploadFinish",
        "resourceUploadAbort",
        "resourceUploadRelease",
        "hostStop",
        "commandWait",
        "commandRegisterReadiness",
        "commandInterruptWait",
        "commandRelease",
        "hostClaimApplicationEvents",
        "hostClaimDiagnostics",
        "eventStreamRegisterReadiness",
        "readinessRegistrationRelease",
        "eventStreamInterruptWait",
        "eventStreamRelease",
        "eventStreamNext",
        "eventRelease",
        "eventKind",
        "eventBytes",
        "eventString",
        "eventU64",
        "eventU128",
        "eventResourceStream",
        "resourceStreamRelease",
        "resourceStreamNext",
        "hostAnnounce",
        "hostSendSinglePacket",
        "hostCloseLink",
        "hostAttachTcpServer",
        "hostAttachTcpClient",
        "hostAttachUdp",
        "hostDetachInterface",
        "hostEstablishLink",
        "hostRequestPath",
        "hostIdentify",
        "hostSendLinkPacket",
        "hostRequest",
        "hostRespond",
        "hostSendResource",
        "hostSetLinkResourceStrategy",
        "hostSetDestinationResourceStrategy",
        "hostSendChannelMessage",
        "hostAllowRequester",
        "hostAttachInterface",
    ];
}

internal readonly record struct RawUnit;
internal readonly record struct RawOwned<T>(T Value);
internal readonly record struct RawBorrowed<T>(T Value);
internal abstract record RawCallResult<T>
{
    internal sealed record Success(T Value) : RawCallResult<T>;
    internal sealed record Failure(Status Error) : RawCallResult<T>;
}
internal interface IRawCommandResult { }
internal interface IRawContractInfo { }
internal interface IRawEvent { }
internal interface IRawEventStream { }
internal interface IRawHost { }
internal interface IRawHostInspection { }
internal interface IRawHostOptions { }
internal interface IRawIssuedCommand { }
internal interface IRawLifecycle { }
internal interface IRawReadinessCallback { }
internal interface IRawReadinessRegistration { }
internal interface IRawResourceChunk { }
internal interface IRawResourceStream { }
internal interface IRawResourceUpload { }
internal interface IRawSuppliedPipe { }
internal interface IRawSuppliedPipeOpenRequest { }
internal interface IRawOpaquePointer { }

internal interface IRawHostProtocol
{
    RawCallResult<IRawContractInfo> ContractInfo();
    RawCallResult<BackendInfo> BackendInfo();
    RawCallResult<RawOwned<IRawHost>> HostCreate(IRawHostOptions options);
    RawUnit HostRelease(IRawHost host);
    RawCallResult<IRawLifecycle> HostLifecycle(IRawHost host);
    RawCallResult<RawOwned<IRawHostInspection>> HostSnapshot(IRawHost host, uint timeoutMillis);
    RawCallResult<RawBorrowed<HostSnapshot>> HostSnapshotRead(IRawHostInspection host_inspection);
    RawUnit HostSnapshotRelease(IRawHostInspection host_inspection);
    RawCallResult<RawBorrowed<ReadOnlyMemory<byte>>> HostIdentityHash(IRawHost host);
    nuint HostDestinationCount(IRawHost host);
    RawCallResult<RawBorrowed<ReadOnlyMemory<byte>>> HostDestinationHash(IRawHost host, nuint index);
    RawCallResult<RawOwned<IRawSuppliedPipe>> HostAttachSuppliedPipe(IRawHost host, string name, ulong respawnDelayMillis, Bitrate bitrate);
    RawCallResult<RawOwned<IRawIssuedCommand>> SuppliedPipeClaimAttachment(IRawSuppliedPipe supplied_pipe);
    RawCallResult<RawOwned<IRawSuppliedPipeOpenRequest>> SuppliedPipeNextOpenRequest(IRawSuppliedPipe supplied_pipe, uint timeoutMillis);
    RawCallResult<RawOwned<IRawReadinessRegistration>> SuppliedPipeRegisterReadiness(IRawSuppliedPipe supplied_pipe, IRawReadinessCallback callback, IRawOpaquePointer context);
    RawUnit SuppliedPipeInterruptWait(IRawSuppliedPipe supplied_pipe);
    RawUnit SuppliedPipeRelease(IRawSuppliedPipe supplied_pipe);
    RawCallResult<bool> SuppliedPipeOpenRequestProvide(IRawSuppliedPipeOpenRequest supplied_pipe_open_request, long descriptor);
    RawCallResult<bool> SuppliedPipeOpenRequestDecline(IRawSuppliedPipeOpenRequest supplied_pipe_open_request);
    RawUnit SuppliedPipeOpenRequestRelease(IRawSuppliedPipeOpenRequest supplied_pipe_open_request);
    RawCallResult<RawOwned<IRawResourceUpload>> HostBeginResourceUpload(IRawHost host, LinkId linkId, ulong declaredLength, ReadOnlyMemory<byte>? packedMetadata, ResourceCompression compression);
    RawCallResult<RawUnit> ResourceUploadWrite(IRawResourceUpload resource_upload, ReadOnlyMemory<byte> chunk);
    RawCallResult<bool> ResourceUploadIsWritable(IRawResourceUpload resource_upload);
    RawCallResult<RawOwned<IRawIssuedCommand>> ResourceUploadFinish(IRawResourceUpload resource_upload);
    RawUnit ResourceUploadAbort(IRawResourceUpload resource_upload);
    RawUnit ResourceUploadRelease(IRawResourceUpload resource_upload);
    RawCallResult<RawUnit> HostStop(IRawHost host);
    RawCallResult<RawBorrowed<IRawCommandResult>> CommandWait(IRawIssuedCommand issued_command, uint timeoutMillis);
    RawCallResult<RawOwned<IRawReadinessRegistration>> CommandRegisterReadiness(IRawIssuedCommand issued_command, IRawReadinessCallback callback, IRawOpaquePointer context);
    RawUnit CommandInterruptWait(IRawIssuedCommand issued_command);
    RawUnit CommandRelease(IRawIssuedCommand issued_command);
    RawCallResult<RawOwned<IRawEventStream>> HostClaimApplicationEvents(IRawHost host);
    RawCallResult<RawOwned<IRawEventStream>> HostClaimDiagnostics(IRawHost host);
    RawCallResult<RawOwned<IRawReadinessRegistration>> EventStreamRegisterReadiness(IRawEventStream event_stream, IRawReadinessCallback callback, IRawOpaquePointer context);
    RawUnit ReadinessRegistrationRelease(IRawReadinessRegistration readiness_registration);
    RawUnit EventStreamInterruptWait(IRawEventStream event_stream);
    RawUnit EventStreamRelease(IRawEventStream event_stream);
    RawCallResult<RawOwned<IRawEvent>> EventStreamNext(IRawEventStream event_stream, uint timeoutMillis);
    RawUnit EventRelease(IRawEvent eventValue);
    uint EventKind(IRawEvent eventValue);
    RawCallResult<RawBorrowed<ReadOnlyMemory<byte>>> EventBytes(IRawEvent eventValue, EventField field);
    RawCallResult<RawBorrowed<string>> EventString(IRawEvent eventValue, EventField field);
    RawCallResult<ulong> EventU64(IRawEvent eventValue, EventField field);
    RawCallResult<UInt128> EventU128(IRawEvent eventValue, EventField field);
    RawCallResult<RawOwned<IRawResourceStream>> EventResourceStream(IRawEvent eventValue);
    RawUnit ResourceStreamRelease(IRawResourceStream resource_stream);
    RawCallResult<RawBorrowed<IRawResourceChunk>> ResourceStreamNext(IRawResourceStream resource_stream, nuint maximumBytes);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostAnnounce(IRawHost host, DestinationHash destination, InterfaceId? interfaceId);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostSendSinglePacket(IRawHost host, DestinationHash destination, ReadOnlyMemory<byte> payload);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostCloseLink(IRawHost host, LinkId linkId);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostAttachTcpServer(IRawHost host, string bind, Bitrate bitrate);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostAttachTcpClient(IRawHost host, string target, Bitrate bitrate);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostAttachUdp(IRawHost host, string local, string peer, Bitrate bitrate);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostDetachInterface(IRawHost host, InterfaceId interfaceId);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostEstablishLink(IRawHost host, DestinationHash destination);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostRequestPath(IRawHost host, DestinationHash destination);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostIdentify(IRawHost host, LinkId linkId, IdentityHash identity);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostSendLinkPacket(IRawHost host, LinkId linkId, ReadOnlyMemory<byte> payload);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostRequest(IRawHost host, LinkId linkId, RequestPathHash pathHash, ReadOnlyMemory<byte> payload, ResponseTimeout timeout, ulong? maximumResponseBytes);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostRespond(IRawHost host, LinkId linkId, RequestId requestId, ulong requestRttMillis, ReadOnlyMemory<byte> payload);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostSendResource(IRawHost host, LinkId linkId, ReadOnlyMemory<byte> payload, ReadOnlyMemory<byte>? packedMetadata, ResourceCompression compression);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostSetLinkResourceStrategy(IRawHost host, LinkId linkId, ResourceStrategy strategy);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostSetDestinationResourceStrategy(IRawHost host, DestinationHash destination, ResourceStrategy strategy);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostSendChannelMessage(IRawHost host, LinkId linkId, ushort messageType, ReadOnlyMemory<byte> payload);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostAllowRequester(IRawHost host, DestinationHash destination, RequestPathHash pathHash, IdentityHash identity);
    RawCallResult<RawOwned<IRawIssuedCommand>> HostAttachInterface(IRawHost host, InterfaceConfig config, InterfaceRoutingPolicy? routing);
}
