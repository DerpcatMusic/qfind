using System.Text;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.UI.Xaml.Shapes;
using Microsoft.UI.Xaml.Controls.Primitives;
using Windows.Storage;
using Windows.Storage.FileProperties;
using Windows.System;

namespace Qfind.Windows;

public sealed partial class MainWindow : Window
{
    private readonly NativeManager manager;
    private IReadOnlyList<FileItem> chart = [];

    public MainWindow()
    {
        InitializeComponent();
        manager = new NativeManager(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile));
        Closed += (_, _) => manager.Dispose();
        Refresh();
    }

    private void Refresh()
    {
        var rows = manager.Rows(Search.Text ?? "", Recursive.IsChecked == true);
        Files.ItemsSource = rows;
        FileList.ItemsSource = rows;
        Location.Text = manager.Directory;
        _ = LoadThumbnails(rows);
        if (ChartButton.IsChecked == true) RefreshChart();
    }

    private async Task LoadThumbnails(IReadOnlyList<FileItem> rows)
    {
        foreach (var row in rows.Take(200))
        {
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
        chart = manager.Chart(GlobalChart.IsChecked == true);
        DrawChart();
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
            path.Tapped += (_, _) => { if (row.IsDirectory && manager.Navigate(row.Path)) Refresh(); };
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
        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        var path = tag == "Home" ? home : System.IO.Path.Combine(home, tag);
        if (manager.Navigate(path)) { Search.Text = ""; Refresh(); }
    }

    private void BackClicked(object sender, RoutedEventArgs e) { if (manager.Back()) Refresh(); }
    private void ForwardClicked(object sender, RoutedEventArgs e) { if (manager.Forward()) Refresh(); }
    private void RefreshClicked(object sender, RoutedEventArgs e) => Refresh();
    private void SearchSubmitted(AutoSuggestBox sender, AutoSuggestBoxQuerySubmittedEventArgs args) => Refresh();
    private void SearchChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args) { if (args.Reason == AutoSuggestionBoxTextChangeReason.UserInput) Refresh(); }
    private void ChartClicked(object sender, RoutedEventArgs e)
    {
        var visible = ChartButton.IsChecked == true;
        ChartCanvas.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
        PreviewImage.Visibility = PreviewText.Visibility = visible ? Visibility.Collapsed : Visibility.Visible;
        if (visible) RefreshChart();
    }
    private void ChartSizeChanged(object sender, SizeChangedEventArgs e) { if (ChartButton.IsChecked == true) DrawChart(); }
    private void GlobalChartClicked(object sender, RoutedEventArgs e) { if (ChartButton.IsChecked == true) RefreshChart(); }
    private void ViewClicked(object sender, RoutedEventArgs e)
    {
        var list = ListMode.IsChecked == true;
        Files.Visibility = list ? Visibility.Collapsed : Visibility.Visible;
        FileList.Visibility = list ? Visibility.Visible : Visibility.Collapsed;
    }

    private async void FileClicked(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is not FileItem row) return;
        PreviewText.Text = $"{row.Name}\n{row.Size}\n{row.Path}";
        PreviewImage.Source = row.Thumbnail;
        if (!row.IsDirectory && row.Thumbnail is null) await PreviewTextFile(row.Path);
    }

    private async void FileDoubleTapped(object sender, DoubleTappedRoutedEventArgs e)
    {
        var selected = ListMode.IsChecked == true ? FileList.SelectedItem : Files.SelectedItem;
        if (selected is not FileItem row) return;
        if (row.IsDirectory) { if (manager.Navigate(row.Path)) { Search.Text = ""; Refresh(); } }
        else await Launcher.LaunchFileAsync(await StorageFile.GetFileFromPathAsync(row.Path));
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

    private void PreviewDividerDragged(object sender, DragDeltaEventArgs e) => RightColumn.Width = new(Math.Clamp(RightColumn.ActualWidth - e.HorizontalChange, 240, 800));
}
