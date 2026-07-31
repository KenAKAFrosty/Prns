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
        new InterfaceConfig.TcpClient("127.0.0.1:9", new Bitrate.Auto())
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
