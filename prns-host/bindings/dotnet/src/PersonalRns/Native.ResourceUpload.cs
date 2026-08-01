using System.Runtime.InteropServices;

namespace PersonalRns;

internal static partial class Native
{
    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern unsafe Status prns_host_begin_resource_upload(
        HostHandle host,
        ByteView linkId,
        ulong declaredLength,
        ByteView* packedMetadata,
        ResourceCompressionKind compressionKind,
        out nint upload
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_resource_upload_write(nint upload, ByteView chunk);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern Status prns_resource_upload_finish(
        nint upload,
        out CommandHandle command
    );

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_resource_upload_abort(nint upload);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    internal static extern void prns_resource_upload_release(nint upload);
}
