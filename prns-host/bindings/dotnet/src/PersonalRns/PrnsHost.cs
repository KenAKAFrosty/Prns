using System.Runtime.InteropServices;
using System.Text;

namespace PersonalRns;

public sealed class PrnsException : Exception
{
    public Status Status { get; }

    internal PrnsException(Status status)
        : base($"Personal RNS host operation failed with {status}.")
    {
        Status = status;
    }

    internal static void ThrowIfError(Status status)
    {
        if (status != Status.Ok)
        {
            throw new PrnsException(status);
        }
    }
}

public readonly record struct HostLimits(
    nuint PendingCommands,
    nuint ApplicationEvents,
    nuint RetainedEventBytes,
    nuint Diagnostics
)
{
    public static HostLimits Balanced =>
        new(
            (nuint)HostContract.BalancedPendingCommands,
            (nuint)HostContract.BalancedApplicationEvents,
            (nuint)HostContract.BalancedRetainedEventBytes,
            (nuint)HostContract.BalancedDiagnostics
        );
}

public readonly record struct LifecycleSnapshot(
    ulong Revision,
    LifecyclePhase Phase,
    StopReason? StopReason
);

public abstract record HostCreation
{
    public sealed record Ready(PrnsHost Host) : HostCreation;

    public sealed record ContractMismatch(
        uint RequiredAbi,
        uint ActualAbi,
        string RequiredProductVersion,
        string ActualProductVersion
    ) : HostCreation;

    public sealed record InvalidConfiguration(Status Status) : HostCreation;
    public sealed record BackendFailed(Status Status) : HostCreation;

    public TResult Match<TResult>(
        Func<Ready, TResult> ready,
        Func<ContractMismatch, TResult> contractMismatch,
        Func<InvalidConfiguration, TResult> invalidConfiguration,
        Func<BackendFailed, TResult> backendFailed
    ) =>
        this switch
        {
            Ready value => ready(value),
            ContractMismatch value => contractMismatch(value),
            InvalidConfiguration value => invalidConfiguration(value),
            BackendFailed value => backendFailed(value),
            _ => throw new InvalidOperationException("Unknown host creation case."),
        };
}

public sealed class PrnsHost : IAsyncDisposable
{
    private readonly HostHandle _handle;
    private int _disposed;

    private PrnsHost(HostHandle handle)
    {
        _handle = handle;
    }

    public static HostCreation Create(HostLimits? limits = null)
    {
        var selected = limits ?? HostLimits.Balanced;
        var version = Encoding.UTF8.GetBytes(HostContract.ProductVersion);
        var pinned = GCHandle.Alloc(version, GCHandleType.Pinned);
        try
        {
            var nativeLimits = new Native.Limits
            {
                StructSize = (nuint)Marshal.SizeOf<Native.Limits>(),
                PendingCommands = selected.PendingCommands,
                ApplicationEvents = selected.ApplicationEvents,
                RetainedEventBytes = selected.RetainedEventBytes,
                Diagnostics = selected.Diagnostics,
            };
            var options = new Native.HostOptions
            {
                StructSize = (nuint)Marshal.SizeOf<Native.HostOptions>(),
                RequiredAbi = HostContract.Abi,
                RequiredProductVersion = new Native.StringView
                {
                    Data = pinned.AddrOfPinnedObject(),
                    Length = (nuint)version.Length,
                },
                Limits = nativeLimits,
            };
            var status = Native.prns_host_create(in options, out var handle);
            if (status == Status.Ok)
            {
                return new HostCreation.Ready(new PrnsHost(handle));
            }
            handle?.Dispose();
            if (status == Status.ContractMismatch)
            {
                var actual = NativeContract();
                return new HostCreation.ContractMismatch(
                    HostContract.Abi,
                    actual.Abi,
                    HostContract.ProductVersion,
                    actual.ProductVersion
                );
            }
            if (status == Status.InvalidArgument)
            {
                return new HostCreation.InvalidConfiguration(status);
            }
            return new HostCreation.BackendFailed(status);
        }
        finally
        {
            pinned.Free();
        }
    }

    private static (uint Abi, string ProductVersion) NativeContract()
    {
        var info = new Native.ContractInfo
        {
            StructSize = (nuint)Marshal.SizeOf<Native.ContractInfo>(),
        };
        var status = Native.prns_contract_info(ref info);
        if (status != Status.Ok)
        {
            return (0, string.Empty);
        }
        return (info.Abi, NativeValue.CopyString(info.ProductVersion));
    }

    public LifecycleSnapshot Lifecycle
    {
        get
        {
            ObjectDisposedException.ThrowIf(_disposed != 0, this);
            var lifecycle = new Native.Lifecycle
            {
                StructSize = (nuint)Marshal.SizeOf<Native.Lifecycle>(),
            };
            PrnsException.ThrowIfError(Native.prns_host_lifecycle(_handle, ref lifecycle));
            var reason =
                lifecycle.Phase == LifecyclePhase.Stopped
                    ? (StopReason?)lifecycle.Reason
                    : null;
            return new LifecycleSnapshot(lifecycle.Revision, lifecycle.Phase, reason);
        }
    }

    public StreamClaim<ApplicationEvent> ClaimEvents()
    {
        ObjectDisposedException.ThrowIf(_disposed != 0, this);
        var status = Native.prns_host_claim_application_events(_handle, out var stream);
        if (status == Status.AlreadyClaimed)
        {
            stream?.Dispose();
            return new StreamClaim<ApplicationEvent>.AlreadyClaimed(
                AsyncLaneName.ApplicationEvents
            );
        }
        PrnsException.ThrowIfError(status);
        return new StreamClaim<ApplicationEvent>.Claimed(
            new NativeEventStream<ApplicationEvent>(stream, EventDecoder.Application)
        );
    }

    public StreamClaim<DiagnosticEvent> ClaimDiagnostics()
    {
        ObjectDisposedException.ThrowIf(_disposed != 0, this);
        var status = Native.prns_host_claim_diagnostics(_handle, out var stream);
        if (status == Status.AlreadyClaimed)
        {
            stream?.Dispose();
            return new StreamClaim<DiagnosticEvent>.AlreadyClaimed(AsyncLaneName.Diagnostics);
        }
        PrnsException.ThrowIfError(status);
        return new StreamClaim<DiagnosticEvent>.Claimed(
            new NativeEventStream<DiagnosticEvent>(stream, EventDecoder.Diagnostic)
        );
    }

    public ValueTask DisposeAsync()
    {
        if (Interlocked.Exchange(ref _disposed, 1) == 0)
        {
            PrnsException.ThrowIfError(Native.prns_host_stop(_handle));
            _handle.Dispose();
        }
        return ValueTask.CompletedTask;
    }
}

internal static class NativeValue
{
    internal static byte[] CopyBytes(Native.ByteView view)
    {
        if (view.Length > int.MaxValue)
        {
            throw new OverflowException("Native byte view exceeds the .NET array limit.");
        }
        var bytes = new byte[(int)view.Length];
        if (bytes.Length > 0)
        {
            Marshal.Copy(view.Data, bytes, 0, bytes.Length);
        }
        return bytes;
    }

    internal static string CopyString(Native.StringView view)
    {
        return Encoding.UTF8.GetString(CopyBytes(new Native.ByteView
        {
            Data = view.Data,
            Length = view.Length,
        }));
    }
}
