using System.Collections.Immutable;
using System.Net;
using System.Net.Sockets;
using System.Text.Json;
using PersonalRns;

if (HostContract.Abi != 1 || HostContract.DestinationHashLength != 16)
{
    throw new InvalidOperationException("Generated contract constants drifted.");
}

var firstHash = new DestinationHash(new byte[HostContract.DestinationHashLength]);
var secondHash = new DestinationHash(new byte[HostContract.DestinationHashLength]);
if (firstHash != secondHash || firstHash.GetHashCode() != secondHash.GetHashCode())
{
    throw new InvalidOperationException("Fixed-size contract values lost structural equality.");
}

var defaultHash = default(DestinationHash);
if (
    defaultHash.Span.Length != HostContract.DestinationHashLength
    || defaultHash != firstHash
)
{
    throw new InvalidOperationException("Default fixed-size values are not valid zero values.");
}

ApplicationEvent sample = new ApplicationEvent.SingleDelivery(
    firstHash,
    new InterfaceId(new byte[HostContract.InterfaceIdLength]),
    new byte[] { 1, 2, 3 }
);

var size = sample.Match(
    singleDelivery => singleDelivery.Plaintext.Length,
    _ => 0,
    _ => 0,
    _ => 0,
    _ => 0,
    _ => 0,
    _ => 0,
    _ => 0,
    _ => 0
);

if (size != 3)
{
    throw new InvalidOperationException("Generated exhaustive match returned the wrong case.");
}

var host = PrnsHost.Create(HostOptions.EphemeralEndpoint).Match(
    ready => ready.Host,
    mismatch =>
        throw new InvalidOperationException(
            $"Native contract {mismatch.ActualAbi}/{mismatch.ActualProductVersion} does not satisfy {mismatch.RequiredAbi}/{mismatch.RequiredProductVersion}."
        ),
    invalid =>
        throw new InvalidOperationException($"Native host rejected balanced limits: {invalid.Status}."),
    failed =>
        throw new InvalidOperationException($"Native host creation failed: {failed.Status}.")
);

await using (host)
{
    if (host.Lifecycle.Phase != LifecyclePhase.Running)
    {
        throw new InvalidOperationException("A newly created host is not running.");
    }
    if (host.IdentityHash.Span.Length != HostContract.IdentityHashLength)
    {
        throw new InvalidOperationException("The real host identity hash is unavailable.");
    }
    if (host.BackendInfo.Backend != BackendKind.Native)
    {
        throw new InvalidOperationException("The native backend reported the wrong kind.");
    }
    var initialSnapshot = host.CaptureSnapshot();
    if (!initialSnapshot.Runtime.Running || initialSnapshot.Runtime.InterfaceCount != 0)
    {
        throw new InvalidOperationException("The initial runtime snapshot is inconsistent.");
    }

    var firstClaim = host.ClaimEvents();
    if (firstClaim is StreamClaim<ApplicationEvent>.AlreadyClaimed rejected)
    {
        throw new InvalidOperationException($"First {rejected.Lane} claim was rejected.");
    }
    var events = ((StreamClaim<ApplicationEvent>.Claimed)firstClaim).Stream;
    await using (events)
    {
        var secondClaim = host.ClaimEvents();
        if (secondClaim is StreamClaim<ApplicationEvent>.Claimed)
        {
            throw new InvalidOperationException("A second application consumer was admitted.");
        }
        var alreadyClaimed = (StreamClaim<ApplicationEvent>.AlreadyClaimed)secondClaim;
        if (alreadyClaimed.Lane != AsyncLaneName.ApplicationEvents)
        {
            throw new InvalidOperationException("The wrong lane rejected a second claim.");
        }
        using var cancellation = new CancellationTokenSource();
        await using var iterator = events.GetAsyncEnumerator(cancellation.Token);
        var waiting = iterator.MoveNextAsync().AsTask();
        cancellation.Cancel();
        try
        {
            await waiting;
            throw new InvalidOperationException("A cancelled event wait completed successfully.");
        }
        catch (OperationCanceledException)
        {
        }
    }

    var reclaim = host.ClaimEvents();
    if (reclaim is StreamClaim<ApplicationEvent>.AlreadyClaimed unreleased)
    {
        throw new InvalidOperationException($"{unreleased.Lane} was not released for reclaim.");
    }
    await ((StreamClaim<ApplicationEvent>.Claimed)reclaim).Stream.DisposeAsync();

    var settled = await host.CloseLinkAsync(new LinkId(new byte[HostContract.LinkIdLength]));
    if (
        settled
        is not CommandSettlement.Succeeded { Outcome: CommandOutcome.LinkCloseQueued }
    )
    {
        throw new InvalidOperationException("An asynchronous command did not settle.");
    }
    var resource = await host.SendResourceAsync(
        new LinkId(new byte[HostContract.LinkIdLength]),
        "bounded upload"u8.ToArray(),
        null,
        new ResourceCompression.Never()
    );
    if (
        resource
        is not CommandSettlement.Failed { Failure: CommandFailure.UnknownLink }
    )
    {
        throw new InvalidOperationException("Bounded resource upload returned the wrong failure.");
    }

    var attached = await host.AttachInterfaceAsync(
        new InterfaceConfig.TcpClient("127.0.0.1:9", new Bitrate.Auto()),
        new InterfaceRoutingPolicy(
            InterfaceMode.Boundary,
            -73,
            true,
            false,
            true
        )
    );
    if (
        attached
        is not CommandSettlement.Succeeded
        {
            Outcome: CommandOutcome.InterfaceAttached attachedOutcome,
        }
    )
    {
        throw new InvalidOperationException("Generic interface attachment did not settle.");
    }
    var attachedSnapshot = host.CaptureSnapshot();
    if (
        attachedSnapshot.Runtime.InterfaceCount != 1
        || attachedSnapshot.Interfaces.Length != 1
        || attachedSnapshot.Interfaces[0].InterfaceId != attachedOutcome.Interface
    )
    {
        throw new InvalidOperationException("The attached interface is absent from the snapshot.");
    }
    var detached = await host.DetachInterfaceAsync(attachedOutcome.Interface);
    if (
        detached
        is not CommandSettlement.Succeeded { Outcome: CommandOutcome.InterfaceDetached }
    )
    {
        throw new InvalidOperationException("Generic interface detachment did not settle.");
    }
}

#if PRNS_CONTRACT_INTERNALS
MarshalInterfaceFixtures();
#endif
await PersistentTwoNodeJourneyAsync();

#if PRNS_CONTRACT_INTERNALS
static void MarshalInterfaceFixtures()
{
    var fixturePath = Path.Combine(
        "prns-host",
        "conformance",
        "interface-configs-v1.json"
    );
    var fixture = JsonSerializer.Deserialize<InterfaceFixture>(
        File.ReadAllText(fixturePath),
        new JsonSerializerOptions { PropertyNameCaseInsensitive = true }
    ) ?? throw new InvalidOperationException("The interface fixture is empty.");
    var line = new SerialLineConfig(
        115_200,
        SerialDataBits.Eight,
        SerialParity.None,
        SerialStopBits.One
    );
    var ax25Line = line with { Baud = 9_600 };
    var radio = new RNodeRadioConfig(915_000_000, 125_000, 14, 8, 5);
    var configs = ImmutableArray.Create<InterfaceConfig>(
        new InterfaceConfig.AutoLan(
            "sdk-fixture",
            DiscoveryScope.Organization,
            29_710,
            42_444,
            ImmutableArray.Create("eth0"),
            ImmutableArray.Create("lo"),
            MulticastAddressType.Permanent
        ),
        new InterfaceConfig.TcpClient(
            "127.0.0.1:4242",
            new Bitrate.BitsPerSecond(1_000_000)
        ),
        new InterfaceConfig.TcpServer("127.0.0.1:4242", new Bitrate.Auto()),
        new InterfaceConfig.Udp(
            "127.0.0.1:4242",
            "127.0.0.1:4243",
            new Bitrate.BitsPerSecond(2_000_000)
        ),
        new InterfaceConfig.Serial("/dev/ttyUSB0", line),
        new InterfaceConfig.Kiss(
            "/dev/ttyUSB1",
            line,
            true,
            150,
            50,
            64,
            20,
            "PRNS",
            300
        ),
        new InterfaceConfig.Ax25Kiss(
            "/dev/ttyUSB2",
            ax25Line,
            false,
            100,
            25,
            32,
            10,
            "PRNS",
            1
        ),
        new InterfaceConfig.RNode(
            "/dev/ttyACM0",
            radio,
            true,
            "PRNS",
            300,
            1_000,
            500
        ),
        new InterfaceConfig.MultiRNode(
            "/dev/ttyACM1",
            "PRNS",
            300,
            ImmutableArray.Create(
                new MultiRNodeMemberConfig("primary", 1, radio, true, true)
            )
        ),
        new InterfaceConfig.Pipe(
            ImmutableArray.Create("fixture-command", "--fixture"),
            1_000
        ),
        new InterfaceConfig.BackboneClient("127.0.0.1:4244", new Bitrate.Auto()),
        new InterfaceConfig.BackboneServer(
            "127.0.0.1:4245",
            new Bitrate.BitsPerSecond(4_000_000)
        ),
        new InterfaceConfig.I2p(ImmutableArray.Create("fixture.b32.i2p"), true),
        new InterfaceConfig.Weave("/dev/ttyWEAVE0"),
        new InterfaceConfig.AutomaticUsb(),
        new InterfaceConfig.AutomaticBluetoothLe(),
        new InterfaceConfig.WebSocketClient(
            "ws://fixture.invalid/client",
            WebSocketFramingSelection.Auto
        ),
        new InterfaceConfig.WebSocketServer(
            "127.0.0.1:4246",
            WebSocketFramingSelection.Hdlc
        ),
        new InterfaceConfig.BrowserRendezvous("ws://fixture.invalid/rendezvous")
    );
    if (fixture.SchemaVersion != HostContract.SchemaVersion || fixture.Interfaces.Length != configs.Length)
    {
        throw new InvalidOperationException("The interface fixture does not match the host contract.");
    }
    using var arena = new NativeArena();
    for (var index = 0; index < configs.Length; index++)
    {
        var native = InterfaceConfigMarshaller.Marshal(configs[index], arena);
        if (
            native.Kind != (InterfaceKind)(index + 1)
            || native.Kind.ToString() != fixture.Interfaces[index].Kind
        )
        {
            throw new InvalidOperationException(
                $"{fixture.Interfaces[index].Kind} marshalled as {native.Kind}."
            );
        }
    }
}
#endif

static async Task PersistentTwoNodeJourneyAsync()
{
    var fixturePath = Path.Combine(
        "prns-host",
        "conformance",
        "persistent-two-node-v1.json"
    );
    var fixture = JsonSerializer.Deserialize<JourneyFixture>(
        await File.ReadAllTextAsync(fixturePath),
        new JsonSerializerOptions { PropertyNameCaseInsensitive = true }
    ) ?? throw new InvalidOperationException("The persistent journey fixture is empty.");
    if (fixture.SchemaVersion != HostContract.SchemaVersion)
    {
        throw new InvalidOperationException("The persistent journey fixture schema drifted.");
    }

    var port = ReserveLoopbackPort();
    var root = Directory.CreateTempSubdirectory("prns-dotnet-journey-").FullName;
    var destination = new DestinationConfig.Single(
        new DestinationName(
            fixture.Destination.AppName,
            fixture.Destination.Aspects.ToImmutableArray()
        ),
        new DestinationIdentityConfig.HostIdentity(),
        DecodeHex(fixture.Destination.AnnounceAppDataHex),
        1_048_576,
        [new RequestHandlerConfig(fixture.Request.Path, RequestPolicy.AllowAll)]
    );
    var serverOptions = HostOptions.PersistentEndpoint(Path.Combine(root, "server")) with
    {
        Destinations = [destination],
        RequiredCapabilities = [Capability.TcpServer],
    };
    var clientOptions = HostOptions.PersistentEndpoint(Path.Combine(root, "client")) with
    {
        RequiredCapabilities = [Capability.TcpClient],
    };

    try
    {
        IdentityHash serverIdentity;
        IdentityHash clientIdentity;
        DestinationHash destinationHash;
        await using (var server = CreateHost(serverOptions))
        await using (var client = CreateHost(clientOptions))
        {
            serverIdentity = server.IdentityHash;
            clientIdentity = client.IdentityHash;
            destinationHash = server.DestinationHashes.Single();
            var eventClaim = server.ClaimEvents();
            if (eventClaim is not StreamClaim<ApplicationEvent>.Claimed claimedEvents)
            {
                throw new InvalidOperationException("The server event lane could not be claimed.");
            }
            await using var events = claimedEvents.Stream;
            using var eventTimeout = new CancellationTokenSource(TimeSpan.FromSeconds(10));
            await using var eventIterator = events.GetAsyncEnumerator(eventTimeout.Token);

            SuccessfulOutcome(
                await server.AttachInterfaceAsync(
                    new InterfaceConfig.TcpServer(
                        $"127.0.0.1:{port}",
                        new Bitrate.Auto()
                    )
                )
            );
            SuccessfulOutcome(
                await client.AttachInterfaceAsync(
                    new InterfaceConfig.TcpClient(
                        $"127.0.0.1:{port}",
                        new Bitrate.Auto()
                    )
                )
            );

            var routed = false;
            for (var attempt = 0; attempt < 50 && !routed; attempt++)
            {
                routed = client.CaptureSnapshot().Routes.Any(
                    route => route.Destination == destinationHash
                );
                if (!routed)
                {
                    SuccessfulOutcome(await server.AnnounceAsync(destinationHash));
                    await Task.Delay(50);
                }
            }
            if (!routed)
            {
                throw new InvalidOperationException(
                    "The announced destination did not become routable."
                );
            }

            var link = SuccessfulOutcome(
                await client.EstablishLinkAsync(destinationHash)
            ) as CommandOutcome.LinkEstablished
                ?? throw new InvalidOperationException("Link establishment returned the wrong outcome.");
            var requestPayload = DecodeHex(fixture.Request.PayloadHex);
            var responsePayload = DecodeHex(fixture.Request.ResponseHex);
            var requestTask = client.RequestAsync(
                link.LinkId,
                new RequestPathHash(DecodeHex(fixture.Request.PathHashHex)),
                requestPayload,
                new ResponseTimeout.Exact(fixture.Request.TimeoutMillis),
                1_048_576
            ).AsTask();
            var request = await NextEventAsync<ApplicationEvent.Request>(eventIterator);
            if (!request.Data.Span.SequenceEqual(requestPayload))
            {
                throw new InvalidOperationException("The server received the wrong request payload.");
            }
            SuccessfulOutcome(
                await server.RespondAsync(
                    request.LinkId,
                    request.RequestId,
                    request.RttMillis,
                    responsePayload
                )
            );
            var response = SuccessfulOutcome(await requestTask) as CommandOutcome.ResponseReceived
                ?? throw new InvalidOperationException("The request returned the wrong outcome.");
            if (!response.Data.Span.SequenceEqual(responsePayload))
            {
                throw new InvalidOperationException("The client received the wrong response payload.");
            }

            SuccessfulOutcome(
                await server.SetLinkResourceStrategyAsync(
                    request.LinkId,
                    new ResourceStrategy.Accept(
                        fixture.Resource.MaximumUncompressedBytes,
                        fixture.Resource.AcceptCompressed
                    )
                )
            );
            var chunks = fixture.Resource.ChunksHex.Select(DecodeHex).ToArray();
            var resourcePayload = chunks.SelectMany(chunk => chunk).ToArray();
            var metadata = DecodeHex(fixture.Resource.MetadataHex);
            await using (var upload = client.BeginResourceUpload(
                link.LinkId,
                (ulong)resourcePayload.Length,
                metadata,
                new ResourceCompression.Never()
            ))
            {
                foreach (var chunk in chunks)
                {
                    await upload.WriteAsync(chunk);
                }
                if (SuccessfulOutcome(await upload.FinishAsync()) is not CommandOutcome.ResourceSent)
                {
                    throw new InvalidOperationException("The resource send returned the wrong outcome.");
                }
            }
            var resource = await NextEventAsync<ApplicationEvent.ResourceAvailable>(eventIterator);
            if (
                resource.Metadata is not { } receivedMetadata
                || !receivedMetadata.Span.SequenceEqual(metadata)
            )
            {
                throw new InvalidOperationException("The resource metadata changed in transit.");
            }
            var resourceClaim = resource.Resource.Claim();
            if (resourceClaim is not StreamClaim<ReadOnlyMemory<byte>>.Claimed claimedResource)
            {
                throw new InvalidOperationException("The resource stream could not be claimed.");
            }
            await using var resourceStream = claimedResource.Stream;
            using var received = new MemoryStream();
            await foreach (var chunk in resourceStream.WithCancellation(eventTimeout.Token))
            {
                received.Write(chunk.Span);
            }
            if (!received.ToArray().AsSpan().SequenceEqual(resourcePayload))
            {
                throw new InvalidOperationException("The streamed resource changed in transit.");
            }
        }

        await using var restoredServer = CreateHost(serverOptions);
        await using var restoredClient = CreateHost(clientOptions);
        if (
            restoredServer.IdentityHash != serverIdentity
            || restoredClient.IdentityHash != clientIdentity
        )
        {
            throw new InvalidOperationException("Persistent identities changed across restart.");
        }
        if (restoredServer.DestinationHashes.Single() != destinationHash)
        {
            throw new InvalidOperationException("The persistent destination changed across restart.");
        }
        var serverSnapshot = restoredServer.CaptureSnapshot();
        var clientSnapshot = restoredClient.CaptureSnapshot();
        if (!serverSnapshot.Persistence.Restored || !clientSnapshot.Persistence.Restored)
        {
            throw new InvalidOperationException("Persistence did not report restoration.");
        }
        if (!clientSnapshot.Routes.Any(route => route.Destination == destinationHash))
        {
            throw new InvalidOperationException("The client route was not restored.");
        }
    }
    finally
    {
        Directory.Delete(root, true);
    }
}

static PrnsHost CreateHost(HostOptions options) =>
    PrnsHost.Create(options).Match(
        ready => ready.Host,
        mismatch =>
            throw new InvalidOperationException(
                $"Native contract {mismatch.ActualAbi}/{mismatch.ActualSchemaVersion} does not satisfy {mismatch.RequiredAbi}/{mismatch.RequiredSchemaVersion}."
            ),
        invalid =>
            throw new InvalidOperationException($"Native host rejected the journey: {invalid.Status}."),
        failed =>
            throw new InvalidOperationException($"Native host creation failed: {failed.Status}.")
    );

static CommandOutcome SuccessfulOutcome(CommandSettlement settlement) =>
    settlement is CommandSettlement.Succeeded succeeded
        ? succeeded.Outcome
        : throw new InvalidOperationException(
            $"Command failed with {((CommandSettlement.Failed)settlement).Failure.GetType().Name}."
        );

static async Task<TEvent> NextEventAsync<TEvent>(
    IAsyncEnumerator<ApplicationEvent> events
) where TEvent : ApplicationEvent
{
    while (await events.MoveNextAsync())
    {
        if (events.Current is TEvent value)
        {
            return value;
        }
    }
    throw new InvalidOperationException($"The event stream ended before {typeof(TEvent).Name}.");
}

static byte[] DecodeHex(string value) => Convert.FromHexString(value);

static int ReserveLoopbackPort()
{
    var listener = new TcpListener(IPAddress.Loopback, 0);
    listener.Start();
    try
    {
        return ((IPEndPoint)listener.LocalEndpoint).Port;
    }
    finally
    {
        listener.Stop();
    }
}

internal sealed record JourneyFixture(
    uint SchemaVersion,
    JourneyDestination Destination,
    JourneyRequest Request,
    JourneyResource Resource
);

#if PRNS_CONTRACT_INTERNALS
internal sealed record InterfaceFixture(
    uint SchemaVersion,
    InterfaceFixtureCase[] Interfaces
);

internal sealed record InterfaceFixtureCase(string Kind);
#endif

internal sealed record JourneyDestination(
    string AppName,
    string[] Aspects,
    string AnnounceAppDataHex
);

internal sealed record JourneyRequest(
    string Path,
    string PathHashHex,
    string PayloadHex,
    string ResponseHex,
    ulong TimeoutMillis
);

internal sealed record JourneyResource(
    string[] ChunksHex,
    string MetadataHex,
    ulong MaximumUncompressedBytes,
    bool AcceptCompressed,
    string Compression
);
