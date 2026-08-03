# Personal RNS for .NET

> **SDK preview: implemented, tested in-tree, and awaiting polished distribution.**
> This adapter runs the same Rust engine as every Prns node and is exercised by the repository's registered live .NET conformance suite.
> NuGet packaging with runtime-specific native assets and public-package qualification are active release work, so a source checkout is currently the supported evaluation path.
> If you are experienced with .NET API or packaging design, help making this feel completely at home in .NET would be especially valuable.

## Evaluate the current source

On Linux, the registered suite builds the matching native capsule and runs the
complete persistent two-node journey:

```console
python3 validation/run.py run --suite host-dotnet-contract
```

The intended public delivery is a `PersonalRns` NuGet package containing the
target's native runtime asset. Until that package has completed public
qualification, do not assume NuGet contains the adapter from this checkout. See
the [SDK guide](../../../docs/sdks.md#native-sdk-previews) for the shared release
posture and contribution path.

## API shape

The .NET adapter is a thin, idiomatic presentation of the common host contract:

- `SafeHandle` owns every native handle.
- Native readiness resumes `ValueTask` and `IAsyncEnumerable<T>` consumers without a blocking worker or moving pressure policy out of Rust.
- `StreamClaim<T>` makes single-consumer ownership explicit.
- Every contract union is a sealed record hierarchy with an exhaustive `Match` method generated from the language-neutral schema.
- Fixed-size hashes and identifiers validate and copy at construction.
- Native event memory is copied exactly once before its event handle is released.

### On-the-fly start

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

The source adapter expects the target's `prns_host` native library to be
available through normal .NET native-library resolution. The planned NuGet
package supplies it as a runtime-specific asset.

`ExecuteAsync` accepts the generated `HostCommand` sum and resolves to `CommandSettlement.Succeeded(CommandOutcome)` or `CommandSettlement.Failed(CommandFailure)`. Convenience methods such as `SendSinglePacketAsync`, `AttachTcpClientAsync`, and `DetachInterfaceAsync` delegate to that same contract.
