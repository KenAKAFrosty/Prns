using System.Collections.Immutable;
using System.Runtime.InteropServices;
using System.Text;

namespace PersonalRns;

public sealed class PrnsException : Exception
{
    public Status Status { get; }

    internal PrnsException(Status status)
        : base($"Personal RNS host operation failed with {status}.")
    {
        Status = status;
    }

    internal static void ThrowIfError(Status status)
    {
        if (status != Status.Ok)
        {
            throw new PrnsException(status);
        }
    }
}

public readonly record struct HostLimits(
    nuint PendingCommands,
    nuint ApplicationEvents,
    nuint RetainedEventBytes,
    nuint Diagnostics
)
{
    public static HostLimits Balanced =>
        new(
            (nuint)HostContract.BalancedPendingCommands,
            (nuint)HostContract.BalancedApplicationEvents,
            (nuint)HostContract.BalancedRetainedEventBytes,
            (nuint)HostContract.BalancedDiagnostics
        );
}

public readonly record struct LifecycleSnapshot(
    ulong Revision,
    LifecyclePhase Phase,
    StopReason? StopReason
);

public sealed record HostOptions(
    IdentityConfig Identity,
    HostRole Role,
    ImmutableArray<DestinationConfig> Destinations,
    ImmutableArray<Capability> RequiredCapabilities,
    HostLimits Limits,
    PersistenceConfig? Persistence = null
)
{
    public static HostOptions EphemeralEndpoint =>
        new(
            new IdentityConfig.GenerateEphemeral(),
            HostRole.Endpoint,
            [],
            [],
            HostLimits.Balanced,
            new PersistenceConfig.Ephemeral()
        );

    public static HostOptions PersistentEndpoint(string root) =>
        new(
            new IdentityConfig.LoadOrCreate(Path.Combine(root, "identity")),
            HostRole.Endpoint,
            [],
            [],
            HostLimits.Balanced,
            new PersistenceConfig.Directory(Path.Combine(root, "state"))
        );
}

public abstract record CommandSettlement
{
    public sealed record Succeeded(CommandOutcome Outcome) : CommandSettlement;
    public sealed record Failed(CommandFailure Failure) : CommandSettlement;

    public TResult Match<TResult>(
        Func<Succeeded, TResult> succeeded,
        Func<Failed, TResult> failed
    ) =>
        this switch
        {
            Succeeded value => succeeded(value),
            Failed value => failed(value),
            _ => throw new InvalidOperationException("Unknown command settlement case."),
        };
}

public abstract record HostCreation
{
    public sealed record Ready(PrnsHost Host) : HostCreation;

    public sealed record ContractMismatch(
        uint RequiredAbi,
        uint ActualAbi,
        uint RequiredSchemaVersion,
        uint ActualSchemaVersion,
        string RequiredProductVersion,
        string ActualProductVersion
    ) : HostCreation;

    public sealed record InvalidConfiguration(Status Status) : HostCreation;
    public sealed record BackendFailed(Status Status) : HostCreation;

    public TResult Match<TResult>(
        Func<Ready, TResult> ready,
        Func<ContractMismatch, TResult> contractMismatch,
        Func<InvalidConfiguration, TResult> invalidConfiguration,
        Func<BackendFailed, TResult> backendFailed
    ) =>
        this switch
        {
            Ready value => ready(value),
            ContractMismatch value => contractMismatch(value),
            InvalidConfiguration value => invalidConfiguration(value),
            BackendFailed value => backendFailed(value),
            _ => throw new InvalidOperationException("Unknown host creation case."),
        };
}

public sealed class PrnsHost : IAsyncDisposable
{
    private readonly HostHandle _handle;
    private int _disposed;

    private PrnsHost(HostHandle handle)
    {
        _handle = handle;
    }

    public static HostCreation Create(HostOptions options)
    {
        ArgumentNullException.ThrowIfNull(options);
        var actual = NativeContract();
        if (actual.Status != Status.Ok)
        {
            return new HostCreation.BackendFailed(actual.Status);
        }
        if (
            actual.Abi != HostContract.Abi
            || actual.SchemaVersion != HostContract.SchemaVersion
            || actual.ProductVersion != HostContract.ProductVersion
        )
        {
            return new HostCreation.ContractMismatch(
                HostContract.Abi,
                actual.Abi,
                HostContract.SchemaVersion,
                actual.SchemaVersion,
                HostContract.ProductVersion,
                actual.ProductVersion
            );
        }
        using var arena = new NativeArena();
        var version = arena.String(HostContract.ProductVersion);
        try
        {
            var destinations = MarshalDestinations(options.Destinations, arena);
            var requiredCapabilities = options.RequiredCapabilities.IsDefault
                ? ImmutableArray<Capability>.Empty
                : options.RequiredCapabilities;
            var nativeLimits = new Native.Limits
            {
                StructSize = (nuint)Marshal.SizeOf<Native.Limits>(),
                PendingCommands = options.Limits.PendingCommands,
                ApplicationEvents = options.Limits.ApplicationEvents,
                RetainedEventBytes = options.Limits.RetainedEventBytes,
                Diagnostics = options.Limits.Diagnostics,
            };
            var nativeOptions = new Native.HostOptions
            {
                StructSize = (nuint)Marshal.SizeOf<Native.HostOptions>(),
                RequiredAbi = HostContract.Abi,
                RequiredProductVersion = version,
                Limits = nativeLimits,
                Role = options.Role,
                Identity = MarshalIdentity(options.Identity, arena),
                Destinations = arena.Array<Native.DestinationConfig>(destinations),
                DestinationCount = (nuint)destinations.Length,
                RequiredCapabilities = arena.Array<Capability>(requiredCapabilities.AsSpan()),
                RequiredCapabilityCount = (nuint)requiredCapabilities.Length,
                Persistence = MarshalPersistence(options.Persistence, arena),
            };
            var status = Native.prns_host_create(in nativeOptions, out var handle);
            if (status == Status.Ok)
            {
                return new HostCreation.Ready(new PrnsHost(handle));
            }
            handle?.Dispose();
            if (status == Status.ContractMismatch)
            {
                return new HostCreation.ContractMismatch(
                    HostContract.Abi,
                    actual.Abi,
                    HostContract.SchemaVersion,
                    actual.SchemaVersion,
                    HostContract.ProductVersion,
                    actual.ProductVersion
                );
            }
            if (status == Status.InvalidArgument)
            {
                return new HostCreation.InvalidConfiguration(status);
            }
            return new HostCreation.BackendFailed(status);
        }
        catch (ArgumentException)
        {
            return new HostCreation.InvalidConfiguration(Status.InvalidArgument);
        }
    }

    private static Native.IdentityConfig MarshalIdentity(
        IdentityConfig identity,
        NativeArena arena
    )
    {
        ArgumentNullException.ThrowIfNull(identity);
        return identity.Match(
            existing =>
                new Native.IdentityConfig
                {
                    StructSize = (nuint)Marshal.SizeOf<Native.IdentityConfig>(),
                    Kind = IdentityConfigKind.Existing,
                    Secret = arena.Bytes(existing.Secret.Span),
                },
            _ =>
                new Native.IdentityConfig
                {
                    StructSize = (nuint)Marshal.SizeOf<Native.IdentityConfig>(),
                    Kind = IdentityConfigKind.GenerateEphemeral,
                },
            loadOrCreate =>
                new Native.IdentityConfig
                {
                    StructSize = (nuint)Marshal.SizeOf<Native.IdentityConfig>(),
                    Kind = IdentityConfigKind.LoadOrCreate,
                    Path = arena.String(loadOrCreate.Path),
                }
        );
    }

    private static Native.PersistenceConfig MarshalPersistence(
        PersistenceConfig? persistence,
        NativeArena arena
    )
    {
        persistence ??= new PersistenceConfig.Ephemeral();
        return persistence.Match(
            _ =>
                new Native.PersistenceConfig
                {
                    StructSize = (nuint)Marshal.SizeOf<Native.PersistenceConfig>(),
                    Kind = PersistenceConfigKind.Ephemeral,
                },
            directory =>
                new Native.PersistenceConfig
                {
                    StructSize = (nuint)Marshal.SizeOf<Native.PersistenceConfig>(),
                    Kind = PersistenceConfigKind.Directory,
                    Path = arena.String(directory.Path),
                }
        );
    }

    private static Native.DestinationName MarshalDestinationName(
        DestinationName name,
        NativeArena arena
    )
    {
        ArgumentNullException.ThrowIfNull(name);
        if (string.IsNullOrEmpty(name.AppName) || name.Aspects.IsDefaultOrEmpty)
        {
            throw new ArgumentException("A destination requires an app name and aspects.");
        }
        var aspects = new Native.StringView[name.Aspects.Length];
        for (var index = 0; index < aspects.Length; index++)
        {
            if (string.IsNullOrEmpty(name.Aspects[index]))
            {
                throw new ArgumentException("Destination aspects cannot be empty.");
            }
            aspects[index] = arena.String(name.Aspects[index]);
        }
        return new Native.DestinationName
        {
            StructSize = (nuint)Marshal.SizeOf<Native.DestinationName>(),
            AppName = arena.String(name.AppName),
            Aspects = arena.Array<Native.StringView>(aspects),
            AspectCount = (nuint)aspects.Length,
        };
    }

    private static Native.DestinationConfig[] MarshalDestinations(
        ImmutableArray<DestinationConfig> destinations,
        NativeArena arena
    )
    {
        if (destinations.IsDefaultOrEmpty)
        {
            return [];
        }
        var native = new Native.DestinationConfig[destinations.Length];
        for (var index = 0; index < destinations.Length; index++)
        {
            native[index] = destinations[index].Match(
                plain =>
                    new Native.DestinationConfig
                    {
                        StructSize = (nuint)Marshal.SizeOf<Native.DestinationConfig>(),
                        Kind = DestinationConfigKind.Plain,
                        Name = MarshalDestinationName(plain.Name, arena),
                    },
                single =>
                {
                    var identity = single.Identity.Match(
                        _ =>
                            (
                                DestinationIdentityConfigKind.HostIdentity,
                                default(Native.IdentityConfig)
                            ),
                        dedicated =>
                            (
                                DestinationIdentityConfigKind.DedicatedIdentity,
                                MarshalIdentity(dedicated.Identity, arena)
                            )
                    );
                    var handlers = single.RequestHandlers.IsDefaultOrEmpty
                        ? []
                        : single.RequestHandlers
                            .Select(handler => new Native.RequestHandlerConfig
                            {
                                StructSize = (nuint)Marshal.SizeOf<Native.RequestHandlerConfig>(),
                                Path = arena.String(handler.Path),
                                Policy = handler.Policy,
                            })
                            .ToArray();
                    return new Native.DestinationConfig
                    {
                        StructSize = (nuint)Marshal.SizeOf<Native.DestinationConfig>(),
                        Kind = DestinationConfigKind.Single,
                        Name = MarshalDestinationName(single.Name, arena),
                        IdentityKind = identity.Item1,
                        DedicatedIdentity = identity.Item2,
                        AnnounceAppData = single.AnnounceAppData is { } appData
                            ? arena.Bytes(appData.Span)
                            : default,
                        RequestHandlers = arena.Array<Native.RequestHandlerConfig>(handlers),
                        RequestHandlerCount = (nuint)handlers.Length,
                    };
                }
            );
        }
        return native;
    }

    private static (
        Status Status,
        uint Abi,
        uint SchemaVersion,
        string ProductVersion
    ) NativeContract()
    {
        var info = new Native.ContractInfo
        {
            StructSize = (nuint)Marshal.SizeOf<Native.ContractInfo>(),
        };
        var status = Native.prns_contract_info(ref info);
        if (status != Status.Ok)
        {
            return (status, 0, 0, string.Empty);
        }
        return (
            status,
            info.Abi,
            info.SchemaVersion,
            NativeValue.CopyString(info.ProductVersion)
        );
    }

    public LifecycleSnapshot Lifecycle
    {
        get
        {
            ObjectDisposedException.ThrowIf(_disposed != 0, this);
            var lifecycle = new Native.Lifecycle
            {
                StructSize = (nuint)Marshal.SizeOf<Native.Lifecycle>(),
            };
            PrnsException.ThrowIfError(Native.prns_host_lifecycle(_handle, ref lifecycle));
            var reason =
                lifecycle.Phase == LifecyclePhase.Stopped
                    ? (StopReason?)lifecycle.Reason
                    : null;
            return new LifecycleSnapshot(lifecycle.Revision, lifecycle.Phase, reason);
        }
    }

    public IdentityHash IdentityHash
    {
        get
        {
            ObjectDisposedException.ThrowIf(_disposed != 0, this);
            PrnsException.ThrowIfError(Native.prns_host_identity_hash(_handle, out var hash));
            return new IdentityHash(NativeValue.CopyBytes(hash));
        }
    }

    public ImmutableArray<DestinationHash> DestinationHashes
    {
        get
        {
            ObjectDisposedException.ThrowIf(_disposed != 0, this);
            var count = Native.prns_host_destination_count(_handle);
            if (count > int.MaxValue)
            {
                throw new OverflowException("Native destination count exceeds the .NET array limit.");
            }
            var hashes = ImmutableArray.CreateBuilder<DestinationHash>((int)count);
            for (nuint index = 0; index < count; index++)
            {
                PrnsException.ThrowIfError(
                    Native.prns_host_destination_hash(_handle, index, out var hash)
                );
                hashes.Add(new DestinationHash(NativeValue.CopyBytes(hash)));
            }
            return hashes.MoveToImmutable();
        }
    }

    public async ValueTask<CommandSettlement> ExecuteAsync(
        HostCommand command,
        CancellationToken cancellationToken = default
    )
    {
        ObjectDisposedException.ThrowIf(_disposed != 0, this);
        ArgumentNullException.ThrowIfNull(command);
        CommandHandle nativeCommand;
        using (var arena = new NativeArena())
        {
            nativeCommand = command.Match(
                announce => Submit(announce, arena),
                send => Submit(send, arena),
                close => Submit(close, arena),
                server => Submit(server, arena),
                client => Submit(client, arena),
                udp => Submit(udp, arena),
                detach => Submit(detach, arena),
                establish => Submit(establish, arena),
                path => Submit(path, arena),
                identify => Submit(identify, arena),
                sendLink => Submit(sendLink, arena),
                request => Submit(request, arena),
                respond => Submit(respond, arena),
                resource => Submit(resource, arena),
                linkStrategy => Submit(linkStrategy, arena),
                destinationStrategy => Submit(destinationStrategy, arena),
                channel => Submit(channel, arena),
                allow => Submit(allow, arena)
            );
        }
        using (nativeCommand)
        using (var readiness = NativeReadiness.ForCommand(nativeCommand))
        {
            while (true)
            {
                cancellationToken.ThrowIfCancellationRequested();
                var settlement = Poll(nativeCommand);
                if (settlement is not null)
                {
                    return settlement;
                }
                await readiness.WaitAsync(cancellationToken).ConfigureAwait(false);
            }
        }
    }

    public ValueTask<CommandSettlement> AnnounceAsync(
        DestinationHash destination,
        InterfaceId? interfaceId = null,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.Announce(destination, interfaceId), cancellationToken);

    public ValueTask<CommandSettlement> SendSinglePacketAsync(
        DestinationHash destination,
        ReadOnlyMemory<byte> payload,
        CancellationToken cancellationToken = default
    ) =>
        ExecuteAsync(
            new HostCommand.SendSinglePacket(destination, payload),
            cancellationToken
        );

    public ValueTask<CommandSettlement> CloseLinkAsync(
        LinkId linkId,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.CloseLink(linkId), cancellationToken);

    public ValueTask<CommandSettlement> AttachTcpServerAsync(
        string bind,
        Bitrate bitrate,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.AttachTcpServer(bind, bitrate), cancellationToken);

    public ValueTask<CommandSettlement> AttachTcpClientAsync(
        string target,
        Bitrate bitrate,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.AttachTcpClient(target, bitrate), cancellationToken);

    public ValueTask<CommandSettlement> AttachUdpAsync(
        string local,
        string peer,
        Bitrate bitrate,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.AttachUdp(local, peer, bitrate), cancellationToken);

    public ValueTask<CommandSettlement> DetachInterfaceAsync(
        InterfaceId interfaceId,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.DetachInterface(interfaceId), cancellationToken);

    public ValueTask<CommandSettlement> EstablishLinkAsync(
        DestinationHash destination,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.EstablishLink(destination), cancellationToken);

    public ValueTask<CommandSettlement> RequestPathAsync(
        DestinationHash destination,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.RequestPath(destination), cancellationToken);

    public ValueTask<CommandSettlement> IdentifyAsync(
        LinkId linkId,
        IdentityHash identity,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.Identify(linkId, identity), cancellationToken);

    public ValueTask<CommandSettlement> SendLinkPacketAsync(
        LinkId linkId,
        ReadOnlyMemory<byte> payload,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.SendLinkPacket(linkId, payload), cancellationToken);

    public ValueTask<CommandSettlement> RequestAsync(
        LinkId linkId,
        RequestPathHash pathHash,
        ReadOnlyMemory<byte> payload,
        ResponseTimeout timeout,
        CancellationToken cancellationToken = default
    ) => ExecuteAsync(new HostCommand.Request(linkId, pathHash, payload, timeout), cancellationToken);

    public ValueTask<CommandSettlement> RespondAsync(
        LinkId linkId,
        RequestId requestId,
        ulong requestRttMillis,
        ReadOnlyMemory<byte> payload,
        CancellationToken cancellationToken = default
    ) =>
        ExecuteAsync(
            new HostCommand.Respond(linkId, requestId, requestRttMillis, payload),
            cancellationToken
        );

    public ValueTask<CommandSettlement> SendResourceAsync(
        LinkId linkId,
        ReadOnlyMemory<byte> payload,
        ReadOnlyMemory<byte>? packedMetadata,
        ResourceCompression compression,
        CancellationToken cancellationToken = default
    ) =>
        ExecuteAsync(
            new HostCommand.SendResource(linkId, payload, packedMetadata, compression),
            cancellationToken
        );

    public ValueTask<CommandSettlement> SetLinkResourceStrategyAsync(
        LinkId linkId,
        ResourceStrategy strategy,
        CancellationToken cancellationToken = default
    ) =>
        ExecuteAsync(
            new HostCommand.SetLinkResourceStrategy(linkId, strategy),
            cancellationToken
        );

    public ValueTask<CommandSettlement> SetDestinationResourceStrategyAsync(
        DestinationHash destination,
        ResourceStrategy strategy,
        CancellationToken cancellationToken = default
    ) =>
        ExecuteAsync(
            new HostCommand.SetDestinationResourceStrategy(destination, strategy),
            cancellationToken
        );

    public ValueTask<CommandSettlement> SendChannelMessageAsync(
        LinkId linkId,
        ushort messageType,
        ReadOnlyMemory<byte> payload,
        CancellationToken cancellationToken = default
    ) =>
        ExecuteAsync(
            new HostCommand.SendChannelMessage(linkId, messageType, payload),
            cancellationToken
        );

    public ValueTask<CommandSettlement> AllowRequesterAsync(
        DestinationHash destination,
        RequestPathHash pathHash,
        IdentityHash identity,
        CancellationToken cancellationToken = default
    ) =>
        ExecuteAsync(
            new HostCommand.AllowRequester(destination, pathHash, identity),
            cancellationToken
        );

    private unsafe CommandHandle Submit(HostCommand.Announce command, NativeArena arena)
    {
        var destination = arena.Bytes(command.Destination.Span);
        var status = Status.InvalidArgument;
        CommandHandle nativeCommand;
        if (command.Interface is { } interfaceId)
        {
            var interfaceView = arena.Bytes(interfaceId.Span);
            status = Native.prns_host_announce(
                _handle,
                destination,
                &interfaceView,
                out nativeCommand
            );
        }
        else
        {
            status = Native.prns_host_announce(_handle, destination, null, out nativeCommand);
        }
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.SendSinglePacket command, NativeArena arena)
    {
        var status = Native.prns_host_send_single_packet(
            _handle,
            arena.Bytes(command.Destination.Span),
            arena.Bytes(command.Payload.Span),
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.CloseLink command, NativeArena arena)
    {
        var status = Native.prns_host_close_link(
            _handle,
            arena.Bytes(command.LinkId.Span),
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.AttachTcpServer command, NativeArena arena)
    {
        var bitrate = MarshalBitrate(command.Bitrate);
        var status = Native.prns_host_attach_tcp_server(
            _handle,
            arena.String(command.Bind),
            bitrate.Kind,
            bitrate.Value,
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.AttachTcpClient command, NativeArena arena)
    {
        var bitrate = MarshalBitrate(command.Bitrate);
        var status = Native.prns_host_attach_tcp_client(
            _handle,
            arena.String(command.Target),
            bitrate.Kind,
            bitrate.Value,
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.AttachUdp command, NativeArena arena)
    {
        var bitrate = MarshalBitrate(command.Bitrate);
        var status = Native.prns_host_attach_udp(
            _handle,
            arena.String(command.Local),
            arena.String(command.Peer),
            bitrate.Kind,
            bitrate.Value,
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.DetachInterface command, NativeArena arena)
    {
        var status = Native.prns_host_detach_interface(
            _handle,
            arena.Bytes(command.Interface.Span),
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.EstablishLink command, NativeArena arena)
    {
        var status = Native.prns_host_establish_link(
            _handle,
            arena.Bytes(command.Destination.Span),
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.RequestPath command, NativeArena arena)
    {
        var status = Native.prns_host_request_path(
            _handle,
            arena.Bytes(command.Destination.Span),
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.Identify command, NativeArena arena)
    {
        var status = Native.prns_host_identify(
            _handle,
            arena.Bytes(command.LinkId.Span),
            arena.Bytes(command.Identity.Span),
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.SendLinkPacket command, NativeArena arena)
    {
        var status = Native.prns_host_send_link_packet(
            _handle,
            arena.Bytes(command.LinkId.Span),
            arena.Bytes(command.Payload.Span),
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.Request command, NativeArena arena)
    {
        var timeout = MarshalResponseTimeout(command.Timeout);
        var status = Native.prns_host_request(
            _handle,
            arena.Bytes(command.LinkId.Span),
            arena.Bytes(command.PathHash.Span),
            arena.Bytes(command.Payload.Span),
            timeout.Kind,
            timeout.Millis,
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.Respond command, NativeArena arena)
    {
        var status = Native.prns_host_respond(
            _handle,
            arena.Bytes(command.LinkId.Span),
            arena.Bytes(command.RequestId.Span),
            command.RequestRttMillis,
            arena.Bytes(command.Payload.Span),
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private unsafe CommandHandle Submit(HostCommand.SendResource command, NativeArena arena)
    {
        var linkId = arena.Bytes(command.LinkId.Span);
        var payload = arena.Bytes(command.Payload.Span);
        var compression = MarshalResourceCompression(command.Compression);
        if (command.PackedMetadata is { } metadata)
        {
            var nativeMetadata = arena.Bytes(metadata.Span);
            var status = Native.prns_host_send_resource(
                _handle,
                linkId,
                payload,
                &nativeMetadata,
                compression,
                out var nativeCommand
            );
            return Submitted(status, nativeCommand);
        }
        {
            var status = Native.prns_host_send_resource(
                _handle,
                linkId,
                payload,
                null,
                compression,
                out var nativeCommand
            );
            return Submitted(status, nativeCommand);
        }
    }

    private CommandHandle Submit(HostCommand.SetLinkResourceStrategy command, NativeArena arena)
    {
        var strategy = MarshalResourceStrategy(command.Strategy);
        var status = Native.prns_host_set_link_resource_strategy(
            _handle,
            arena.Bytes(command.LinkId.Span),
            strategy.Kind,
            strategy.Maximum,
            strategy.AcceptCompressed,
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(
        HostCommand.SetDestinationResourceStrategy command,
        NativeArena arena
    )
    {
        var strategy = MarshalResourceStrategy(command.Strategy);
        var status = Native.prns_host_set_destination_resource_strategy(
            _handle,
            arena.Bytes(command.Destination.Span),
            strategy.Kind,
            strategy.Maximum,
            strategy.AcceptCompressed,
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.SendChannelMessage command, NativeArena arena)
    {
        var status = Native.prns_host_send_channel_message(
            _handle,
            arena.Bytes(command.LinkId.Span),
            command.MessageType,
            arena.Bytes(command.Payload.Span),
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private CommandHandle Submit(HostCommand.AllowRequester command, NativeArena arena)
    {
        var status = Native.prns_host_allow_requester(
            _handle,
            arena.Bytes(command.Destination.Span),
            arena.Bytes(command.PathHash.Span),
            arena.Bytes(command.Identity.Span),
            out var nativeCommand
        );
        return Submitted(status, nativeCommand);
    }

    private static (BitrateKind Kind, ulong Value) MarshalBitrate(Bitrate bitrate)
    {
        ArgumentNullException.ThrowIfNull(bitrate);
        return bitrate.Match<(BitrateKind Kind, ulong Value)>(
            _ => (BitrateKind.Auto, 0),
            explicitBitrate => (BitrateKind.BitsPerSecond, explicitBitrate.Value)
        );
    }

    private static (ResponseTimeoutKind Kind, ulong Millis) MarshalResponseTimeout(
        ResponseTimeout timeout
    )
    {
        ArgumentNullException.ThrowIfNull(timeout);
        return timeout.Match<(ResponseTimeoutKind Kind, ulong Millis)>(
            _ => (ResponseTimeoutKind.LinkDefault, 0),
            exact => (ResponseTimeoutKind.Exact, exact.Millis)
        );
    }

    private static ResourceCompressionKind MarshalResourceCompression(
        ResourceCompression compression
    )
    {
        ArgumentNullException.ThrowIfNull(compression);
        return compression.Match(
            _ => ResourceCompressionKind.Auto,
            _ => ResourceCompressionKind.Never
        );
    }

    private static (
        ResourceStrategyKind Kind,
        ulong Maximum,
        byte AcceptCompressed
    ) MarshalResourceStrategy(ResourceStrategy strategy)
    {
        ArgumentNullException.ThrowIfNull(strategy);
        return strategy.Match<(
            ResourceStrategyKind Kind,
            ulong Maximum,
            byte AcceptCompressed
        )>(
            _ => (ResourceStrategyKind.Refuse, 0, (byte)0),
            accept =>
            {
                ArgumentOutOfRangeException.ThrowIfZero(accept.MaximumUncompressedBytes);
                return (
                    ResourceStrategyKind.Accept,
                    accept.MaximumUncompressedBytes,
                    accept.AcceptCompressed ? (byte)1 : (byte)0
                );
            }
        );
    }

    private static CommandHandle Submitted(Status status, CommandHandle command)
    {
        if (status == Status.Ok)
        {
            return command;
        }
        command?.Dispose();
        PrnsException.ThrowIfError(status);
        throw new InvalidOperationException("Native command submission returned no result.");
    }

    private static CommandSettlement? Poll(CommandHandle command)
    {
        var result = new Native.CommandResult
        {
            StructSize = (nuint)Marshal.SizeOf<Native.CommandResult>(),
        };
        var status = Native.prns_command_wait(command, 0, ref result);
        if (status == Status.TimedOut)
        {
            return null;
        }
        PrnsException.ThrowIfError(status);
        if (result.Failure != 0)
        {
            return new CommandSettlement.Failed(DecodeCommandFailure(result));
        }
        CommandOutcome outcome = result.Outcome switch
        {
            CommandOutcomeKind.Announced => new CommandOutcome.Announced(),
            CommandOutcomeKind.PacketDelivered => new CommandOutcome.PacketDelivered(
                result.RttMillis,
                result.Evidence,
                DecodePacketHash(result.Evidence, result.Value)
            ),
            CommandOutcomeKind.LinkCloseQueued => new CommandOutcome.LinkCloseQueued(),
            CommandOutcomeKind.InterfaceAttached => new CommandOutcome.InterfaceAttached(
                new InterfaceId(NativeValue.CopyBytes(result.Value))
            ),
            CommandOutcomeKind.InterfaceDetached => new CommandOutcome.InterfaceDetached(
                new InterfaceId(NativeValue.CopyBytes(result.Value))
            ),
            CommandOutcomeKind.LinkEstablished => new CommandOutcome.LinkEstablished(
                new LinkId(NativeValue.CopyBytes(result.Value)),
                result.RttMillis
            ),
            CommandOutcomeKind.PathDiscovered => new CommandOutcome.PathDiscovered(
                DecodeHops(result.Value)
            ),
            CommandOutcomeKind.Identified => new CommandOutcome.Identified(),
            CommandOutcomeKind.ResponseReceived => new CommandOutcome.ResponseReceived(
                NativeValue.CopyBytes(result.Value),
                result.RttMillis
            ),
            CommandOutcomeKind.ResponseSent => new CommandOutcome.ResponseSent(result.RttMillis),
            CommandOutcomeKind.ResourceSent => new CommandOutcome.ResourceSent(),
            CommandOutcomeKind.ResourceStrategySet => new CommandOutcome.ResourceStrategySet(),
            CommandOutcomeKind.RequesterAllowed => new CommandOutcome.RequesterAllowed(),
            _ => throw new InvalidOperationException("Unknown native command outcome."),
        };
        return new CommandSettlement.Succeeded(outcome);
    }

    private static CommandFailure DecodeCommandFailure(Native.CommandResult result)
    {
        var detail = NativeValue.CopyString(result.Detail);
        return result.Failure switch
        {
            CommandFailureKind.NodeStopped => new CommandFailure.NodeStopped(),
            CommandFailureKind.Busy => new CommandFailure.Busy(),
            CommandFailureKind.PayloadTooLarge => new CommandFailure.PayloadTooLarge(),
            CommandFailureKind.UnknownDestination => new CommandFailure.UnknownDestination(),
            CommandFailureKind.NotSingleDestination =>
                new CommandFailure.NotSingleDestination(),
            CommandFailureKind.AnnounceAppDataTooLong =>
                new CommandFailure.AnnounceAppDataTooLong(),
            CommandFailureKind.UnknownInterface => new CommandFailure.UnknownInterface(),
            CommandFailureKind.NoRouteToDestination =>
                new CommandFailure.NoRouteToDestination(),
            CommandFailureKind.NotDirectlyReachable =>
                new CommandFailure.NotDirectlyReachable(),
            CommandFailureKind.PacketCulled => new CommandFailure.PacketCulled(),
            CommandFailureKind.DeliveryTimedOut => new CommandFailure.DeliveryTimedOut(),
            CommandFailureKind.InvalidBitrate => new CommandFailure.InvalidBitrate(),
            CommandFailureKind.BindFailed => new CommandFailure.BindFailed(detail),
            CommandFailureKind.WriteFailed => new CommandFailure.WriteFailed(detail),
            CommandFailureKind.UnsupportedByBackend =>
                new CommandFailure.UnsupportedByBackend(),
            CommandFailureKind.UnknownLink => new CommandFailure.UnknownLink(),
            CommandFailureKind.LinkNotActive => new CommandFailure.LinkNotActive(),
            CommandFailureKind.EntropyUnavailable => new CommandFailure.EntropyUnavailable(),
            CommandFailureKind.NotLinkInitiator => new CommandFailure.NotLinkInitiator(),
            CommandFailureKind.IdentityNotHeld => new CommandFailure.IdentityNotHeld(),
            CommandFailureKind.UnknownRequestHandler => new CommandFailure.UnknownRequestHandler(),
            CommandFailureKind.RequestPolicyNotAllowList =>
                new CommandFailure.RequestPolicyNotAllowList(),
            CommandFailureKind.RequestAllowListFull =>
                new CommandFailure.RequestAllowListFull(),
            CommandFailureKind.LinkBusy => new CommandFailure.LinkBusy(),
            CommandFailureKind.ResourceTableFull => new CommandFailure.ResourceTableFull(),
            CommandFailureKind.ResourceMetadataTooLarge =>
                new CommandFailure.ResourceMetadataTooLarge(),
            CommandFailureKind.ResourceRejectedByPeer =>
                new CommandFailure.ResourceRejectedByPeer(),
            CommandFailureKind.ResourceSequencingFailed =>
                new CommandFailure.ResourceSequencingFailed(),
            CommandFailureKind.ResourcePredecessorFailed =>
                new CommandFailure.ResourcePredecessorFailed(),
            CommandFailureKind.ChannelWindowFull => new CommandFailure.ChannelWindowFull(),
            CommandFailureKind.ChannelUntrackable => new CommandFailure.ChannelUntrackable(),
            CommandFailureKind.InvalidChannelMessageType =>
                new CommandFailure.InvalidChannelMessageType(),
            _ => throw new InvalidOperationException("Unknown native command failure."),
        };
    }

    private static byte DecodeHops(Native.ByteView value)
    {
        var bytes = NativeValue.CopyBytes(value);
        return bytes.Length == 1
            ? bytes[0]
            : throw new InvalidOperationException("Native path outcome has an invalid shape.");
    }

    private static PacketHash? DecodePacketHash(
        DeliveryEvidenceKind evidence,
        Native.ByteView value
    ) =>
        evidence switch
        {
            DeliveryEvidenceKind.Response when value.Length == 0 => null,
            DeliveryEvidenceKind.ExplicitProof
                or DeliveryEvidenceKind.ImplicitProof =>
                new PacketHash(NativeValue.CopyBytes(value)),
            _ => throw new InvalidOperationException(
                "Native delivery evidence and packet hash disagree."
            ),
        };

    public StreamClaim<ApplicationEvent> ClaimEvents()
    {
        ObjectDisposedException.ThrowIf(_disposed != 0, this);
        var status = Native.prns_host_claim_application_events(_handle, out var stream);
        if (status == Status.AlreadyClaimed)
        {
            stream?.Dispose();
            return new StreamClaim<ApplicationEvent>.AlreadyClaimed(
                AsyncLaneName.ApplicationEvents
            );
        }
        PrnsException.ThrowIfError(status);
        return new StreamClaim<ApplicationEvent>.Claimed(
            new NativeEventStream<ApplicationEvent>(stream, EventDecoder.Application)
        );
    }

    public StreamClaim<DiagnosticEvent> ClaimDiagnostics()
    {
        ObjectDisposedException.ThrowIf(_disposed != 0, this);
        var status = Native.prns_host_claim_diagnostics(_handle, out var stream);
        if (status == Status.AlreadyClaimed)
        {
            stream?.Dispose();
            return new StreamClaim<DiagnosticEvent>.AlreadyClaimed(AsyncLaneName.Diagnostics);
        }
        PrnsException.ThrowIfError(status);
        return new StreamClaim<DiagnosticEvent>.Claimed(
            new NativeEventStream<DiagnosticEvent>(stream, EventDecoder.Diagnostic)
        );
    }

    public ValueTask DisposeAsync()
    {
        if (Interlocked.Exchange(ref _disposed, 1) == 0)
        {
            PrnsException.ThrowIfError(Native.prns_host_stop(_handle));
            _handle.Dispose();
        }
        return ValueTask.CompletedTask;
    }
}

internal static class NativeValue
{
    internal static byte[] CopyBytes(Native.ByteView view)
    {
        if (view.Length > int.MaxValue)
        {
            throw new OverflowException("Native byte view exceeds the .NET array limit.");
        }
        var bytes = new byte[(int)view.Length];
        if (bytes.Length > 0)
        {
            Marshal.Copy(view.Data, bytes, 0, bytes.Length);
        }
        return bytes;
    }

    internal static string CopyString(Native.StringView view)
    {
        return Encoding.UTF8.GetString(CopyBytes(new Native.ByteView
        {
            Data = view.Data,
            Length = view.Length,
        }));
    }
}
