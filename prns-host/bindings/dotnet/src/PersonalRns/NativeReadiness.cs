using System.Runtime.InteropServices;

namespace PersonalRns;

internal sealed class NativeReadiness : IDisposable
{
    private static readonly Native.ReadinessCallback Callback = Signal;
    private readonly SemaphoreSlim _available = new(0, 1);
    private readonly GCHandle _context;
    private ReadinessRegistrationHandle? _registration;
    private int _pending;
    private int _disposed;

    private NativeReadiness()
    {
        _context = GCHandle.Alloc(this);
    }

    internal static NativeReadiness ForCommand(CommandHandle command)
    {
        var readiness = new NativeReadiness();
        var status = Native.prns_command_register_readiness(
            command,
            Callback,
            GCHandle.ToIntPtr(readiness._context),
            out var registration
        );
        return readiness.Registered(status, registration);
    }

    internal static NativeReadiness ForEventStream(EventStreamHandle stream)
    {
        var readiness = new NativeReadiness();
        var status = Native.prns_event_stream_register_readiness(
            stream,
            Callback,
            GCHandle.ToIntPtr(readiness._context),
            out var registration
        );
        return readiness.Registered(status, registration);
    }

    private NativeReadiness Registered(
        Status status,
        ReadinessRegistrationHandle registration
    )
    {
        if (status != Status.Ok)
        {
            registration?.Dispose();
            Dispose();
            PrnsException.ThrowIfError(status);
        }
        _registration = registration;
        return this;
    }

    internal async ValueTask WaitAsync(CancellationToken cancellationToken)
    {
        await _available.WaitAsync(cancellationToken).ConfigureAwait(false);
        Volatile.Write(ref _pending, 0);
    }

    private static void Signal(nint context)
    {
        var handle = GCHandle.FromIntPtr(context);
        if (handle.Target is NativeReadiness readiness)
        {
            readiness.Set();
        }
    }

    private void Set()
    {
        if (Volatile.Read(ref _disposed) != 0)
        {
            return;
        }
        if (Interlocked.Exchange(ref _pending, 1) == 0)
        {
            _available.Release();
        }
    }

    public void Dispose()
    {
        if (Interlocked.Exchange(ref _disposed, 1) != 0)
        {
            return;
        }
        Interlocked.Exchange(ref _registration, null)?.Dispose();
        _context.Free();
        _available.Dispose();
    }
}
