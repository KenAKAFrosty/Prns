# Personal RNS for .NET

The .NET adapter is a thin, idiomatic presentation of the common host contract:

- `SafeHandle` owns every native handle.
- Native readiness resumes `ValueTask` and `IAsyncEnumerable<T>` consumers without a blocking worker or moving pressure policy out of Rust.
- `StreamClaim<T>` makes single-consumer ownership explicit.
- Every contract union is a sealed record hierarchy with an exhaustive `Match` method generated from the language-neutral schema.
- Fixed-size hashes and identifiers validate and copy at construction.
- Native event memory is copied exactly once before its event handle is released.

## On-the-fly start

```csharp
using PersonalRns;

var run = PrnsHost.Create().Match(
    ready => Run(ready.Host),
    mismatch => Task.FromException(
        new InvalidOperationException(
            $"Host ABI {mismatch.ActualAbi} cannot satisfy {mismatch.RequiredAbi}."
        )
    ),
    invalid => Task.FromException(
        new InvalidOperationException($"Invalid host configuration: {invalid.Status}.")
    ),
    failed => Task.FromException(
        new InvalidOperationException($"Native host failed: {failed.Status}.")
    )
);

await run;

static async Task Run(PrnsHost host)
{
    await using (host)
    {
        var claim = host.ClaimEvents();
        if (claim is StreamClaim<ApplicationEvent>.AlreadyClaimed already)
        {
            throw new InvalidOperationException($"{already.Lane} already has an owner.");
        }
        await Consume(((StreamClaim<ApplicationEvent>.Claimed)claim).Stream);
    }
}

static async Task Consume(OwnedAsyncStream<ApplicationEvent> events)
{
    await using var owned = events;
    await foreach (var item in owned)
    {
        var summary = item.Match(
            delivery => $"single packet: {delivery.Plaintext.Length} bytes",
            request => $"request: {request.Data.Length} bytes",
            response => $"response: {response.Data.Length} bytes",
            segment => $"response segment {segment.SegmentIndex}",
            resource => $"resource: {resource.Hash}",
            segment => $"resource segment {segment.SegmentIndex}",
            compressed => $"compressed stream: {compressed.UncompressedDataBytes} bytes",
            channel => $"channel message: {channel.MessageType}"
        );
        Console.WriteLine(summary);
    }
}
```

The managed package expects the target’s `prns_host` native library to be available through normal .NET native-library resolution. On Linux, the registered contract smoke builds the capsule and exercises contract-gated creation, lifecycle, single-owner rejection, release, and reclaim across the real ABI:

```sh
python3 validation/run.py run --suite host-dotnet-contract
```

`ExecuteAsync` accepts the generated `HostCommand` sum and resolves to `CommandSettlement.Succeeded(CommandOutcome)` or `CommandSettlement.Failed(CommandFailure)`. Convenience methods such as `SendSinglePacketAsync`, `AttachTcpClientAsync`, and `DetachInterfaceAsync` delegate to that same contract.
