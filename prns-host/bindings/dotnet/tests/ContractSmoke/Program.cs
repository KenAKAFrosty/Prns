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
    }

    var reclaim = host.ClaimEvents();
    if (reclaim is StreamClaim<ApplicationEvent>.AlreadyClaimed unreleased)
    {
        throw new InvalidOperationException($"{unreleased.Lane} was not released for reclaim.");
    }
    await ((StreamClaim<ApplicationEvent>.Claimed)reclaim).Stream.DisposeAsync();
}
