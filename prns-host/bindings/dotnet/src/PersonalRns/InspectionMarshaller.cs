using System.Collections.Immutable;
using System.Runtime.InteropServices;

namespace PersonalRns;

internal static class InspectionMarshaller
{
    internal static BackendInfo Decode(Native.BackendInfo value) =>
        new(
            value.Backend,
            EnumArray<Capability>(value.Capabilities, value.CapabilityCount),
            EnumArray<InterfaceKind>(value.InterfaceKinds, value.InterfaceKindCount)
        );

    internal static HostSnapshot Decode(Native.HostSnapshot value)
    {
        var interfaces = StructArray<Native.InterfaceSnapshot>(
                value.Interfaces,
                value.InterfaceCount
            )
            .Select(item => new InterfaceSnapshot(
                new InterfaceId(NativeValue.CopyBytes(item.InterfaceId)),
                item.HasName == 0 ? null : NativeValue.CopyString(item.Name),
                item.HasKind == 0 ? null : item.Kind,
                item.Health,
                item.HasFailureDetail == 0
                    ? null
                    : NativeValue.CopyString(item.FailureDetail),
                item.RxBytes,
                item.TxBytes,
                item.HasRxBps == 0 ? null : item.RxBps,
                item.HasTxBps == 0 ? null : item.TxBps,
                item.RouteCount,
                item.LinkCount,
                item.TransportedLinkCount
            ))
            .ToImmutableArray();
        var routes = StructArray<Native.RouteSnapshot>(value.Routes, value.RouteCount)
            .Select(item => new RouteSnapshot(
                new DestinationHash(NativeValue.CopyBytes(item.Destination)),
                item.Hops,
                item.HasViaIdentity == 0
                    ? null
                    : new IdentityHash(NativeValue.CopyBytes(item.ViaIdentity)),
                new InterfaceId(NativeValue.CopyBytes(item.InterfaceId)),
                item.LearnedAtMillis,
                item.LastRouteActivityAtMillis,
                item.ExpiresAtMillis
            ))
            .ToImmutableArray();
        var identities = StructArray<Native.DestinationIdentitySnapshot>(
                value.DestinationIdentities,
                value.DestinationIdentityCount
            )
            .Select(item => new DestinationIdentitySnapshot(
                new DestinationHash(NativeValue.CopyBytes(item.Destination)),
                new IdentityHash(NativeValue.CopyBytes(item.Identity))
            ))
            .ToImmutableArray();
        var runtime = value.Runtime;
        var persistence = value.Persistence;
        return new HostSnapshot(
            value.Revision,
            Decode(value.Backend),
            interfaces,
            routes,
            value.ActiveLinkCount,
            identities,
            new RuntimeHealthSnapshot(
                runtime.Running != 0,
                runtime.UptimeMillis,
                runtime.InterfaceCount,
                runtime.OnlineInterfaceCount,
                runtime.RouteCount,
                runtime.LinkCount,
                runtime.TransportedLinkCount,
                runtime.RxBytes,
                runtime.TxBytes,
                runtime.RxBps,
                runtime.TxBps
            ),
            new PersistenceSnapshot(
                persistence.Persistent != 0,
                persistence.Restored != 0,
                persistence.HasLastFlushCause == 0
                    ? null
                    : persistence.LastFlushCause,
                persistence.HasLastFailureDetail == 0
                    ? null
                    : NativeValue.CopyString(persistence.LastFailureDetail)
            )
        );
    }

    private static ImmutableArray<Value> EnumArray<Value>(nint pointer, nuint count)
        where Value : struct, Enum
    {
        if (count > int.MaxValue)
        {
            throw new OverflowException("Native enum count exceeds the .NET array limit.");
        }
        var values = ImmutableArray.CreateBuilder<Value>((int)count);
        for (var index = 0; index < (int)count; index++)
        {
            values.Add((Value)Enum.ToObject(typeof(Value), Marshal.ReadInt32(pointer, index * 4)));
        }
        return values.MoveToImmutable();
    }

    private static IEnumerable<Value> StructArray<Value>(nint pointer, nuint count)
        where Value : struct
    {
        if (count > int.MaxValue)
        {
            throw new OverflowException("Native structure count exceeds the .NET array limit.");
        }
        var size = Marshal.SizeOf<Value>();
        for (var index = 0; index < (int)count; index++)
        {
            yield return Marshal.PtrToStructure<Value>(pointer + index * size);
        }
    }
}
