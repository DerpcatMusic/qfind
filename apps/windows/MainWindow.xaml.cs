using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.UI;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.UI.Xaml.Shapes;
using Microsoft.UI.Xaml.Controls.Primitives;
using Windows.ApplicationModel.DataTransfer;
using Windows.ApplicationModel.DataTransfer.DragDrop;
using Windows.Storage;
using Windows.Storage.FileProperties;
using Windows.System;
using WinUIDispatcherQueue = Microsoft.UI.Dispatching.DispatcherQueue;
using WinUIDispatcherQueueTimer = Microsoft.UI.Dispatching.DispatcherQueueTimer;

namespace Qfind.Windows;

public sealed partial class MainWindow : Window
{
    private static readonly JsonSerializerOptions jsonOptions = new() { PropertyNameCaseInsensitive = true };
    private NativeManager? manager;
    private readonly string? initialDirectory;
    private readonly CancellationTokenSource lifetime = new();
    private readonly TaskFactory nativeTasks = new(
        CancellationToken.None,
        TaskCreationOptions.DenyChildAttach,
        TaskContinuationOptions.None,
        new ConcurrentExclusiveSchedulerPair().ExclusiveScheduler);
    private readonly TaskFactory componentTasks = new(
        CancellationToken.None,
        TaskCreationOptions.DenyChildAttach,
        TaskContinuationOptions.None,
        new ConcurrentExclusiveSchedulerPair().ExclusiveScheduler);
    private readonly TaskFactory longComponentTasks = new(
        CancellationToken.None,
        TaskCreationOptions.DenyChildAttach,
        TaskContinuationOptions.None,
        new ConcurrentExclusiveSchedulerPair().ExclusiveScheduler);
    private CancellationTokenSource? refreshCancellation;
    private int refreshGeneration;
    private WinUIDispatcherQueueTimer? folderSizeTimer;
    private ulong folderSizeRevision;
    private HashSet<string>? pendingSelectionPaths;
    private WinUIDispatcherQueue? uiQueue;
    private readonly object folderWatcherGate = new();
    private System.IO.FileSystemWatcher? folderWatcher;
    private CancellationTokenSource? folderWatchDebounce;
    private IReadOnlyList<FileItem> chart = [];
    private IReadOnlyList<FileItem> fileRows = [];
    private readonly List<ColumnState> columnStates =
    [
        new("name", "Name", 320),
        new("size", "Size", 110),
        new("kind", "Kind", 120),
        new("path", "Location", 420),
    ];
    private readonly ObservableCollection<ColumnState> visibleColumns = [];
    private ColumnState? draggedColumn;
    private double columnDragStartX;
    private string? columnSortId;
    private bool columnSortDescending;
    private readonly List<Window> projectWindows = [];
    private ProjectInfo? selectedProject;
    private IReadOnlyList<DiffLine> diffLines = [];
    private readonly HashSet<int> collapsedHunks = [];
    private string? archiveWorkspacePath;
    private string? gitFooterPath;
    private DateTimeOffset gitFooterUpdated = DateTimeOffset.MinValue;
    private bool gitFooterRequestInFlight;
    private long gitFooterRequestId;

    public MainWindow() : this(null) { }

    public MainWindow(string? initialDirectory)
    {
        this.initialDirectory = initialDirectory;
        InitializeComponent();
        AppWindow.SetIcon(System.IO.Path.Combine(AppContext.BaseDirectory, "megaman.ico"));
        uiQueue = WinUIDispatcherQueue.GetForCurrentThread();
        LoadColumnState();
        FileColumnsHeader.ItemsSource = visibleColumns;
        SystemBackdrop = new MicaBackdrop();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(TitleBarDragRegion);

        Closed += (_, _) => _ = CloseAsync();
        SortMenu.SelectedIndex = 0;
        GitMode.SelectedIndex = 0;
        _ = InitializeAsync();
    }

    private sealed record ShellResponse(List<ComponentInfo>? Components);
    private sealed record ComponentInfo(string Id, string Title, string Icon, List<ComponentCommandInfo>? Commands);
    private sealed record ComponentCommandInfo(string Component, string Id, string Title, bool Mutating, string? Icon)
    {
        public string Glyph => Icon switch
        {
            "repository" => "\uE8B6",
            "branch" => "\uE8AD",
            "terminal" => "\uE756",
            "disk" => "\uEDA2",
            "files" => "\uE8A5",
            "archive" => "\uE7B8",
            _ => "\uE8B7",
        };
        public ComponentCommandInfo WithComponent(string component, string? icon) => this with { Component = component, Icon = icon };
    }
    private sealed record ProjectArtifact(string Path, ulong? Bytes);
    private sealed record ProjectInfo(string Path, string Repository, string Branch, bool Rust, bool Node, ulong? Bytes, List<ProjectArtifact>? Artifacts)
    {
        public string Summary => string.IsNullOrWhiteSpace(Repository) ? Branch : $"{Repository} · {Branch}";
        public string Metrics => $"{(Rust ? "Rust" : "")}{(Rust && Node ? " · " : "")}{(Node ? "Node" : "")}{(Rust || Node ? " · " : "")}{SizeText(Bytes ?? 0)} · {Artifacts?.Count ?? 0} artifacts";
    }
    private sealed record ProjectsResponse(List<ProjectInfo>? Projects);
    private sealed record TextResponse(string? Text);
    private sealed record GitResponse(string? Text, string? Status, List<string>? Files);
    private sealed record TaskInfo(string Id, string Title);
    private sealed record TasksResponse(List<TaskInfo>? Commands);
    private sealed record StorageEntryInfo(string Name, string Path, ulong Bytes, [property: JsonPropertyName("is_dir")] bool IsDir)
    {
        public string SizeText => IsDir ? "Folder" : SizeTextValue(Bytes);
    }
    private sealed record StorageResponse(List<StorageEntryInfo>? Entries, ulong Free, ulong Total);
    private sealed record BatchItem(string From, string To);
    private sealed record BatchResponse(List<BatchItem>? Items, string? Text);
    private sealed record ArchiveResponse(string? Path, string? Text);
    private sealed record DiffLine(string UnifiedNumberText, string UnifiedText, string OldNumberText, string OldText, string NewNumberText, string NewText, int HunkIndex, bool IsHunkHeader);
    private sealed record HunkInfo(int Index, string Text, DiffLine Line)
    {
        public string Label => $"Hunk {Index + 1}: {Text}";
    }
    private sealed record ColumnPreference(string Id, bool Visible, double Width, int Order);

    private sealed class ColumnState(string id, string title, double width) : INotifyPropertyChanged
    {
        private bool visible = true;
        private double currentWidth = width;
        public string Id { get; } = id;
        public string Title { get; } = title;
        public bool Visible
        {
            get => visible;
            set { if (visible == value) return; visible = value; PropertyChanged?.Invoke(this, new(nameof(Visible))); }
        }
        public double Width
        {
            get => currentWidth;
            set { value = Math.Clamp(value, 72, 900); if (Math.Abs(currentWidth - value) < 0.1) return; currentWidth = value; PropertyChanged?.Invoke(this, new(nameof(Width))); }
        }
        public event PropertyChangedEventHandler? PropertyChanged;
    }

    private sealed class FileColumnValue(FileItem item, ColumnState column)
    {
        public ColumnState Column { get; } = column;
        public string Text => Column.Id switch
        {
            "name" => item.Name,
            "size" => item.Size,
            "kind" => item.IsDirectory ? "Folder" : "File",
            "path" => item.Path,
            _ => "",
        };
    }

    private sealed class FileListRow(FileItem item, IEnumerable<ColumnState> columns)
    {
        public FileItem Item { get; } = item;
        public IReadOnlyList<FileColumnValue> Columns { get; } = columns.Select(column => new FileColumnValue(item, column)).ToArray();
    }

    private static readonly string columnSettingsPath = System.IO.Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Megaman", "columns-files.json");

    private void LoadColumnState()
    {
        var order = columnStates.ToList();
        try
        {
            if (System.IO.File.Exists(columnSettingsPath))
            {
                var preferences = JsonSerializer.Deserialize<List<ColumnPreference>>(System.IO.File.ReadAllText(columnSettingsPath), jsonOptions) ?? [];
                var byId = columnStates.ToDictionary(column => column.Id, StringComparer.OrdinalIgnoreCase);
                foreach (var preference in preferences)
                {
                    if (!byId.TryGetValue(preference.Id, out var column)) continue;
                    column.Visible = preference.Visible;
                    if (double.IsFinite(preference.Width)) column.Width = preference.Width;
                }
                order = preferences
                    .OrderBy(preference => preference.Order)
                    .Select(preference => byId.GetValueOrDefault(preference.Id))
                    .Where(column => column is not null)
                    .Cast<ColumnState>()
                    .Concat(columnStates.Where(column => !preferences.Any(preference => string.Equals(preference.Id, column.Id, StringComparison.OrdinalIgnoreCase))))
                    .ToList();
            }
        }
        catch { order = columnStates.ToList(); }

        var name = columnStates[0];
        name.Visible = true;
        visibleColumns.Clear();
        foreach (var column in order.Where(column => column.Visible).Distinct()) visibleColumns.Add(column);
    }

    private void SaveColumnState()
    {
        try
        {
            var order = visibleColumns.Select((column, index) => (column.Id, index)).ToDictionary(item => item.Id, item => item.index, StringComparer.OrdinalIgnoreCase);
            var preferences = columnStates.Select(column => new ColumnPreference(column.Id, column.Visible, column.Width, order.GetValueOrDefault(column.Id, int.MaxValue))).ToArray();
            System.IO.Directory.CreateDirectory(System.IO.Path.GetDirectoryName(columnSettingsPath)!);
            System.IO.File.WriteAllText(columnSettingsPath, JsonSerializer.Serialize(preferences, jsonOptions));
        }
        catch { }
    }

    private void BindFileListRows() => FileList.ItemsSource = fileRows.Select(row => new FileListRow(row, visibleColumns)).ToArray();

    private IReadOnlyList<FileItem> SortFileRows(IReadOnlyList<FileItem> rows)
    {
        if (columnSortId is null) return rows;
        IOrderedEnumerable<FileItem> sorted = columnSortId switch
        {
            "size" => columnSortDescending ? rows.OrderByDescending(row => row.Bytes) : rows.OrderBy(row => row.Bytes),
            "kind" => columnSortDescending ? rows.OrderByDescending(row => row.IsDirectory) : rows.OrderBy(row => row.IsDirectory),
            "path" => columnSortDescending ? rows.OrderByDescending(row => row.Path, StringComparer.CurrentCultureIgnoreCase) : rows.OrderBy(row => row.Path, StringComparer.CurrentCultureIgnoreCase),
            _ => columnSortDescending ? rows.OrderByDescending(row => row.Name, StringComparer.CurrentCultureIgnoreCase) : rows.OrderBy(row => row.Name, StringComparer.CurrentCultureIgnoreCase),
        };
        return sorted.ThenBy(row => row.Name, StringComparer.CurrentCultureIgnoreCase).ToArray();
    }

    private static FileItem? AsFileItem(object? value) => value switch
    {
        FileItem row => row,
        FileListRow row => row.Item,
        _ => null,
    };

    private FileListRow? ListRowFor(FileItem row) =>
        (FileList.ItemsSource as IEnumerable<FileListRow>)?.FirstOrDefault(item => ReferenceEquals(item.Item, row));

    private static ColumnState? HeaderColumn(DependencyObject? source)
    {
        for (var current = source; current is not null; current = VisualTreeHelper.GetParent(current))
            if (current is FrameworkElement { Tag: ColumnState column }) return column;
        return null;
    }

    private void ColumnPointerPressed(object sender, PointerRoutedEventArgs e)
    {
        if (e.OriginalSource is DependencyObject source && HeaderColumn(source) is not null && source is Thumb) return;
        if ((sender as FrameworkElement)?.Tag is not ColumnState column || !e.GetCurrentPoint((UIElement)sender).Properties.IsLeftButtonPressed) return;
        draggedColumn = column;
        columnDragStartX = e.GetCurrentPoint(FileColumnsHeader).Position.X;
        FileColumnsHeader.CapturePointer(e.Pointer);
    }

    private void ColumnPointerMoved(object sender, PointerRoutedEventArgs e)
    {
        if (draggedColumn is null || !e.GetCurrentPoint(FileColumnsHeader).Properties.IsLeftButtonPressed) return;
        var index = visibleColumns.IndexOf(draggedColumn);
        if (index < 0) return;
        var position = e.GetCurrentPoint(FileColumnsHeader).Position.X;
        while (index < visibleColumns.Count - 1 && position - columnDragStartX > visibleColumns[index + 1].Width / 2)
        {
            var crossedWidth = visibleColumns[index + 1].Width;
            visibleColumns.Move(index, index + 1);
            columnDragStartX += crossedWidth;
            index++;
        }
        while (index > 0 && columnDragStartX - position > visibleColumns[index - 1].Width / 2)
        {
            var crossedWidth = visibleColumns[index - 1].Width;
            visibleColumns.Move(index, index - 1);
            columnDragStartX -= crossedWidth;
            index--;
        }
    }

    private void ColumnPointerReleased(object sender, PointerRoutedEventArgs e)
    {
        if (draggedColumn is null) return;
        var selectedPaths = SelectedRows().Select(row => row.Path).ToHashSet(StringComparer.OrdinalIgnoreCase);
        draggedColumn = null;
        FileColumnsHeader.ReleasePointerCapture(e.Pointer);
        SaveColumnState();
        BindFileListRows();
        RestoreSelectionPaths(selectedPaths);
    }

    private void ColumnWidthDragged(object sender, DragDeltaEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is ColumnState column) column.Width += e.HorizontalChange;
    }

    private void ColumnWidthDragCompleted(object sender, DragCompletedEventArgs e) => SaveColumnState();

    private void FileColumnClicked(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is not ColumnState column) return;
        if (columnSortId == column.Id) columnSortDescending = !columnSortDescending;
        else
        {
            columnSortId = column.Id;
            columnSortDescending = false;
        }
        var selectedPaths = SelectedRows().Select(row => row.Path).ToHashSet(StringComparer.OrdinalIgnoreCase);
        fileRows = SortFileRows(fileRows);
        Files.ItemsSource = fileRows;
        BindFileListRows();
        RestoreSelectionPaths(selectedPaths);
    }

    private void RestorePendingSelection()
    {
        if (pendingSelectionPaths is not { Count: > 0 } paths) return;
        if (ListMode.IsChecked == true)
        {
            foreach (var row in fileRows)
                if (paths.Contains(row.Path) && ListRowFor(row) is { } listRow) FileList.SelectedItems.Add(listRow);
        }
        else
        {
            foreach (var row in fileRows)
                if (paths.Contains(row.Path)) Files.SelectedItems.Add(row);
        }
        pendingSelectionPaths = null;
    }

    private void RestoreSelectionPaths(IReadOnlySet<string> paths)
    {
        if (ListMode.IsChecked == true)
        {
            foreach (var row in fileRows)
                if (paths.Contains(row.Path) && ListRowFor(row) is { } listRow) FileList.SelectedItems.Add(listRow);
        }
        else
        {
            foreach (var row in fileRows)
                if (paths.Contains(row.Path)) Files.SelectedItems.Add(row);
        }
    }

    private async void ColumnsClicked(object sender, RoutedEventArgs e)
    {
        var checks = columnStates.Select(column => new CheckBox { Content = column.Title, IsChecked = column.Visible, Tag = column }).ToArray();
        var dialog = new ContentDialog
        {
            Title = "File list columns",
            Content = new StackPanel { Spacing = 8, Children = { new TextBlock { Text = "Drag headers to reorder. Choose which columns remain visible." }, new ItemsControl { ItemsSource = checks } } },
            PrimaryButtonText = "Apply",
            CloseButtonText = "Cancel",
            XamlRoot = Shell.XamlRoot,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
        if (checks.All(check => check.IsChecked != true))
        {
            ShowError("Keep at least one file column visible.");
            return;
        }
        var previousOrder = visibleColumns.ToArray();
        foreach (var check in checks)
            if (check.Tag is ColumnState column) column.Visible = check.IsChecked == true;
        visibleColumns.Clear();
        foreach (var column in previousOrder.Where(column => column.Visible)) visibleColumns.Add(column);
        foreach (var column in columnStates.Where(column => column.Visible && !visibleColumns.Contains(column))) visibleColumns.Add(column);
        SaveColumnState();
        var selectedPaths = SelectedRows().Select(row => row.Path).ToHashSet(StringComparer.OrdinalIgnoreCase);
        BindFileListRows();
        RestoreSelectionPaths(selectedPaths);
    }

    private async Task LoadNativeComponentsAsync(NativeManager current)
    {
        try
        {
            var result = await RunComponentAsync("shell", "{}", () => current.Component("shell", "{}"), lifetime.Token);
            if (!result.Succeeded)
            {
                ShowError(result.Error, "Megaman could not load its native command registry.");
                return;
            }

            var shell = JsonSerializer.Deserialize<ShellResponse>(result.Json, jsonOptions);
            var commands = shell?.Components?.SelectMany(component => (component.Commands ?? []).Select(command => command.WithComponent(component.Id, component.Icon))).ToArray() ?? [];
            NativeCommands.ItemsSource = commands;
            NativeCommands.Visibility = commands.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        }
        catch (OperationCanceledException) { }
        catch (JsonException ex) { ShowError(ex.Message, "Megaman received an invalid native command registry."); }
        catch (Exception ex) { ShowError(ex.Message, "Megaman could not load its native command registry."); }
    }

    private async Task<NativeComponentResult?> CallComponentAsync(string component, object request, string fallback)
    {
        var current = manager;
        if (current is null || lifetime.IsCancellationRequested) return null;
        try
        {
            var requestJson = JsonSerializer.Serialize(request, jsonOptions);
            var result = await RunComponentAsync(component, requestJson, () => current.Component(component, requestJson), lifetime.Token);
            if (!result.Succeeded) ShowError(result.Error, fallback);
            return result;
        }
        catch (OperationCanceledException) { return null; }
        catch (Exception ex) { ShowError(ex.Message, fallback); return null; }
    }

    private void ShowComponentPanel(string title, StackPanel panel)
    {
        ComponentTitle.Text = title;
        ComponentHostPanel.Visibility = Visibility.Visible;
        PreviewPanel.Visibility = Visibility.Collapsed;
        ChartCanvas.Visibility = Visibility.Collapsed;
        ProjectsViewPanel.Visibility = Visibility.Collapsed;
        GitViewPanel.Visibility = Visibility.Collapsed;
        TasksViewPanel.Visibility = Visibility.Collapsed;
        StorageViewPanel.Visibility = Visibility.Collapsed;
        ArchiveViewPanel.Visibility = Visibility.Collapsed;
        panel.Visibility = Visibility.Visible;
    }

    private void CloseComponentPanel()
    {
        ComponentHostPanel.Visibility = Visibility.Collapsed;
        var chartVisible = ChartButton.IsChecked == true;
        ChartCanvas.Visibility = chartVisible ? Visibility.Visible : Visibility.Collapsed;
        PreviewPanel.Visibility = chartVisible ? Visibility.Collapsed : Visibility.Visible;
    }

    private async void NativeCommandClicked(object sender, RoutedEventArgs e)
    {
        if ((sender as Button)?.Tag is not ComponentCommandInfo command) return;
        switch (command.Component)
        {
            case "projects":
                if (command.Id == "list") await LoadProjectsAsync(true);
                else if (command.Id == "workspace") await LoadProjectsAsync();
                else if (command.Id is "open" && selectedProject is not null) OpenProjectWindow(selectedProject);
                break;
            case "git":
                await RunGitActionAsync(command.Id);
                break;
            case "tasks":
                if (command.Id is "list" or "run") await LoadTasksAsync();
                break;
            case "storage":
                if (command.Id == "map") await LoadStorageAsync();
                break;
            case "archives":
                if (command.Id == "open") await OpenArchiveAsync();
                else if (command.Id == "compress") await CompressSelectionAsync();
                else if (command.Id == "extract") await ExtractArchiveAsync();
                else if (command.Id == "save") await SaveArchiveAsync();
                break;
            case "batch":
                if (command.Id == "rename_preview") await BatchRenameAsync(false);
                else if (command.Id == "rename") await BatchRenameAsync(true);
                else if (command.Id is "copy" or "move") await BatchTransferAsync(command.Id);
                break;
            default:
                var result = await CallComponentAsync(command.Component, new { }, "Megaman could not run this native command.");
                if (result is { Succeeded: true }) ShowSuccess($"{command.Title} completed.");
                break;
        }
    }

    private async Task LoadProjectsAsync(bool refresh = false)
    {
        var result = await CallComponentAsync("projects", new { action = refresh ? "refresh" : "list" }, refresh ? "Megaman could not refresh projects." : "Megaman could not load projects.");
        if (result is null or { Succeeded: false }) return;
        try
        {
            var payload = JsonSerializer.Deserialize<ProjectsResponse>(result.Value.Json, jsonOptions);
            selectedProject = null;
            ProjectsList.ItemsSource = payload?.Projects ?? [];
            ShowComponentPanel("Projects", ProjectsViewPanel);
        }
        catch (JsonException ex) { ShowError(ex.Message, "Megaman received an invalid projects response."); }
    }

    private async void ProjectsRefreshClicked(object sender, RoutedEventArgs e) => await LoadProjectsAsync(true);

    private void ProjectClicked(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is ProjectInfo project) selectedProject = project;
    }

    private void ProjectDoubleTapped(object sender, DoubleTappedRoutedEventArgs e)
    {
        if (ProjectsList.SelectedItem is ProjectInfo project) OpenProjectWindow(project);
    }

    private void OpenProjectWindowClicked(object sender, RoutedEventArgs e)
    {
        if (ProjectsList.SelectedItem is ProjectInfo project) OpenProjectWindow(project);
        else ShowError("Select a project first.");
    }

    private void OpenProjectWindow(ProjectInfo project)
    {
        if (string.IsNullOrWhiteSpace(project.Path))
        {
            ShowError("The selected project has no folder path.");
            return;
        }
        try
        {
            var window = new MainWindow(project.Path);
            projectWindows.Add(window);
            window.Closed += (_, _) => projectWindows.Remove(window);
            window.Activate();
        }
        catch (Exception ex) { ShowError(ex.Message, "Megaman could not open the project window."); }
    }

    private async Task RunGitActionAsync(string action)
    {
        if (action is not ("status" or "diff" or "stage" or "unstage")) return;
        var currentManager = manager;
        if (currentManager is null) return;
        var path = selectedProject?.Path;
        if (string.IsNullOrWhiteSpace(path)) path = await CurrentDirectoryAsync();
        if (string.IsNullOrWhiteSpace(path)) return;
        var generation = Volatile.Read(ref refreshGeneration);

        var request = new Dictionary<string, object?>
        {
            ["action"] = action,
            ["path"] = path,
            ["staged"] = action == "unstage",
        };
        var rows = SelectedRows();
        if (rows.Count == 1 && !rows[0].IsDirectory)
        {
            var relative = System.IO.Path.GetRelativePath(path, rows[0].Path);
            if (!relative.StartsWith("..", StringComparison.Ordinal)) request["file"] = relative;
        }

        var result = await CallComponentAsync("git", request, $"Megaman could not run git {action}.");
        if (result is null or { Succeeded: false }) return;
        try
        {
            var payload = JsonSerializer.Deserialize<GitResponse>(result.Value.Json, jsonOptions);
            var text = payload?.Text ?? result.Value.Json;
            GitSummary.Text = $"{action} · {path}";
            if (payload?.Status is { } status) ApplyGitFooterStatus(currentManager, path, status, generation);
            RenderDiff(text);
            ShowComponentPanel("Git", GitViewPanel);
            if (action is "stage" or "unstage") Refresh(true);
        }
        catch (JsonException ex) { ShowError(ex.Message, "Megaman received an invalid git response."); }
    }

    private void RequestGitFooterRefresh(NativeManager current, string path, int generation, CancellationToken cancellation, bool force)
    {
        if (string.IsNullOrWhiteSpace(path) || lifetime.IsCancellationRequested) return;
        var now = DateTimeOffset.UtcNow;
        var samePath = SamePath(gitFooterPath, path);
        if (!force && samePath && (gitFooterRequestInFlight || now - gitFooterUpdated < TimeSpan.FromSeconds(5))) return;
        var requestId = ++gitFooterRequestId;
        gitFooterRequestInFlight = true;
        _ = RefreshGitFooterAsync(current, path, generation, cancellation, requestId);
    }

    private async Task RefreshGitFooterAsync(NativeManager current, string path, int generation, CancellationToken cancellation, long requestId)
    {
        try
        {
            var requestJson = JsonSerializer.Serialize(new { action = "status", path }, jsonOptions);
            var result = await RunComponentAsync("git", requestJson, () => current.Component("git", requestJson), cancellation);
            if (requestId != gitFooterRequestId || !IsCurrentGitPath(current, path, generation, cancellation)) return;
            gitFooterPath = path;
            gitFooterUpdated = DateTimeOffset.UtcNow;
            if (!result.Succeeded) { GitFooterSummary.Text = ""; return; }
            var payload = JsonSerializer.Deserialize<GitResponse>(result.Json, jsonOptions);
            GitFooterSummary.Text = FormatGitStatus(payload?.Status);
        }
        catch (OperationCanceledException) { }
        catch (JsonException) { }
        catch { }
        finally
        {
            if (gitFooterRequestId == requestId) gitFooterRequestInFlight = false;
        }
    }

    private void ApplyGitFooterStatus(NativeManager current, string path, string status, int generation)
    {
        if (!IsCurrentGitPath(current, path, generation, lifetime.Token)) return;
        gitFooterPath = path;
        gitFooterUpdated = DateTimeOffset.UtcNow;
        GitFooterSummary.Text = FormatGitStatus(status);
    }

    private bool IsCurrentGitPath(NativeManager current, string path, int generation, CancellationToken cancellation) =>
        ReferenceEquals(manager, current) && IsCurrent(generation, cancellation) && SamePath(AddressBar.Text, path);

    private static bool SamePath(string? left, string? right) =>
        !string.IsNullOrWhiteSpace(left) && !string.IsNullOrWhiteSpace(right) &&
        string.Equals(left.TrimEnd('\\', '/'), right.TrimEnd('\\', '/'), StringComparison.OrdinalIgnoreCase);

    private static string FormatGitStatus(string? status)
    {
        if (string.IsNullOrWhiteSpace(status)) return "";
        var lines = status.Replace("\r", "").Split('\n', StringSplitOptions.RemoveEmptyEntries);
        var branchLine = lines.FirstOrDefault(line => line.StartsWith("## ", StringComparison.Ordinal));
        if (branchLine is null) return "";
        var branch = branchLine[3..].Trim();
        var upstream = branch.IndexOf("...", StringComparison.Ordinal);
        if (upstream >= 0) branch = branch[..upstream];
        var tracking = branch.IndexOf(" [", StringComparison.Ordinal);
        if (tracking >= 0) branch = branch[..tracking];
        if (branch.Length == 0) return "";
        var changed = lines.Count(line => !line.StartsWith("## ", StringComparison.Ordinal) && !string.IsNullOrWhiteSpace(line));
        return changed == 0 ? $"{branch} · clean" : $"{branch} · {changed} change{(changed == 1 ? "" : "s")}";
    }

    private async void GitStatusClicked(object sender, RoutedEventArgs e) => await RunGitActionAsync("status");
    private async void GitDiffClicked(object sender, RoutedEventArgs e) => await RunGitActionAsync("diff");
    private async void GitStageClicked(object sender, RoutedEventArgs e) => await RunGitActionAsync("stage");
    private async void GitUnstageClicked(object sender, RoutedEventArgs e) => await RunGitActionAsync("unstage");

    private void GitModeChanged(object sender, SelectionChangedEventArgs e)
    {
        ApplyGitMode();
    }

    private void ApplyGitMode()
    {
        if (GitUnifiedView is null) return;
        var split = GitMode.SelectedItem is ComboBoxItem item && item.Tag?.ToString() == "split";
        GitUnifiedView.Visibility = split ? Visibility.Collapsed : Visibility.Visible;
        GitSplitView.Visibility = split ? Visibility.Visible : Visibility.Collapsed;
    }

    private void RenderDiff(string text)
    {
        diffLines = ParseDiff(text);
        collapsedHunks.Clear();
        GitHunkMenu.ItemsSource = diffLines.Where(line => line.IsHunkHeader).Select(line => new HunkInfo(line.HunkIndex, line.UnifiedText, line)).ToArray();
        GitHunkMenu.SelectedIndex = -1;
        RenderVisibleDiff();
        ApplyGitMode();
    }

    private void RenderVisibleDiff()
    {
        var visible = diffLines.Where(line => line.IsHunkHeader || !collapsedHunks.Contains(line.HunkIndex)).ToArray();
        GitUnifiedLines.ItemsSource = visible;
        GitSplitLeftLines.ItemsSource = visible;
        GitSplitRightLines.ItemsSource = visible;
    }

    private static IReadOnlyList<DiffLine> ParseDiff(string text)
    {
        var oldLine = 0;
        var newLine = 0;
        var hunkIndex = -1;
        var result = new List<DiffLine>();
        foreach (var raw in text.Replace("\r", "").Split('\n'))
        {
            if (raw.StartsWith("@@", StringComparison.Ordinal))
            {
                ParseHunkHeader(raw, ref oldLine, ref newLine);
                hunkIndex++;
                result.Add(new DiffLine("", raw, "", raw, "", raw, hunkIndex, true));
                continue;
            }
            if (raw.StartsWith("diff ", StringComparison.Ordinal) || raw.StartsWith("index ", StringComparison.Ordinal) || raw.StartsWith("--- ", StringComparison.Ordinal) || raw.StartsWith("+++ ", StringComparison.Ordinal))
            {
                result.Add(new DiffLine("", raw, "", raw, "", raw, hunkIndex, false));
                continue;
            }

            var marker = raw.Length == 0 ? ' ' : raw[0];
            var content = raw.Length > 0 && marker is '+' or '-' or ' ' ? raw[1..] : raw;
            if (marker == '+')
            {
                result.Add(new DiffLine($"+{newLine}", raw, "", "", newLine.ToString(), content, hunkIndex, false));
                newLine++;
            }
            else if (marker == '-')
            {
                result.Add(new DiffLine($"-{oldLine}", raw, oldLine.ToString(), content, "", "", hunkIndex, false));
                oldLine++;
            }
            else
            {
                result.Add(new DiffLine($"{oldLine}/{newLine}", raw, oldLine.ToString(), content, newLine.ToString(), content, hunkIndex, false));
                oldLine++;
                newLine++;
            }
        }
        return result;
    }

    private void DiffLineTapped(object sender, TappedRoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is not DiffLine { IsHunkHeader: true } line) return;
        if (!collapsedHunks.Add(line.HunkIndex)) collapsedHunks.Remove(line.HunkIndex);
        RenderVisibleDiff();
        e.Handled = true;
    }

    private void GitHunkChanged(object sender, SelectionChangedEventArgs e)
    {
        if (GitHunkMenu.SelectedItem is not HunkInfo hunk) return;
        (GitUnifiedLines.ContainerFromItem(hunk.Line) as UIElement)?.StartBringIntoView();
        (GitSplitLeftLines.ContainerFromItem(hunk.Line) as UIElement)?.StartBringIntoView();
        (GitSplitRightLines.ContainerFromItem(hunk.Line) as UIElement)?.StartBringIntoView();
    }

    private void PreviousHunkClicked(object sender, RoutedEventArgs e)
    {
        if (GitHunkMenu.Items.Count == 0) return;
        GitHunkMenu.SelectedIndex = Math.Max(0, GitHunkMenu.SelectedIndex <= 0 ? 0 : GitHunkMenu.SelectedIndex - 1);
    }

    private void NextHunkClicked(object sender, RoutedEventArgs e)
    {
        if (GitHunkMenu.Items.Count == 0) return;
        GitHunkMenu.SelectedIndex = Math.Min(GitHunkMenu.Items.Count - 1, GitHunkMenu.SelectedIndex + 1);
    }

    private void CollapseHunksClicked(object sender, RoutedEventArgs e)
    {
        foreach (var line in diffLines.Where(line => line.IsHunkHeader)) collapsedHunks.Add(line.HunkIndex);
        RenderVisibleDiff();
    }

    private void ExpandHunksClicked(object sender, RoutedEventArgs e)
    {
        collapsedHunks.Clear();
        RenderVisibleDiff();
    }

    private static void ParseHunkHeader(string value, ref int oldLine, ref int newLine)
    {
        var parts = value.Split(' ');
        foreach (var part in parts)
        {
            if (part.StartsWith("-", StringComparison.Ordinal)) oldLine = ParseLineNumber(part[1..]);
            else if (part.StartsWith("+", StringComparison.Ordinal)) newLine = ParseLineNumber(part[1..]);
        }
    }

    private static int ParseLineNumber(string value)
    {
        var comma = value.IndexOf(',');
        return int.TryParse(comma < 0 ? value : value[..comma], out var line) ? line : 0;
    }

    private async Task LoadTasksAsync()
    {
        var path = selectedProject?.Path ?? await CurrentDirectoryAsync();
        if (string.IsNullOrWhiteSpace(path)) return;
        var result = await CallComponentAsync("tasks", new { action = "list", path }, "Megaman could not list project tasks.");
        if (result is null or { Succeeded: false }) return;
        try
        {
            var payload = JsonSerializer.Deserialize<TasksResponse>(result.Value.Json, jsonOptions);
            TaskCommands.ItemsSource = payload?.Commands ?? [];
            TaskOutput.Text = "";
            ShowComponentPanel("Tasks", TasksViewPanel);
        }
        catch (JsonException ex) { ShowError(ex.Message, "Megaman received an invalid tasks response."); }
    }

    private async void TaskCommandClicked(object sender, RoutedEventArgs e)
    {
        if ((sender as Button)?.Tag is not TaskInfo task) return;
        var path = selectedProject?.Path ?? await CurrentDirectoryAsync();
        if (string.IsNullOrWhiteSpace(path)) return;
        var result = await CallComponentAsync("tasks", new { action = "run", path, command = task.Id }, $"Megaman could not run {task.Title}.");
        if (result is null or { Succeeded: false }) return;
        try
        {
            var payload = JsonSerializer.Deserialize<TextResponse>(result.Value.Json, jsonOptions);
            TaskOutput.Text = payload?.Text ?? result.Value.Json;
            ShowComponentPanel("Tasks", TasksViewPanel);
        }
        catch (JsonException ex) { ShowError(ex.Message, "Megaman received an invalid task response."); }
    }

    private async Task LoadStorageAsync()
    {
        var path = await CurrentDirectoryAsync();
        if (string.IsNullOrWhiteSpace(path)) return;
        var result = await CallComponentAsync("storage", new { action = "map", path }, "Megaman could not map this storage location.");
        if (result is null or { Succeeded: false }) return;
        try
        {
            var payload = JsonSerializer.Deserialize<StorageResponse>(result.Value.Json, jsonOptions);
            StorageEntries.ItemsSource = payload?.Entries ?? [];
            StorageSummary.Text = $"Folder entries · Free {SizeTextValue(payload?.Free ?? 0)} · Total {SizeTextValue(payload?.Total ?? 0)}";
            ShowComponentPanel("Storage map", StorageViewPanel);
        }
        catch (JsonException ex) { ShowError(ex.Message, "Megaman received an invalid storage response."); }
    }

    private async void StorageEntryClicked(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is not StorageEntryInfo entry || !entry.IsDir) return;
        CloseComponentPanel();
        await NavigateToAsync(entry.Path);
    }

    private async Task<string?> CurrentDirectoryAsync()
    {
        var current = manager;
        if (current is null || lifetime.IsCancellationRequested) return null;
        try { return await RunNativeAsync(() => current.Directory, lifetime.Token); }
        catch (OperationCanceledException) { return null; }
        catch (Exception ex) { ShowError(ex.Message, "Megaman could not read the current location."); return null; }
    }

    private async Task OpenArchiveAsync()
    {
        var rows = SelectedRows();
        if (rows.Count != 1 || rows[0].IsDirectory)
        {
            ShowError("Select one archive file to open.");
            return;
        }
        var archive = rows[0].Path;
        var result = await CallComponentAsync("archives", new { action = "open", path = archive }, "Megaman could not open this archive.");
        if (result is null or { Succeeded: false }) return;
        try
        {
            var payload = JsonSerializer.Deserialize<ArchiveResponse>(result.Value.Json, jsonOptions);
            archiveWorkspacePath = payload?.Path;
            ArchiveSummary.Text = $"{archive}{(string.IsNullOrWhiteSpace(archiveWorkspacePath) ? "" : $"\nWorkspace: {archiveWorkspacePath}")}";
            ArchiveOutput.Text = payload?.Text ?? result.Value.Json;
            ShowComponentPanel("Archive", ArchiveViewPanel);
            if (!string.IsNullOrWhiteSpace(archiveWorkspacePath)) await NavigateToAsync(archiveWorkspacePath);
        }
        catch (JsonException ex) { ShowError(ex.Message, "Megaman received an invalid archive response."); }
    }

    private async Task CompressSelectionAsync()
    {
        var paths = SelectedRows().Select(row => row.Path).Where(path => !string.IsNullOrWhiteSpace(path)).Distinct(StringComparer.OrdinalIgnoreCase).ToArray();
        if (paths.Length == 0)
        {
            ShowError("Select one or more files or folders to compress.");
            return;
        }
        var current = await CurrentDirectoryAsync();
        var destination = new TextBox { Text = System.IO.Path.Combine(current ?? "", "archive.zip"), PlaceholderText = "New archive path" };
        var dialog = new ContentDialog
        {
            Title = "Compress selection",
            Content = destination,
            PrimaryButtonText = "Compress",
            CloseButtonText = "Cancel",
            XamlRoot = Shell.XamlRoot,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary || string.IsNullOrWhiteSpace(destination.Text)) return;
        var result = await CallComponentAsync("archives", new { action = "compress", paths, destination = destination.Text.Trim() }, "Megaman could not create the archive.");
        if (result is null or { Succeeded: false }) return;
        ArchiveOutput.Text = ArchiveResultText(result.Value.Json);
        ArchiveSummary.Text = destination.Text.Trim();
        ShowComponentPanel("Archive", ArchiveViewPanel);
        ShowSuccess("Archive created.");
        Refresh(true);
    }

    private async Task ExtractArchiveAsync()
    {
        var rows = SelectedRows();
        if (rows.Count != 1 || rows[0].IsDirectory)
        {
            ShowError("Select one archive file to extract.");
            return;
        }
        var current = await CurrentDirectoryAsync();
        var destination = new TextBox { Text = System.IO.Path.Combine(current ?? "", "extracted"), PlaceholderText = "Destination folder" };
        var dialog = new ContentDialog
        {
            Title = "Extract archive",
            Content = destination,
            PrimaryButtonText = "Extract",
            CloseButtonText = "Cancel",
            XamlRoot = Shell.XamlRoot,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary || string.IsNullOrWhiteSpace(destination.Text)) return;
        var result = await CallComponentAsync("archives", new { action = "extract", path = rows[0].Path, destination = destination.Text.Trim() }, "Megaman could not extract this archive.");
        if (result is null or { Succeeded: false }) return;
        ArchiveOutput.Text = ArchiveResultText(result.Value.Json);
        ArchiveSummary.Text = destination.Text.Trim();
        ShowComponentPanel("Archive", ArchiveViewPanel);
        ShowSuccess("Archive extracted.");
        Refresh(true);
    }

    private async Task SaveArchiveAsync()
    {
        if (string.IsNullOrWhiteSpace(archiveWorkspacePath))
        {
            ShowError("Open an archive workspace before saving changes.");
            return;
        }
        var result = await CallComponentAsync("archives", new { action = "save", path = archiveWorkspacePath }, "Megaman could not save this archive.");
        if (result is null or { Succeeded: false }) return;
        ArchiveOutput.Text = ArchiveResultText(result.Value.Json);
        ShowComponentPanel("Archive", ArchiveViewPanel);
        ShowSuccess("Archive changes saved.");
    }

    private async void ArchiveSaveClicked(object sender, RoutedEventArgs e) => await SaveArchiveAsync();

    private static string ArchiveResultText(string json)
    {
        try
        {
            var payload = JsonSerializer.Deserialize<ArchiveResponse>(json, jsonOptions);
            return payload?.Text ?? json;
        }
        catch (JsonException) { return json; }
    }

    private async Task BatchRenameAsync(bool applyAfterPreview)
    {
        var paths = SelectedRows().Select(row => row.Path).Where(path => !string.IsNullOrWhiteSpace(path)).Distinct(StringComparer.OrdinalIgnoreCase).ToArray();
        if (paths.Length == 0)
        {
            ShowError("Select one or more files or folders for the batch rename.");
            return;
        }

        var find = new TextBox { PlaceholderText = "Text to find" };
        var replace = new TextBox { PlaceholderText = "Replacement text" };
        var prefix = new TextBox { PlaceholderText = "Prefix" };
        var suffix = new TextBox { PlaceholderText = "Suffix" };
        var content = new StackPanel { Spacing = 8, Children = { find, replace, prefix, suffix, new TextBlock { Text = $"{paths.Length} selected item{(paths.Length == 1 ? "" : "s")}" } } };
        var dialog = new ContentDialog
        {
            Title = "Batch rename preview",
            Content = content,
            PrimaryButtonText = "Preview",
            CloseButtonText = "Cancel",
            XamlRoot = Shell.XamlRoot,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;

        var request = new Dictionary<string, object?>
        {
            ["action"] = "rename_preview",
            ["paths"] = paths,
            ["destination"] = "",
            ["find"] = find.Text,
            ["replace"] = replace.Text,
            ["prefix"] = prefix.Text,
            ["suffix"] = suffix.Text,
            ["start"] = 1,
        };
        var preview = await CallComponentAsync("batch", request, "Megaman could not preview the batch rename.");
        if (preview is null or { Succeeded: false }) return;
        var previewText = BatchPreviewText(preview.Value.Json);
        var applyDialog = new ContentDialog
        {
            Title = applyAfterPreview ? "Apply batch rename?" : "Batch rename preview",
            Content = new ScrollViewer { MaxHeight = 420, Content = new TextBlock { Text = previewText, IsTextSelectionEnabled = true, TextWrapping = TextWrapping.NoWrap, FontFamily = new Microsoft.UI.Xaml.Media.FontFamily("Consolas") } },
            PrimaryButtonText = applyAfterPreview ? "Apply" : "Close",
            CloseButtonText = "Cancel",
            XamlRoot = Shell.XamlRoot,
        };
        if (await applyDialog.ShowAsync() != ContentDialogResult.Primary) return;
        if (!applyAfterPreview) return;

        request["action"] = "rename";
        var applied = await CallComponentAsync("batch", request, "Megaman could not apply the batch rename.");
        if (applied is { Succeeded: true })
        {
            ShowSuccess("Batch rename applied.");
            Refresh(true);
        }
    }

    private async Task BatchTransferAsync(string action)
    {
        var paths = SelectedRows().Select(row => row.Path).Where(path => !string.IsNullOrWhiteSpace(path)).Distinct(StringComparer.OrdinalIgnoreCase).ToArray();
        if (paths.Length == 0)
        {
            ShowError("Select one or more files or folders first.");
            return;
        }
        var destination = new TextBox { PlaceholderText = "Destination folder path" };
        var dialog = new ContentDialog
        {
            Title = action == "copy" ? "Copy selected items" : "Move selected items",
            Content = destination,
            PrimaryButtonText = action == "copy" ? "Copy" : "Move",
            CloseButtonText = "Cancel",
            XamlRoot = Shell.XamlRoot,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary || string.IsNullOrWhiteSpace(destination.Text)) return;
        var result = await CallComponentAsync("batch", new { action, paths, destination = destination.Text.Trim(), find = "", replace = "", prefix = "", suffix = "", start = 1 }, $"Megaman could not {action} the selected items.");
        if (result is { Succeeded: true })
        {
            ShowSuccess($"Batch {action} completed.");
            Refresh(true);
        }
    }

    private static string BatchPreviewText(string json)
    {
        try
        {
            var payload = JsonSerializer.Deserialize<BatchResponse>(json, jsonOptions);
            if (!string.IsNullOrWhiteSpace(payload?.Text)) return payload.Text;
            return string.Join(Environment.NewLine, payload?.Items?.Select(item => $"{item.From}  →  {item.To}") ?? []);
        }
        catch (JsonException) { return json; }
    }

    private async Task InitializeAsync()
    {
        try
        {
            var current = await RunNativeAsync(() => new NativeManager(initialDirectory ?? Environment.GetFolderPath(Environment.SpecialFolder.UserProfile)), lifetime.Token);
            if (lifetime.IsCancellationRequested)
            {
                current.Dispose();
                return;
            }
            manager = current;
            folderSizeRevision = NativeManager.FolderSizesRevision();
            StartFolderSizePolling();
            await LoadNativeComponentsAsync(current);
            Refresh(true);
        }
        catch (OperationCanceledException) { }
        catch (Exception ex) { ShowError(ex.Message, "Megaman could not start its native manager."); }
    }

    private async Task CloseAsync()
    {
        folderSizeTimer?.Stop();
        folderSizeTimer = null;
        StopFolderWatcher();
        lifetime.Cancel();
        refreshCancellation?.Cancel();
        var current = Interlocked.Exchange(ref manager, null);
        if (current is not null) await RunNativeAsync(current.Dispose, CancellationToken.None);
        refreshCancellation?.Dispose();
        lifetime.Dispose();
    }

    private void StartFolderSizePolling()
    {
        var queue = uiQueue;
        if (queue is null) return;
        folderSizeTimer = queue.CreateTimer();
        folderSizeTimer.Interval = TimeSpan.FromSeconds(1);
        folderSizeTimer.Tick += FolderSizeTimerTick;
        folderSizeTimer.Start();
    }

    private void FolderSizeTimerTick(WinUIDispatcherQueueTimer sender, object args)
    {
        var revision = NativeManager.FolderSizesRevision();
        if (revision == 0 || revision == folderSizeRevision || manager is null || lifetime.IsCancellationRequested) return;
        folderSizeRevision = revision;
        pendingSelectionPaths = SelectedRows().Select(row => row.Path).ToHashSet(StringComparer.OrdinalIgnoreCase);
        Refresh();
        if (StorageViewPanel.Visibility == Visibility.Visible) _ = LoadStorageAsync();
    }

    private void RebindFolderWatcher(string directory, bool global)
    {
        StopFolderWatcher();
        if (global || string.IsNullOrWhiteSpace(directory) || !System.IO.Directory.Exists(directory)) return;
        try
        {
            var watcher = new System.IO.FileSystemWatcher(directory)
            {
                IncludeSubdirectories = false,
                NotifyFilter = System.IO.NotifyFilters.FileName | System.IO.NotifyFilters.DirectoryName | System.IO.NotifyFilters.Size | System.IO.NotifyFilters.LastWrite,
                Filter = "*",
            };
            watcher.Created += FolderWatcherChanged;
            watcher.Deleted += FolderWatcherChanged;
            watcher.Renamed += FolderWatcherRenamed;
            watcher.Changed += FolderWatcherChanged;
            watcher.Error += FolderWatcherError;
            watcher.EnableRaisingEvents = true;
            folderWatcher = watcher;
        }
        catch { }
    }

    private void StopFolderWatcher()
    {
        System.IO.FileSystemWatcher? watcher;
        lock (folderWatcherGate)
        {
            watcher = folderWatcher;
            folderWatcher = null;
            folderWatchDebounce?.Cancel();
            folderWatchDebounce = null;
        }
        if (watcher is null) return;
        watcher.EnableRaisingEvents = false;
        watcher.Dispose();
    }

    private void FolderWatcherChanged(object sender, System.IO.FileSystemEventArgs e) => ScheduleFolderRefresh();
    private void FolderWatcherRenamed(object sender, System.IO.RenamedEventArgs e) => ScheduleFolderRefresh();
    private void FolderWatcherError(object sender, System.IO.ErrorEventArgs e) => ScheduleFolderRefresh();

    private void ScheduleFolderRefresh()
    {
        CancellationTokenSource debounce;
        lock (folderWatcherGate)
        {
            if (lifetime.IsCancellationRequested) return;
            folderWatchDebounce?.Cancel();
            debounce = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
            folderWatchDebounce = debounce;
        }
        _ = Task.Run(async () =>
        {
            try
            {
                await Task.Delay(250, debounce.Token);
                if (debounce.IsCancellationRequested) return;
                uiQueue?.TryEnqueue(() =>
                {
                    if (debounce.IsCancellationRequested || lifetime.IsCancellationRequested) return;
                    pendingSelectionPaths = SelectedRows().Select(row => row.Path).ToHashSet(StringComparer.OrdinalIgnoreCase);
                    Refresh(true);
                });
            }
            catch (OperationCanceledException) { }
            finally
            {
                lock (folderWatcherGate)
                {
                    if (ReferenceEquals(folderWatchDebounce, debounce)) folderWatchDebounce = null;
                }
                debounce.Dispose();
            }
        });
    }

    private Task<T> RunNativeAsync<T>(Func<T> operation, CancellationToken cancellation) => nativeTasks.StartNew(operation, cancellation);
    private Task RunNativeAsync(Action operation, CancellationToken cancellation) => nativeTasks.StartNew(operation, cancellation);
    private Task<T> RunComponentAsync<T>(string component, string requestJson, Func<T> operation, CancellationToken cancellation) =>
        (IsLongRunningComponent(component, requestJson) ? longComponentTasks : componentTasks).StartNew(operation, cancellation);

    private static bool IsLongRunningComponent(string component, string requestJson)
    {
        if (component == "archives") return true;
        if (component is not ("tasks" or "batch")) return false;
        try
        {
            using var document = JsonDocument.Parse(requestJson);
            if (!document.RootElement.TryGetProperty("action", out var action)) return false;
            return component == "tasks"
                ? action.GetString() == "run"
                : action.GetString() is "rename" or "copy" or "move";
        }
        catch (JsonException) { return false; }
    }

    private void Refresh(bool refreshGitStatus = false)
    {
        var current = manager;
        if (current is null || lifetime.IsCancellationRequested) return;
        refreshCancellation?.Cancel();
        var cancellation = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
        refreshCancellation = cancellation;
        var generation = Interlocked.Increment(ref refreshGeneration);
        var query = Search.Text ?? "";
        var recursive = Recursive.IsChecked == true;
        var global = GlobalSearch.IsChecked == true;
        var sort = SortMenu.SelectedItem is ComboBoxItem item && uint.TryParse(item.Tag?.ToString(), out var value)
            ? (NativeManager.SortMode)value
            : NativeManager.SortMode.Relevance;
        _ = RefreshAsync(current, generation, query, recursive, global, sort, cancellation.Token, refreshGitStatus);
    }

    private async Task RefreshAsync(NativeManager current, int generation, string query, bool recursive, bool global, NativeManager.SortMode sort, CancellationToken cancellation, bool refreshGitStatus)
    {
        try
        {
            await Task.Delay(120, cancellation);
            var view = await RunNativeAsync(() => current.ReadView(query, recursive, global, sort), cancellation);
            if (!IsCurrent(generation, cancellation)) return;

            var rows = view.Rows;
            var directory = view.Directory;
            fileRows = SortFileRows(rows);
            Files.ItemsSource = fileRows;
            BindFileListRows();
            RestorePendingSelection();
            AddressBar.Text = directory;
            AddressBar.IsEnabled = !global;
            RebindFolderWatcher(directory, global);
            ViewHeading.Text = global ? "Everywhere" : FolderName(directory);
            ResultsSummary.Text = rows.Count == 1 ? "1 item" : $"{rows.Count:n0} items";
            if (view.Succeeded) RequestGitFooterRefresh(current, directory, generation, cancellation, refreshGitStatus);
            _ = LoadThumbnails(rows, generation, cancellation);
            if (ChartButton.IsChecked == true) RefreshChart();
            if (!view.Succeeded) ShowError(view.Error, "Megaman could not load this location.");
        }
        catch (OperationCanceledException) { }
        catch (Exception ex)
        {
            if (IsCurrent(generation, cancellation)) ShowError(ex.Message, "Megaman could not load this location.");
        }
    }

    private bool IsCurrent(int generation, CancellationToken cancellation) =>
        !cancellation.IsCancellationRequested && !lifetime.IsCancellationRequested && generation == Volatile.Read(ref refreshGeneration);

    private async Task LoadThumbnails(IReadOnlyList<FileItem> rows, int generation, CancellationToken cancellation)
    {
        foreach (var row in rows.Take(200))
        {
            if (!IsCurrent(generation, cancellation)) return;
            try
            {
                StorageItemThumbnail thumbnail = row.IsDirectory
                    ? await (await StorageFolder.GetFolderFromPathAsync(row.Path)).GetThumbnailAsync(ThumbnailMode.SingleItem, 160)
                    : await (await StorageFile.GetFileFromPathAsync(row.Path)).GetThumbnailAsync(ThumbnailMode.SingleItem, 160);
                var image = new BitmapImage();
                await image.SetSourceAsync(thumbnail);
                row.Thumbnail = image;
            }
            catch { }
        }
    }

    private void RefreshChart()
    {
        var current = manager;
        if (current is null || lifetime.IsCancellationRequested) return;
        var generation = Volatile.Read(ref refreshGeneration);
        var cancellation = refreshCancellation?.Token ?? lifetime.Token;
        _ = RefreshChartAsync(current, generation, GlobalSearch.IsChecked == true, cancellation);
    }

    private async Task RefreshChartAsync(NativeManager current, int generation, bool global, CancellationToken cancellation)
    {
        try
        {
            var result = await RunNativeAsync(() => current.ReadChart(global), cancellation);
            if (!IsCurrent(generation, cancellation) || ChartButton.IsChecked != true) return;
            chart = result.Rows;
            if (!result.Succeeded)
            {
                ChartCanvas.Children.Clear();
                ShowError(result.Error, "Storage analysis needs an index.");
                return;
            }
            DrawChart();
        }
        catch (OperationCanceledException) { }
        catch (Exception ex)
        {
            if (IsCurrent(generation, cancellation)) ShowError(ex.Message, "Megaman could not read storage analysis.");
        }
    }

    private void DrawChart()
    {
        ChartCanvas.Children.Clear();
        var total = Math.Max(1d, chart.Sum(row => (double)row.Bytes));
        var radius = Math.Max(1, Math.Min(ChartCanvas.ActualWidth, ChartCanvas.ActualHeight) / 2 - 24);
        var center = new global::Windows.Foundation.Point(ChartCanvas.ActualWidth / 2, ChartCanvas.ActualHeight / 2);
        var start = -Math.PI / 2;
        for (var i = 0; i < chart.Count; i++)
        {
            var sweep = chart[i].Bytes / total * Math.PI * 2;
            if (sweep <= 0) continue;
            var end = start + sweep;
            var figure = new PathFigure { StartPoint = center, IsClosed = true };
            figure.Segments.Add(new LineSegment { Point = Point(center, radius, start) });
            figure.Segments.Add(new ArcSegment { Point = Point(center, radius, end), Size = new(radius, radius), SweepDirection = SweepDirection.Clockwise, IsLargeArc = sweep > Math.PI });
            var geometry = new PathGeometry(); geometry.Figures.Add(figure);
            var path = new Microsoft.UI.Xaml.Shapes.Path { Data = geometry, Fill = new SolidColorBrush(ColorHelper.FromArgb(255, (byte)(70 + i * 47 % 150), (byte)(100 + i * 71 % 130), (byte)(130 + i * 31 % 110))) };
            var row = chart[i];
            path.Tapped += (_, _) => { if (row.IsDirectory) _ = NavigateToAsync(row.Path); };
            ChartCanvas.Children.Add(path);
            if (sweep > 0.24)
            {
                var label = new TextBlock { Text = row.Size, Foreground = new SolidColorBrush(Colors.White), FontSize = 12 };
                var at = Point(center, radius * .58, start + sweep / 2);
                Canvas.SetLeft(label, at.X - 22); Canvas.SetTop(label, at.Y - 8); ChartCanvas.Children.Add(label);
            }
            start = end;
        }
    }

    private static global::Windows.Foundation.Point Point(global::Windows.Foundation.Point center, double radius, double angle) => new(center.X + Math.Cos(angle) * radius, center.Y + Math.Sin(angle) * radius);

    private void PlaceChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItemContainer?.Tag is not string tag) return;
        if (tag == "Global")
        {
            GlobalSearch.IsChecked = true;
            Refresh(true);
            return;
        }

        GlobalSearch.IsChecked = false;
        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        var path = tag == "Home" ? home : System.IO.Path.Combine(home, tag);
        _ = NavigateToAsync(path);
    }

    private async void BackClicked(object sender, RoutedEventArgs e)
    {
        if (await ApplyActionAsync(current => current.BackResult(), "There is no previous location."))
        {
            GlobalSearch.IsChecked = false;
            Refresh(true);
        }
    }

    private async void ForwardClicked(object sender, RoutedEventArgs e)
    {
        if (await ApplyActionAsync(current => current.ForwardResult(), "There is no next location."))
        {
            GlobalSearch.IsChecked = false;
            Refresh();
        }
    }

    private async void UpClicked(object sender, RoutedEventArgs e)
    {
        var current = manager;
        if (current is null) return;
        try
        {
            var path = await RunNativeAsync(() => current.Directory, lifetime.Token);
            var parent = System.IO.Directory.GetParent(path)?.FullName;
            if (parent is not null) await NavigateToAsync(parent);
        }
        catch (OperationCanceledException) { }
        catch (Exception ex) { ShowError(ex.Message, "Megaman could not read the current location."); }
    }

    private void AddressKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != VirtualKey.Enter) return;
        e.Handled = true;
        _ = NavigateToAsync(AddressBar.Text.Trim());
    }

    private async Task<bool> NavigateToAsync(string path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            ShowError("Enter a folder path.");
            return false;
        }
        if (!await ApplyActionAsync(current => current.NavigateResult(path), $"Megaman could not open {path}.")) return false;
        GlobalSearch.IsChecked = false;
        Search.Text = "";
        Refresh(true);
        return true;
    }

    private async Task<bool> ApplyActionAsync(Func<NativeManager, NativeActionResult> action, string fallback)
    {
        var current = manager;
        if (current is null || lifetime.IsCancellationRequested) return false;
        try
        {
            var result = await RunNativeAsync(() => action(current), lifetime.Token);
            if (!result.Succeeded) ShowError(result.Error, fallback);
            return result.Succeeded;
        }
        catch (OperationCanceledException) { return false; }
        catch (Exception ex) { ShowError(ex.Message, fallback); return false; }
    }

    private void CloseComponentClicked(object sender, RoutedEventArgs e) => CloseComponentPanel();
    private async void BatchRenameClicked(object sender, RoutedEventArgs e) => await BatchRenameAsync(true);
    private void RefreshClicked(object sender, RoutedEventArgs e) => Refresh(true);
    private void SearchSubmitted(AutoSuggestBox sender, AutoSuggestBoxQuerySubmittedEventArgs args) => Refresh();
    private void SearchChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args)
    {
        if (args.Reason == AutoSuggestionBoxTextChangeReason.UserInput) Refresh();
    }

    private void GlobalSearchClicked(object sender, RoutedEventArgs e) => Refresh();

    private void GlobalSearchAccelerator(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        GlobalSearch.IsChecked = true;
        Search.Focus(FocusState.Keyboard);
        args.Handled = true;
        Refresh();
    }

    private void SortChanged(object sender, SelectionChangedEventArgs e)
    {
        Refresh();
    }

    private void ChartClicked(object sender, RoutedEventArgs e)
    {
        var visible = ChartButton.IsChecked == true;
        ChartCanvas.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
        PreviewPanel.Visibility = visible ? Visibility.Collapsed : Visibility.Visible;
        if (visible) RefreshChart();
    }

    private void ChartSizeChanged(object sender, SizeChangedEventArgs e) { if (ChartButton.IsChecked == true) DrawChart(); }

    private void ViewClicked(object sender, RoutedEventArgs e)
    {
        var list = ListMode.IsChecked == true;
        Files.Visibility = list ? Visibility.Collapsed : Visibility.Visible;
        FileList.Visibility = list ? Visibility.Visible : Visibility.Collapsed;
    }

    private async void FileClicked(object sender, ItemClickEventArgs e)
    {
        var row = AsFileItem(e.ClickedItem);
        if (row is null) return;
        PreviewName.Text = row.Name;
        PreviewMeta.Text = row.IsDirectory ? $"Folder\n{row.Path}" : $"{row.Size}\n{row.Path}";
        PreviewText.Text = row.IsDirectory ? "Folder" : "";
        PreviewImage.Source = row.Thumbnail;
        if (!row.IsDirectory && row.Thumbnail is null) await PreviewTextFile(row.Path);
    }

    private void FileRightTapped(object sender, RightTappedRoutedEventArgs e)
    {
        if (sender is not FrameworkElement element || AsFileItem(element.DataContext) is not { } row) return;
        var selected = ListMode.IsChecked == true ? FileList.SelectedItems : Files.SelectedItems;
        selected.Clear();
        if (ListMode.IsChecked == true)
        {
            if (ListRowFor(row) is { } listRow) selected.Add(listRow);
        }
        else selected.Add(row);
    }

    private void FileDragItemsStarting(object sender, DragItemsStartingEventArgs e)
    {
        var paths = e.Items.Select(AsFileItem).OfType<FileItem>().Select(row => row.Path).Where(path => !string.IsNullOrWhiteSpace(path)).Distinct(StringComparer.OrdinalIgnoreCase).ToArray();
        if (paths.Length == 0)
        {
            e.Cancel = true;
            return;
        }
        e.Data.SetText(string.Join(Environment.NewLine, paths));
        e.Data.RequestedOperation = DataPackageOperation.Copy | DataPackageOperation.Move;
    }

    private void FileDragOver(object sender, DragEventArgs e)
    {
        if (!e.DataView.Contains(StandardDataFormats.StorageItems) && !e.DataView.Contains(StandardDataFormats.Text)) return;
        e.AcceptedOperation = e.Modifiers.HasFlag(DragDropModifiers.Shift) ? DataPackageOperation.Move : DataPackageOperation.Copy;
        e.Handled = true;
    }

    private async void FileDrop(object sender, DragEventArgs e)
    {
        if (!e.DataView.Contains(StandardDataFormats.StorageItems) && !e.DataView.Contains(StandardDataFormats.Text)) return;
        e.Handled = true;
        var deferral = e.GetDeferral();
        try
        {
            var destination = DropDirectory(e.OriginalSource as DependencyObject) ?? await CurrentDirectoryAsync();
            var paths = await DropPathsAsync(e.DataView);
            if (string.IsNullOrWhiteSpace(destination) || paths.Count == 0) return;
            var move = e.Modifiers.HasFlag(DragDropModifiers.Shift);
            await CopyDroppedPathsAsync(paths, destination, move);
            ShowSuccess(move ? "Items moved." : "Items copied.");
            Refresh(true);
        }
        catch (OperationCanceledException) { }
        catch (Exception ex) { ShowError(ex.Message, "Megaman could not complete the drop."); }
        finally { deferral.Complete(); }
    }

    private static string? DropDirectory(DependencyObject? source)
    {
        for (var current = source; current is not null; current = VisualTreeHelper.GetParent(current))
            if (current is FrameworkElement element && AsFileItem(element.DataContext) is { IsDirectory: true } row) return row.Path;
        return null;
    }

    private static async Task<IReadOnlyList<string>> DropPathsAsync(DataPackageView data)
    {
        if (data.Contains(StandardDataFormats.StorageItems))
            return (await data.GetStorageItemsAsync()).Select(item => item.Path).Where(path => !string.IsNullOrWhiteSpace(path)).ToArray();
        var text = await data.GetTextAsync();
        return text.Split(['\r', '\n'], StringSplitOptions.RemoveEmptyEntries).Select(value =>
        {
            if (Uri.TryCreate(value, UriKind.Absolute, out var uri) && uri.IsFile) return uri.LocalPath;
            return value.Trim();
        }).Where(path => !string.IsNullOrWhiteSpace(path)).ToArray();
    }

    private async Task CopyDroppedPathsAsync(IReadOnlyList<string> paths, string destinationPath, bool move)
    {
        var destination = await StorageFolder.GetFolderFromPathAsync(destinationPath);
        foreach (var path in paths)
        {
            try
            {
                var folder = await StorageFolder.GetFolderFromPathAsync(path);
                if (IsSameOrDescendant(destination.Path, folder.Path)) throw new IOException("A folder cannot be dropped into itself or one of its children.");
                if (move)
                {
                    await EnsureNoReparsePathAsync(folder.Path);
                    await EnsureNoReparsePathAsync(destination.Path);
                    var target = System.IO.Path.Combine(destination.Path, folder.Name);
                    await RunNativeAsync(() => System.IO.Directory.Move(folder.Path, target), lifetime.Token);
                }
                else await CopyFolderAsync(folder, destination);
            }
            catch (FileNotFoundException)
            {
                var file = await StorageFile.GetFileFromPathAsync(path);
                if (string.Equals(System.IO.Path.GetDirectoryName(file.Path)?.TrimEnd('\\', '/'), destination.Path.TrimEnd('\\', '/'), StringComparison.OrdinalIgnoreCase))
                    throw new IOException("The destination is already the source folder.");
                if (move) await file.MoveAsync(destination, file.Name, NameCollisionOption.GenerateUniqueName);
                else await file.CopyAsync(destination, file.Name, NameCollisionOption.GenerateUniqueName);
            }
        }
    }

    private IReadOnlyList<FileItem> CurrentRows() => fileRows;

    private async void SelectionClicked(object sender, RoutedEventArgs e)
    {
        var rows = CurrentRows();
        if (rows.Count == 0)
        {
            ShowError("There are no loaded results to select.");
            return;
        }
        var name = new TextBox { PlaceholderText = "Name contains (case insensitive)" };
        var extensions = new TextBox { PlaceholderText = "Extensions, separated by commas (for example rs, toml)" };
        var kind = new ComboBox { Header = "Type", HorizontalAlignment = HorizontalAlignment.Stretch };
        foreach (var item in new[] { "Files and folders", "Files only", "Folders only", "Archives", "Images", "Audio", "Video" }) kind.Items.Add(item);
        kind.SelectedIndex = 0;
        var summary = new TextBlock { Opacity = 0.7, TextWrapping = TextWrapping.Wrap };
        var content = new StackPanel { Spacing = 8, Children = { name, extensions, kind, summary } };
        var dialog = new ContentDialog
        {
            Title = "Select matching items",
            Content = content,
            PrimaryButtonText = "Replace selection",
            CloseButtonText = "Cancel",
            XamlRoot = Shell.XamlRoot,
        };
        bool Matches(FileItem row)
        {
            var needle = name.Text.Trim().ToLowerInvariant();
            var ext = extensions.Text.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).Select(value => value.TrimStart('.').ToLowerInvariant()).Where(value => value.Length > 0).ToHashSet();
            var fileName = row.Name.ToLowerInvariant();
            var extension = System.IO.Path.GetExtension(fileName).TrimStart('.');
            var typeMatches = kind.SelectedIndex switch
            {
                1 => !row.IsDirectory,
                2 => row.IsDirectory,
                3 => !row.IsDirectory && extension is ("zip" or "7z" or "rar" or "tar" or "gz" or "bz2" or "xz"),
                4 => !row.IsDirectory && extension is ("png" or "jpg" or "jpeg" or "gif" or "webp" or "bmp" or "svg"),
                5 => !row.IsDirectory && extension is ("mp3" or "wav" or "flac" or "ogg" or "m4a" or "aac"),
                6 => !row.IsDirectory && extension is ("mp4" or "mkv" or "mov" or "webm" or "avi" or "wmv"),
                _ => true,
            };
            return typeMatches && fileName.Contains(needle, StringComparison.Ordinal) && (ext.Count == 0 || (!row.IsDirectory && ext.Contains(extension)));
        }
        void Evaluate(bool apply)
        {
            var matches = rows.Where(Matches).ToArray();
            summary.Text = $"{matches.Length} matching item{(matches.Length == 1 ? "" : "s")} in the loaded results.";
            if (!apply) return;
            var selected = ListMode.IsChecked == true ? FileList.SelectedItems : Files.SelectedItems;
            selected.Clear();
            foreach (var row in matches)
            {
                if (ListMode.IsChecked == true)
                {
                    if (ListRowFor(row) is { } listRow) selected.Add(listRow);
                }
                else selected.Add(row);
            }
        }
        name.TextChanged += (_, _) => Evaluate(false);
        extensions.TextChanged += (_, _) => Evaluate(false);
        kind.SelectionChanged += (_, _) => Evaluate(false);
        Evaluate(false);
        if (await dialog.ShowAsync() == ContentDialogResult.Primary) Evaluate(true);
    }

    private async void FileDoubleTapped(object sender, DoubleTappedRoutedEventArgs e)
    {
        var selected = ListMode.IsChecked == true ? FileList.SelectedItem : Files.SelectedItem;
        if (AsFileItem(selected) is { } row) await OpenItemAsync(row);
    }

    private async void ContextOpenClicked(object sender, RoutedEventArgs e)
    {
        if (AsFileItem((sender as MenuFlyoutItem)?.Tag) is { } row) await OpenItemAsync(row);
    }

    private async Task OpenItemAsync(FileItem row)
    {
        if (row.IsDirectory)
        {
            await NavigateToAsync(row.Path);
            return;
        }
        try
        {
            if (!await Launcher.LaunchFileAsync(await StorageFile.GetFileFromPathAsync(row.Path)))
                ShowError($"Windows could not open {row.Name}.");
        }
        catch (Exception ex) { ShowError(ex.Message, "Windows could not open this file."); }
    }

    private IReadOnlyList<FileItem> SelectedRows()
    {
        var selected = ListMode.IsChecked == true ? FileList.SelectedItems : Files.SelectedItems;
        return selected.Select(AsFileItem).OfType<FileItem>().ToArray();
    }

    private async void NewFolderClicked(object sender, RoutedEventArgs e)
    {
        var current = manager;
        if (current is null) return;
        try
        {
            var directory = await RunNativeAsync(() => current.Directory, lifetime.Token);
            var folder = await StorageFolder.GetFolderFromPathAsync(directory);
            await folder.CreateFolderAsync("New folder", CreationCollisionOption.GenerateUniqueName);
            Refresh(true);
        }
        catch (OperationCanceledException) { }
        catch (Exception ex) { ShowError(ex.Message, "Windows could not create the folder."); }
    }

    private async void PasteClicked(object sender, RoutedEventArgs e)
    {
        var current = manager;
        if (current is null) return;
        try
        {
            var view = Clipboard.GetContent();
            if (!view.Contains(StandardDataFormats.StorageItems))
            {
                ShowError("The clipboard does not contain files or folders.");
                return;
            }
            var directory = await RunNativeAsync(() => current.Directory, lifetime.Token);
            var destination = await StorageFolder.GetFolderFromPathAsync(directory);
            foreach (var item in await view.GetStorageItemsAsync())
            {
                switch (item)
                {
                    case StorageFile file:
                        await file.CopyAsync(destination, file.Name, NameCollisionOption.GenerateUniqueName);
                        break;
                    case StorageFolder folder:
                        await CopyFolderAsync(folder, destination);
                        break;
                }
            }
            Refresh(true);
        }
        catch (OperationCanceledException) { }
        catch (Exception ex) { ShowError($"{ex.Message} Items copied before the error remain in the destination.", "Windows could not paste every item."); }
    }

    private static async Task CopyFolderAsync(StorageFolder source, StorageFolder destination)
    {
        if (IsSameOrDescendant(destination.Path, source.Path))
            throw new IOException("A folder cannot be pasted into itself or one of its children.");
        await EnsureNoReparsePathAsync(source.Path);
        await EnsureNoReparsePathAsync(destination.Path);
        if (await IsReparsePointAsync(source.Path))
            throw new IOException($"The source folder '{source.Name}' is a reparse point and cannot be copied safely.");

        var target = await destination.CreateFolderAsync(source.Name, CreationCollisionOption.GenerateUniqueName);
        foreach (var file in await source.GetFilesAsync())
            await file.CopyAsync(target, file.Name, NameCollisionOption.GenerateUniqueName);
        foreach (var folder in await source.GetFoldersAsync())
        {
            if (await IsReparsePointAsync(folder.Path))
                throw new IOException($"The source folder '{folder.Name}' is a reparse point and cannot be copied safely.");
            await CopyFolderAsync(folder, target);
        }
    }

    private static Task<bool> IsReparsePointAsync(string path) => Task.Run(() => (System.IO.File.GetAttributes(path) & System.IO.FileAttributes.ReparsePoint) != 0);

    private static Task EnsureNoReparsePathAsync(string path) => Task.Run(() =>
    {
        for (var current = new DirectoryInfo(path); current is not null; current = current.Parent)
            if ((current.Attributes & System.IO.FileAttributes.ReparsePoint) != 0)
                throw new IOException($"The copy path passes through the reparse point '{current.Name}'.");
    });

    private static bool IsSameOrDescendant(string candidate, string root)
    {
        var candidatePath = System.IO.Path.GetFullPath(candidate).TrimEnd('\\', '/');
        var rootPath = System.IO.Path.GetFullPath(root).TrimEnd('\\', '/');
        return candidatePath.Equals(rootPath, StringComparison.OrdinalIgnoreCase) || candidatePath.StartsWith(rootPath + System.IO.Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase) || candidatePath.StartsWith(rootPath + System.IO.Path.AltDirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
    }

    private async void CopyPathClicked(object sender, RoutedEventArgs e)
    {
        var current = manager;
        if (current is null) return;
        var rows = SelectedRows();
        try
        {
            var paths = rows.Count == 0
                ? [await RunNativeAsync(() => current.Directory, lifetime.Token)]
                : rows.Select(row => row.Path).ToArray();
            var package = new DataPackage();
            package.SetText(string.Join(Environment.NewLine, paths));
            Clipboard.SetContent(package);
            StatusBar.Severity = InfoBarSeverity.Success;
            StatusBar.Message = paths.Length == 1 ? "Path copied." : $"{paths.Length} paths copied.";
            StatusBar.IsOpen = true;
        }
        catch (OperationCanceledException) { }
        catch (Exception ex) { ShowError(ex.Message, "Windows could not access the clipboard."); }
    }

    private async void RenameClicked(object sender, RoutedEventArgs e)
    {
        var rows = SelectedRows();
        if (rows.Count != 1)
        {
            ShowError("Select one file or folder to rename.");
            return;
        }

        var input = new TextBox { Text = rows[0].Name };
        input.SelectAll();
        var dialog = new ContentDialog
        {
            Title = "Rename",
            Content = input,
            PrimaryButtonText = "Rename",
            CloseButtonText = "Cancel",
            XamlRoot = Shell.XamlRoot,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;
        var name = input.Text.Trim();
        if (string.IsNullOrWhiteSpace(name) || name is "." or ".." || name.IndexOfAny(System.IO.Path.GetInvalidFileNameChars()) >= 0)
        {
            ShowError("Choose a valid Windows file name.");
            return;
        }

        try
        {
            if (rows[0].IsDirectory)
                await (await StorageFolder.GetFolderFromPathAsync(rows[0].Path)).RenameAsync(name, NameCollisionOption.FailIfExists);
            else
                await (await StorageFile.GetFileFromPathAsync(rows[0].Path)).RenameAsync(name, NameCollisionOption.FailIfExists);
            Refresh(true);
        }
        catch (Exception ex) { ShowError(ex.Message, "Windows could not rename this item."); }
    }

    private async void DeleteClicked(object sender, RoutedEventArgs e)
    {
        var rows = SelectedRows();
        if (rows.Count == 0)
        {
            ShowError("Select one or more files or folders to recycle.");
            return;
        }

        var dialog = new ContentDialog
        {
            Title = "Move to Recycle Bin",
            Content = rows.Count == 1 ? $"Move {rows[0].Name} to the Recycle Bin?" : $"Move {rows.Count} items to the Recycle Bin?",
            PrimaryButtonText = "Recycle",
            CloseButtonText = "Cancel",
            XamlRoot = Shell.XamlRoot,
        };
        if (await dialog.ShowAsync() != ContentDialogResult.Primary) return;

        try
        {
            foreach (var row in rows)
            {
                if (row.IsDirectory)
                    await (await StorageFolder.GetFolderFromPathAsync(row.Path)).DeleteAsync(StorageDeleteOption.Default);
                else
                    await (await StorageFile.GetFileFromPathAsync(row.Path)).DeleteAsync(StorageDeleteOption.Default);
            }
            Refresh(true);
        }
        catch (Exception ex) { ShowError(ex.Message, "Windows could not recycle every selected item."); Refresh(true); }
    }

    private async void OpenExplorerClicked(object sender, RoutedEventArgs e)
    {
        var current = manager;
        if (current is null) return;
        try
        {
            var directory = await RunNativeAsync(() => current.Directory, lifetime.Token);
            if (!await Launcher.LaunchFolderPathAsync(directory)) ShowError("Windows could not open this folder in File Explorer.");
        }
        catch (OperationCanceledException) { }
        catch (Exception ex) { ShowError(ex.Message, "Windows could not open File Explorer."); }
    }

    private async Task PreviewTextFile(string path)
    {
        try
        {
            var file = await StorageFile.GetFileFromPathAsync(path);
            using var stream = await file.OpenStreamForReadAsync();
            using var reader = new StreamReader(stream, Encoding.UTF8, true);
            var buffer = new char[64 * 1024];
            PreviewText.Text = new(buffer, 0, await reader.ReadBlockAsync(buffer, 0, buffer.Length));
        }
        catch { }
    }

    private void DensityChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (Files?.ItemsPanelRoot is ItemsWrapGrid wrap) { wrap.ItemWidth = e.NewValue; wrap.ItemHeight = e.NewValue; }
    }

    private void ShellPointerWheelChanged(object sender, PointerRoutedEventArgs e)
    {
        if (!e.KeyModifiers.HasFlag(VirtualKeyModifiers.Control)) return;
        var delta = e.GetCurrentPoint(Shell).Properties.MouseWheelDelta;
        if (delta == 0) return;
        Density.Value = Math.Clamp(Density.Value + Math.Sign(delta) * 8, Density.Minimum, Density.Maximum);
        e.Handled = true;
    }

    private void PreviewDividerDragged(object sender, DragDeltaEventArgs e) => RightColumn.Width = new(Math.Clamp(RightColumn.ActualWidth - e.HorizontalChange, 240, 800));

    private void ShowError(string fallback) => ShowError("", fallback);

    private void ShowError(string message, string fallback)
    {
        StatusBar.Severity = InfoBarSeverity.Error;
        StatusBar.Message = string.IsNullOrWhiteSpace(message) ? fallback : message;
        StatusBar.IsOpen = true;
    }

    private void ShowSuccess(string message)
    {
        StatusBar.Severity = InfoBarSeverity.Success;
        StatusBar.Message = message;
        StatusBar.IsOpen = true;
    }

    private static string SizeText(ulong bytes) => SizeTextValue(bytes);

    private static string SizeTextValue(ulong bytes)
    {
        var value = (double)bytes;
        foreach (var unit in new[] { "B", "KB", "MB", "GB", "TB" })
        {
            if (value < 1024 || unit == "TB") return $"{value:0.#} {unit}";
            value /= 1024;
        }
        return $"{bytes:n0} B";
    }

    private static string FolderName(string path)
    {
        if (string.IsNullOrWhiteSpace(path)) return "Megaman";
        var trimmed = path.TrimEnd(System.IO.Path.DirectorySeparatorChar, System.IO.Path.AltDirectorySeparatorChar);
        return string.IsNullOrWhiteSpace(trimmed) ? path : new DirectoryInfo(trimmed).Name;
    }
}
