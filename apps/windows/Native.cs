using System.Runtime.InteropServices;

namespace Qfind.Windows;

internal sealed record NativeView(IReadOnlyList<FileItem> Rows, string Directory, string Error, bool Succeeded);
internal sealed record NativeChart(IReadOnlyList<FileItem> Rows, string Error, bool Succeeded);
internal readonly record struct NativeActionResult(bool Succeeded, string Error);
internal readonly record struct NativeComponentResult(bool Succeeded, string Json, string Error);

internal sealed class NativeManager : IDisposable
{
    public enum SortMode : uint
    {
        Relevance,
        Name,
        NameDescending,
        Newest,
        Oldest,
        Largest,
        Smallest,
    }

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
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern int qfind_manager_search_scope(nint manager, byte global);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern int qfind_manager_sort(nint manager, uint sort);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern int qfind_manager_error(nint manager, TextCallback callback, nint context);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern int qfind_manager_component(nint manager, [MarshalAs(UnmanagedType.LPUTF8Str)] string component, [MarshalAs(UnmanagedType.LPUTF8Str)] string requestJson, TextCallback callback, nint context);
    [DllImport("qfind_native", CallingConvention = CallingConvention.Cdecl)] private static extern ulong qfind_folder_sizes_revision();

    private readonly object gate = new();
    private nint handle;
    private bool disposed;
    private int activeCalls;
    private nint pendingFreeHandle;
    private static readonly RowCallback collectRow = CollectRow;
    private static readonly TextCallback collectText = CollectText;

    public NativeManager(string directory)
    {
        handle = qfind_manager_open(directory);
        if (handle == 0) throw new InvalidOperationException("Megaman could not open its native manager.");
    }

    public static ulong FolderSizesRevision()
    {
        try { return qfind_folder_sizes_revision(); }
        catch (EntryPointNotFoundException) { return 0; }
    }

    public NativeView ReadView(string query, bool recursive, bool global, SortMode sort) => WithHandle(nativeHandle =>
    {
        OptionalInvokeUnsafe(() => qfind_manager_search_scope(nativeHandle, global ? (byte)1 : (byte)0));
        OptionalInvokeUnsafe(() => qfind_manager_sort(nativeHandle, (uint)sort));
        var (rows, rowsStatus) = CollectUnsafe(callback => qfind_manager_rows(nativeHandle, query, recursive ? (byte)1 : (byte)0, 5_000, collectRow, callback));
        var error = rowsStatus == 0 ? "" : ErrorUnsafe(nativeHandle);
        var (directory, directoryStatus) = DirectoryUnsafe(nativeHandle);
        if (directoryStatus != 0 && string.IsNullOrWhiteSpace(error)) error = ErrorUnsafe(nativeHandle);
        return new NativeView(rows, directory, error, rowsStatus == 0 && directoryStatus == 0 && string.IsNullOrWhiteSpace(error));
    });

    public NativeChart ReadChart(bool global) => WithHandle(nativeHandle =>
    {
        var (rows, status) = CollectUnsafe(callback => qfind_manager_chart(nativeHandle, global ? (byte)1 : (byte)0, 24, collectRow, callback));
        var error = status == 0 ? "" : ErrorUnsafe(nativeHandle);
        return new NativeChart(rows, error, status == 0 && string.IsNullOrWhiteSpace(error));
    });

    public NativeActionResult NavigateResult(string path) => ActionResult(nativeHandle => qfind_manager_navigate(nativeHandle, path));
    public NativeActionResult BackResult() => ActionResult(nativeHandle => qfind_manager_back(nativeHandle));
    public NativeActionResult ForwardResult() => ActionResult(nativeHandle => qfind_manager_forward(nativeHandle));

    public NativeComponentResult Component(string component, string requestJson) => WithHandle(nativeHandle =>
    {
        var box = new TextBox();
        var pin = GCHandle.Alloc(box);
        try
        {
            int status;
            try
            {
                status = qfind_manager_component(nativeHandle, component, requestJson, collectText, GCHandle.ToIntPtr(pin));
            }
            catch (EntryPointNotFoundException)
            {
                return new NativeComponentResult(false, "", "The loaded Windows native library does not expose component workflows.");
            }
            var response = box.Value;
            var error = status == 0 ? "" : response;
            if (status != 0 && string.IsNullOrWhiteSpace(error)) error = ErrorUnsafe(nativeHandle);
            return new NativeComponentResult(status == 0 && string.IsNullOrWhiteSpace(error), status == 0 ? response : "", error);
        }
        finally { pin.Free(); }
    });

    public string Directory => WithHandle(nativeHandle => DirectoryUnsafe(nativeHandle).Value);

    private NativeActionResult ActionResult(Func<nint, int> call) => WithHandle<NativeActionResult>(nativeHandle =>
    {
        var status = call(nativeHandle);
        return status == 0 ? new(true, "") : new(false, ErrorUnsafe(nativeHandle));
    });

    private (IReadOnlyList<FileItem> Rows, int Status) CollectUnsafe(Func<nint, int> call)
    {
        var rows = new List<FileItem>();
        var pin = GCHandle.Alloc(rows);
        try
        {
            var status = call(GCHandle.ToIntPtr(pin));
            if (status != 0) rows.Clear();
            return (rows, status);
        }
        finally { pin.Free(); }
    }

    private (string Value, int Status) DirectoryUnsafe(nint nativeHandle)
    {
        var box = new TextBox();
        var pin = GCHandle.Alloc(box);
        try
        {
            var status = qfind_manager_directory(nativeHandle, collectText, GCHandle.ToIntPtr(pin));
            return (box.Value, status);
        }
        finally { pin.Free(); }
    }

    private string ErrorUnsafe(nint nativeHandle)
    {
        var box = new TextBox();
        var pin = GCHandle.Alloc(box);
        try
        {
            _ = qfind_manager_error(nativeHandle, collectText, GCHandle.ToIntPtr(pin));
            return box.Value;
        }
        catch (EntryPointNotFoundException) { return ""; }
        finally { pin.Free(); }
    }

    private T WithHandle<T>(Func<nint, T> call)
    {
        nint nativeHandle;
        lock (gate)
        {
            ObjectDisposedException.ThrowIf(disposed, this);
            activeCalls++;
            nativeHandle = handle;
        }
        try { return call(nativeHandle); }
        finally
        {
            nint freeHandle = 0;
            lock (gate)
            {
                activeCalls--;
                if (disposed && activeCalls == 0)
                {
                    freeHandle = pendingFreeHandle;
                    pendingFreeHandle = 0;
                }
            }
            if (freeHandle != 0) qfind_manager_free(freeHandle);
        }
    }

    private static int OptionalInvokeUnsafe(Func<int> call)
    {
        try { return call(); }
        catch (EntryPointNotFoundException) { return 0; }
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

    public void Dispose()
    {
        nint freeHandle = 0;
        lock (gate)
        {
            if (disposed) return;
            disposed = true;
            if (activeCalls == 0) freeHandle = handle;
            else pendingFreeHandle = handle;
            handle = 0;
        }
        if (freeHandle != 0) qfind_manager_free(freeHandle);
    }
}
