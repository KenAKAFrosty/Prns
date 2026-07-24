#nullable enable

using System.Collections.Immutable;

namespace PersonalRns;

public static class HostContract
{
    public const uint Abi = 1;
    public const uint SchemaVersion = 1;
    public const string ProductVersion = "0.2.8";
    public const int DestinationHashLength = 16;
    public const int IdentityHashLength = 16;
    public const int InterfaceIdLength = 8;
    public const int LinkIdLength = 16;
    public const int PacketHashLength = 32;
    public const int RequestIdLength = 16;
    public const int RequestPathHashLength = 16;
    public const int ResourceHashLength = 32;
    public const int IdentitySecretLength = 64;
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

public enum CommandOutcomeKind : uint
{
    Announced = 1,
    PacketDelivered = 2,
    LinkCloseQueued = 3,
    InterfaceAttached = 4,
    InterfaceDetached = 5,
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

public abstract record DestinationConfig
{
    private protected DestinationConfig() { }

    public sealed record Plain(
        DestinationName Name
    ) : DestinationConfig;
    public sealed record Single(
        DestinationName Name,
        DestinationIdentityConfig Identity,
        ReadOnlyMemory<byte>? AnnounceAppData
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

    public TResult Match<TResult>(
        Func<HostCommand.Announce, TResult> announce,
        Func<HostCommand.SendSinglePacket, TResult> sendSinglePacket,
        Func<HostCommand.CloseLink, TResult> closeLink,
        Func<HostCommand.AttachTcpServer, TResult> attachTcpServer,
        Func<HostCommand.AttachTcpClient, TResult> attachTcpClient,
        Func<HostCommand.AttachUdp, TResult> attachUdp,
        Func<HostCommand.DetachInterface, TResult> detachInterface
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

    public TResult Match<TResult>(
        Func<CommandOutcome.Announced, TResult> announced,
        Func<CommandOutcome.PacketDelivered, TResult> packetDelivered,
        Func<CommandOutcome.LinkCloseQueued, TResult> linkCloseQueued,
        Func<CommandOutcome.InterfaceAttached, TResult> interfaceAttached,
        Func<CommandOutcome.InterfaceDetached, TResult> interfaceDetached
    ) =>
        this switch
        {
            Announced value => announced(value),
            PacketDelivered value => packetDelivered(value),
            LinkCloseQueued value => linkCloseQueued(value),
            InterfaceAttached value => interfaceAttached(value),
            InterfaceDetached value => interfaceDetached(value),
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
        string MessageType,
        ReadOnlyMemory<byte> Data
    ) : ApplicationEvent;

    public TResult Match<TResult>(
        Func<ApplicationEvent.SingleDelivery, TResult> singleDelivery,
        Func<ApplicationEvent.Request, TResult> request,
        Func<ApplicationEvent.Response, TResult> response,
        Func<ApplicationEvent.ResponseSegment, TResult> responseSegment,
        Func<ApplicationEvent.ResourceAvailable, TResult> resourceAvailable,
        Func<ApplicationEvent.ResourceSegment, TResult> resourceSegment,
        Func<ApplicationEvent.ResourceNeedsDecompression, TResult> resourceNeedsDecompression,
        Func<ApplicationEvent.ChannelMessage, TResult> channelMessage
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
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}

public abstract record DiagnosticEvent
{
    private protected DiagnosticEvent() { }

    public sealed record AnnounceHeard(
        DestinationHash Destination,
        byte Hops,
        InterfaceId SourceInterface
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
        Func<DiagnosticEvent.DiagnosticsDropped, TResult> diagnosticsDropped
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
            _ => throw new InvalidOperationException("Unknown contract case."),
        };
}
