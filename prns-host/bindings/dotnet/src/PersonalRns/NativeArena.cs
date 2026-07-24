using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;

namespace PersonalRns;

internal sealed class NativeArena : IDisposable
{
    private readonly List<GCHandle> _pins = [];
    private readonly List<(nint Pointer, int ByteLength)> _allocations = [];

    internal Native.ByteView Bytes(ReadOnlySpan<byte> value)
    {
        if (value.IsEmpty)
        {
            return default;
        }
        var bytes = value.ToArray();
        var pin = GCHandle.Alloc(bytes, GCHandleType.Pinned);
        _pins.Add(pin);
        return new Native.ByteView
        {
            Data = pin.AddrOfPinnedObject(),
            Length = (nuint)bytes.Length,
        };
    }

    internal Native.StringView String(string value)
    {
        var bytes = Bytes(Encoding.UTF8.GetBytes(value));
        return new Native.StringView
        {
            Data = bytes.Data,
            Length = bytes.Length,
        };
    }

    internal unsafe nint Array<T>(ReadOnlySpan<T> values)
        where T : unmanaged
    {
        if (values.IsEmpty)
        {
            return 0;
        }
        var bytes = checked(values.Length * sizeof(T));
        var pointer = Marshal.AllocHGlobal(bytes);
        _allocations.Add((pointer, bytes));
        values.CopyTo(new Span<T>((void*)pointer, values.Length));
        return pointer;
    }

    public unsafe void Dispose()
    {
        foreach (var pin in _pins)
        {
            if (pin.IsAllocated)
            {
                if (pin.Target is byte[] bytes)
                {
                    CryptographicOperations.ZeroMemory(bytes);
                }
                pin.Free();
            }
        }
        foreach (var allocation in _allocations)
        {
            new Span<byte>((void*)allocation.Pointer, allocation.ByteLength).Clear();
            Marshal.FreeHGlobal(allocation.Pointer);
        }
        _pins.Clear();
        _allocations.Clear();
    }
}
