using System.Runtime.InteropServices;

namespace Qfind.Windows;

internal sealed class NativeManager : IDisposable
{
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeRow
    {
        public nint Name, Path;
        public ulong Bytes, Entries;
        public uint Id;
        public byte IsDirectory;
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void RowCallback(nint context, nint row);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void TextCallback(nint context, nint text);

    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern nint qfind_manager_open([MarshalAs(UnmanagedType.LPUTF8Str)] string directory);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern void qfind_manager_free(nint manager);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern int qfind_manager_navigate(nint manager, [MarshalAs(UnmanagedType.LPUTF8Str)] string path);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern int qfind_manager_back(nint manager);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern int qfind_manager_forward(nint manager);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern int qfind_manager_directory(nint manager, TextCallback callback, nint context);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern int qfind_manager_rows(nint manager, [MarshalAs(UnmanagedType.LPUTF8Str)] string query, byte recursive, uint limit, RowCallback callback, nint context);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern int qfind_manager_chart(nint manager, byte global, uint limit, RowCallback callback, nint context);

    private readonly nint handle;
    private static readonly RowCallback collectRow = CollectRow;
    private static readonly TextCallback collectText = CollectText;

    public NativeManager(string directory)
    {
        handle = qfind_manager_open(directory);
        if (handle == 0) throw new InvalidOperationException("Run qfind index first.");
    }

    public IReadOnlyList<FileItem> Rows(string query, bool recursive) => Collect(callback => qfind_manager_rows(handle, query, recursive ? (byte)1 : (byte)0, 5_000, collectRow, callback));
    public IReadOnlyList<FileItem> Chart(bool global) => Collect(callback => qfind_manager_chart(handle, global ? (byte)1 : (byte)0, 24, collectRow, callback));
    public bool Navigate(string path) => qfind_manager_navigate(handle, path) == 0;
    public bool Back() => qfind_manager_back(handle) == 0;
    public bool Forward() => qfind_manager_forward(handle) == 0;

    public string Directory
    {
        get
        {
            var box = new TextBox();
            var pin = GCHandle.Alloc(box);
            try { _ = qfind_manager_directory(handle, collectText, GCHandle.ToIntPtr(pin)); }
            finally { pin.Free(); }
            return box.Value;
        }
    }

    private IReadOnlyList<FileItem> Collect(Func<nint, int> call)
    {
        var rows = new List<FileItem>();
        var pin = GCHandle.Alloc(rows);
        try
        {
            if (call(GCHandle.ToIntPtr(pin)) != 0) rows.Clear();
            return rows;
        }
        finally { pin.Free(); }
    }

    private static void CollectRow(nint context, nint pointer)
    {
        var row = Marshal.PtrToStructure<NativeRow>(pointer);
        ((List<FileItem>)GCHandle.FromIntPtr(context).Target!).Add(new FileItem(
            row.Id,
            Marshal.PtrToStringUTF8(row.Name) ?? "",
            Marshal.PtrToStringUTF8(row.Path) ?? "",
            row.Bytes,
            row.Entries,
            row.IsDirectory != 0));
    }

    private static void CollectText(nint context, nint pointer) => ((TextBox)GCHandle.FromIntPtr(context).Target!).Value = Marshal.PtrToStringUTF8(pointer) ?? "";
    private sealed class TextBox { public string Value = ""; }
    public void Dispose() => qfind_manager_free(handle);
}
