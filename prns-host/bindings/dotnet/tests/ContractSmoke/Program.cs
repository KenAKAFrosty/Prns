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

var host = PrnsHost.Create().Match(
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

    var events = host.ClaimEvents().Match(
        claimed => claimed.Stream,
        already =>
            throw new InvalidOperationException($"First {already.Lane} claim was rejected.")
    );
    await using (events)
    {
        host.ClaimEvents().Match(
            _ => throw new InvalidOperationException("A second application consumer was admitted."),
            already =>
                already.Lane == AsyncLaneName.ApplicationEvents
                    ? true
                    : throw new InvalidOperationException("The wrong lane rejected a second claim.")
        );
    }

    var reclaimed = host.ClaimEvents().Match(
        claimed => claimed.Stream,
        already =>
            throw new InvalidOperationException($"{already.Lane} was not released for reclaim.")
    );
    await reclaimed.DisposeAsync();
}
