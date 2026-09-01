using System.ComponentModel;
using Microsoft.UI.Xaml.Media;

namespace Qfind.Windows;

internal sealed class FileItem(uint id, string name, string path, ulong bytes, ulong entries, bool isDirectory) : INotifyPropertyChanged
{
    private ImageSource? thumbnail;
    public uint Id { get; } = id;
    public string Name { get; } = name;
    public string Path { get; } = path;
    public ulong Bytes { get; } = bytes;
    public ulong Entries { get; } = entries;
    public bool IsDirectory { get; } = isDirectory;
    public string Size => Bytes == 0 ? "" : HumanSize(Bytes);
    public ImageSource? Thumbnail { get => thumbnail; set { thumbnail = value; PropertyChanged?.Invoke(this, new(nameof(Thumbnail))); } }
    public event PropertyChangedEventHandler? PropertyChanged;

    private static string HumanSize(ulong value)
    {
        string[] units = ["B", "KB", "MB", "GB", "TB"];
        var size = (double)value;
        var unit = 0;
        while (size >= 1024 && unit < units.Length - 1) { size /= 1024; unit++; }
        return $"{size:0.#} {units[unit]}";
    }
}
