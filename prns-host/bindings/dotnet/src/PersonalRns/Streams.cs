namespace PersonalRns;

public enum AsyncLaneName
{
    ApplicationEvents,
    Diagnostics,
    Resource,
}

public abstract record StreamClaim<T>
{
    public sealed record Claimed(OwnedAsyncStream<T> Stream) : StreamClaim<T>;
    public sealed record AlreadyClaimed(AsyncLaneName Lane) : StreamClaim<T>;

    public TResult Match<TResult>(
        Func<Claimed, TResult> claimed,
        Func<AlreadyClaimed, TResult> alreadyClaimed
    ) =>
        this switch
        {
            Claimed value => claimed(value),
            AlreadyClaimed value => alreadyClaimed(value),
            _ => throw new InvalidOperationException("Unknown stream claim case."),
        };
}

public abstract class OwnedAsyncStream<T> : IAsyncEnumerable<T>, IAsyncDisposable
{
    public abstract IAsyncEnumerator<T> GetAsyncEnumerator(
        CancellationToken cancellationToken = default
    );

    public abstract ValueTask DisposeAsync();
}

internal sealed class NativeEventStream<T> : OwnedAsyncStream<T>
{
    private readonly EventStreamHandle _handle;
    private readonly Func<EventHandle, T> _decode;
    private readonly CancellationTokenSource _stopping = new();
    private readonly SemaphoreSlim _drain = new(1, 1);
    private readonly NativeReadiness _readiness;
    private int _claimed;
    private int _disposed;

    internal NativeEventStream(EventStreamHandle handle, Func<EventHandle, T> decode)
    {
        _handle = handle;
        _decode = decode;
        try
        {
            _readiness = NativeReadiness.ForEventStream(handle);
        }
        catch
        {
            handle.Dispose();
            throw;
        }
    }

    public override async IAsyncEnumerator<T> GetAsyncEnumerator(
        CancellationToken cancellationToken = default
    )
    {
        if (Interlocked.Exchange(ref _claimed, 1) != 0)
        {
            throw new InvalidOperationException("This stream already has a consumer.");
        }
        using var linked = CancellationTokenSource.CreateLinkedTokenSource(
            cancellationToken,
            _stopping.Token
        );
        while (true)
        {
            linked.Token.ThrowIfCancellationRequested();
            T? value = default;
            var hasValue = false;
            var waited = false;
            var status = Status.WouldBlock;
            await _drain.WaitAsync(linked.Token).ConfigureAwait(false);
            try
            {
                if (Volatile.Read(ref _disposed) != 0)
                {
                    yield break;
                }
                status = Native.prns_event_stream_next(_handle, 0, out var @event);
                if (status == Status.Ok)
                {
                    using (@event)
                    {
                        value = _decode(@event);
                    }
                    hasValue = true;
                }
                else
                {
                    @event?.Dispose();
                }
                if (status == Status.WouldBlock)
                {
                    await _readiness.WaitAsync(linked.Token).ConfigureAwait(false);
                    waited = true;
                }
            }
            finally
            {
                _drain.Release();
            }
            if (hasValue)
            {
                yield return value!;
                continue;
            }
            if (waited)
            {
                continue;
            }
            if (status == Status.Stopped)
            {
                yield break;
            }
            if (status != Status.WouldBlock)
            {
                PrnsException.ThrowIfError(status);
            }
        }
    }

    public override async ValueTask DisposeAsync()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0)
        {
            return;
        }
        _stopping.Cancel();
        Native.prns_event_stream_interrupt_wait(_handle);
        await _drain.WaitAsync().ConfigureAwait(false);
        try
        {
            _readiness.Dispose();
            _handle.Dispose();
        }
        finally
        {
            _drain.Release();
            _drain.Dispose();
            _stopping.Dispose();
        }
    }
}

public sealed class ResourceStream : IDisposable, IAsyncDisposable
{
    private ResourceStreamHandle? _handle;
    private int _claimed;

    internal ResourceStream(ResourceStreamHandle handle, ulong totalBytes)
    {
        _handle = handle;
        TotalBytes = totalBytes;
    }

    public ulong TotalBytes { get; }

    public StreamClaim<ReadOnlyMemory<byte>> Claim()
    {
        if (Interlocked.Exchange(ref _claimed, 1) != 0)
        {
            return new StreamClaim<ReadOnlyMemory<byte>>.AlreadyClaimed(AsyncLaneName.Resource);
        }
        var handle =
            Interlocked.Exchange(ref _handle, null)
            ?? throw new ObjectDisposedException(nameof(ResourceStream));
        return new StreamClaim<ReadOnlyMemory<byte>>.Claimed(
            new NativeResourceStream(handle)
        );
    }

    public void Dispose()
    {
        Interlocked.Exchange(ref _handle, null)?.Dispose();
    }

    public ValueTask DisposeAsync()
    {
        Dispose();
        return ValueTask.CompletedTask;
    }
}

internal sealed class NativeResourceStream : OwnedAsyncStream<ReadOnlyMemory<byte>>
{
    private readonly ResourceStreamHandle _handle;
    private int _claimed;

    internal NativeResourceStream(ResourceStreamHandle handle)
    {
        _handle = handle;
    }

    public override async IAsyncEnumerator<ReadOnlyMemory<byte>> GetAsyncEnumerator(
        CancellationToken cancellationToken = default
    )
    {
        if (Interlocked.Exchange(ref _claimed, 1) != 0)
        {
            throw new InvalidOperationException("This resource stream already has a consumer.");
        }
        while (true)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var status = Native.prns_resource_stream_next(
                _handle,
                64 * 1024,
                out var chunk,
                out var finished
            );
            PrnsException.ThrowIfError(status);
            if (finished != 0)
            {
                yield break;
            }
            yield return NativeValue.CopyBytes(chunk);
            await Task.Yield();
        }
    }

    public override ValueTask DisposeAsync()
    {
        _handle.Dispose();
        return ValueTask.CompletedTask;
    }
}
