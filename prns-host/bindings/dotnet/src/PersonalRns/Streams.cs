using System.Threading.Channels;

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
    private readonly Channel<T> _channel;
    private readonly Task _pump;
    private int _claimed;

    internal NativeEventStream(EventStreamHandle handle, Func<EventHandle, T> decode)
    {
        _handle = handle;
        _decode = decode;
        _channel = Channel.CreateBounded<T>(
            new BoundedChannelOptions(1)
            {
                SingleReader = true,
                SingleWriter = true,
                FullMode = BoundedChannelFullMode.Wait,
                AllowSynchronousContinuations = false,
            }
        );
        _pump = Task.Factory.StartNew(
            Pump,
            CancellationToken.None,
            TaskCreationOptions.LongRunning,
            TaskScheduler.Default
        );
    }

    public override async IAsyncEnumerator<T> GetAsyncEnumerator(
        CancellationToken cancellationToken = default
    )
    {
        if (Interlocked.Exchange(ref _claimed, 1) != 0)
        {
            throw new InvalidOperationException("This stream already has a consumer.");
        }
        await foreach (
            var value in _channel.Reader.ReadAllAsync(cancellationToken).ConfigureAwait(false)
        )
        {
            yield return value;
        }
    }

    private void Pump()
    {
        Exception? failure = null;
        try
        {
            while (!_stopping.IsCancellationRequested)
            {
                var status = Native.prns_event_stream_next(_handle, 100, out var @event);
                if (status is Status.TimedOut or Status.WouldBlock)
                {
                    @event?.Dispose();
                    continue;
                }
                if (status == Status.Stopped)
                {
                    @event?.Dispose();
                    break;
                }
                PrnsException.ThrowIfError(status);
                using (@event)
                {
                    var value = _decode(@event);
                    _channel.Writer.WriteAsync(value, _stopping.Token).AsTask().GetAwaiter().GetResult();
                }
            }
        }
        catch (OperationCanceledException) when (_stopping.IsCancellationRequested)
        {
        }
        catch (Exception error)
        {
            failure = error;
        }
        _channel.Writer.TryComplete(failure);
    }

    public override async ValueTask DisposeAsync()
    {
        _stopping.Cancel();
        try
        {
            await _pump.ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
        }
        _handle.Dispose();
        _stopping.Dispose();
    }
}

public sealed class ResourceStream
{
    private readonly ResourceStreamHandle _handle;
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
        return new StreamClaim<ReadOnlyMemory<byte>>.Claimed(
            new NativeResourceStream(_handle)
        );
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
