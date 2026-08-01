namespace PersonalRns;

public sealed class ResourceUpload : IDisposable, IAsyncDisposable
{
    private readonly object _guard = new();
    private readonly PrnsHost _host;
    private nint _pointer;
    private bool _finished;

    internal ResourceUpload(PrnsHost host, nint pointer)
    {
        _host = host;
        _pointer = pointer;
    }

    public async ValueTask WriteAsync(
        ReadOnlyMemory<byte> chunk,
        CancellationToken cancellationToken = default
    )
    {
        while (true)
        {
            cancellationToken.ThrowIfCancellationRequested();
            Status status;
            lock (_guard)
            {
                ObjectDisposedException.ThrowIf(_pointer == 0 || _finished, this);
                using var arena = new NativeArena();
                status = Native.prns_resource_upload_write(
                    _pointer,
                    arena.Bytes(chunk.Span)
                );
            }
            if (status == Status.Ok)
            {
                return;
            }
            if (status != Status.WouldBlock)
            {
                PrnsException.ThrowIfError(status);
            }
            await Task.Yield();
        }
    }

    public async ValueTask<CommandSettlement> FinishAsync(
        CancellationToken cancellationToken = default
    )
    {
        CommandHandle command;
        lock (_guard)
        {
            ObjectDisposedException.ThrowIf(_pointer == 0 || _finished, this);
            PrnsException.ThrowIfError(
                Native.prns_resource_upload_finish(_pointer, out command)
            );
            _finished = true;
        }
        try
        {
            return await _host.AwaitNativeCommandAsync(command, cancellationToken)
                .ConfigureAwait(false);
        }
        finally
        {
            Dispose();
        }
    }

    public void Abort()
    {
        lock (_guard)
        {
            if (_pointer == 0 || _finished)
            {
                return;
            }
            Native.prns_resource_upload_abort(_pointer);
            _finished = true;
        }
    }

    public void Dispose()
    {
        lock (_guard)
        {
            if (_pointer == 0)
            {
                return;
            }
            if (!_finished)
            {
                Native.prns_resource_upload_abort(_pointer);
            }
            Native.prns_resource_upload_release(_pointer);
            _pointer = 0;
        }
        GC.SuppressFinalize(this);
    }

    public ValueTask DisposeAsync()
    {
        Dispose();
        return ValueTask.CompletedTask;
    }

    ~ResourceUpload()
    {
        Dispose();
    }
}
