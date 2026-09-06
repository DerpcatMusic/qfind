<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/megaman-logo-dark.svg">
  <img src="assets/megaman-logo.svg" alt="Megaman — Bit, the voxel file keeper" width="640">
</picture>

Files, storage, and projects. One native workspace for Linux, macOS, and Windows.

Meet **Bit**, Megaman’s little voxel file keeper.

[![Release](https://img.shields.io/github/v/release/DerpcatMusic/qfind?style=flat-square)](https://github.com/DerpcatMusic/qfind/releases/latest)
[![License](https://img.shields.io/github/license/DerpcatMusic/qfind?style=flat-square)](LICENSE)

[Features](#what-it-does) · [Install](#install) · [Controls](#tui-controls) · [Preview formats](#preview-support) · [Config](#config)

</div>

<p align="center">
  <img src="docs/images/qfind-grid-preview.png" alt="Qfind grid search with image thumbnails and a large side Preview" width="920">
</p>

Qfind builds one memory-mapped Catalog from your local disks. Queries search filenames without walking the filesystem again. Linux and macOS use the parallel Unix directory path; Windows walks all available drive letters in parallel. Metadata is loaded only when a Sort or Preview needs it.

## What it does

- Fuzzy, substring, exact, glob, extension, FileClass, date, and folder searches
- List and grid surfaces with adjustable density
- Text, Markdown, image, SVG, PDF, video, audio, office, font, and extended-image Previews
- Dual-pane folder browser: folders on the left, contents on the right
- Interactive WeightMap by byte size or file count per extension
- Instant toggles for hidden files, `.gitignore`, global Git excludes, `.git/info/exclude`, and `.ignore`
- Configurable themes, zebra rows, Preview width, WeightMap visibility, editor, and open behavior
- Mouse selection, scrolling, resizable panes, context menus, and native file Drag in Kitty 0.47+
- GTK, Nautilus, Vicinae, and optional Qt/Breeze frontends
- Native SwiftUI/AppKit frontend on macOS and WinUI 3 frontend on Windows

Qfind has no startup splash. Config changes made in the TUI are saved and applied immediately.

## Install

### Linux and macOS

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://raw.githubusercontent.com/DerpcatMusic/qfind/main/install.sh | sh
```

This installs `qfind` and `qfind-tui` into `~/.local/bin`. Override it with `QFIND_INSTALL_DIR=/your/bin`, or install a particular release with `QFIND_VERSION=v0.3.0`.

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/DerpcatMusic/qfind/main/install.ps1 | iex
```

This installs both executables in `%LOCALAPPDATA%\Programs\qfind` and adds that directory to your user `PATH`. Set `$env:QFIND_INSTALL_DIR` or `$env:QFIND_VERSION` first to override those defaults.

After installation:

```bash
qfind index
qfind
```

### Manual Linux x86_64 release

```bash
curl -L https://github.com/DerpcatMusic/qfind/releases/download/v0.2.0/qfind-0.2.0-x86_64.tar.zst \
  | tar -C /tmp -x --zstd
sudo install -Dm755 /tmp/qfind /tmp/qfind-tui -t /usr/local/bin
# Optional desktop frontend:
sudo install -Dm755 /tmp/qfind-gtk -t /usr/local/bin
sudo install -Dm644 /tmp/qfind.desktop /usr/local/share/applications/qfind.desktop
```

The archive also contains `qfind-gtk`, the desktop file, Nautilus integration, and `qfind-qt` when Qt6 was available on the release builder.

### Linux

Requires Rust 1.88 or newer.

```bash
git clone https://github.com/DerpcatMusic/qfind.git
cd qfind
./packaging/install.sh
```

This installs `qfind` and `qfind-tui` in `~/.local/bin`. If GTK4 is available through `pkg-config`, it also installs `qfind-gtk`, the desktop launcher, and the Nautilus plugin. If Qt6 is available, it builds `qfind-qt` with Breeze styling.

For CLI and TUI only:

```bash
cargo build --release
```

Build the GTK frontend separately with `cargo build --release -p qfind-gtk`.

### macOS

Requires Rust 1.88 or newer. No GTK dependencies are needed for the TUI.

```bash
git clone https://github.com/DerpcatMusic/qfind.git
cd qfind
cargo build --release -p qfind -p qfind-tui
sudo install -m755 target/release/qfind target/release/qfind-tui /usr/local/bin/
```

Qfind indexes `/`, including mounted disks under `/Volumes`. Enter opens the macOS default app, `Space` can hand the selection to Quick Look, `Ctrl+O` reveals it in Finder, and `Ctrl+Y` uses `pbcopy`. Build a native Intel or Apple Silicon archive with `./packaging/release-tui.sh`.

### Windows

Install Rust with rustup, then build from PowerShell:

```powershell
git clone https://github.com/DerpcatMusic/qfind.git
cd qfind
cargo build --release -p qfind -p qfind-tui
.\target\release\qfind.exe index
.\target\release\qfind.exe
```

Qfind indexes available drive letters and skips Windows system stores, the Recycle Bin, and System Volume Information. Enter uses the registered Windows app, `Ctrl+O` reveals the file in Explorer, and `Ctrl+Y` uses the Windows clipboard. Create a ZIP with `.\packaging\release-windows.ps1`.

### Arch Linux

The repository includes source and binary PKGBUILDs:

```bash
cd packaging/aur/qfind && makepkg -si          # CLI/TUI from source
cd packaging/aur/qfind-bin && makepkg -si      # CLI/TUI prebuilt
cd packaging/aur/qfind-gtk && makepkg -si      # GTK from source
cd packaging/aur/qfind-gtk-bin && makepkg -si  # GTK prebuilt
```

## First run

```bash
qfind index    # rebuild the Catalog
qfind tui      # open the TUI
```

With arguments, `qfind` works as a regular CLI:

```bash
qfind kick wav                         # filenames containing both words
qfind .wav --files --sort largest      # extension search
qfind cat --folders                    # folders only
qfind logo --class image --json        # JSON lines for scripts
```

The terminal commands use the same file operations and workspace components as the GUI:

```bash
qfind files --help                         # browse, batch operations, and archives
qfind files list ~/Downloads --extension zip --paths-only --nul
qfind files batch-rename a.txt b.txt --prefix 'item-{n}-'  # preview; --apply commits
qfind files batch-copy ~/Backup a.txt b.txt
qfind files archive compress backup.zip a.txt b.txt
qfind places list                         # folders, bookmarks, and mounts
qfind places pin ~/Projects
qfind places actions list                 # Qfind and Nautilus scripts
qfind --in ~/Projects --json '*.rs'         # scoped indexed search
qfind projects                            # repositories for your active gh account
qfind git diff --path ~/Projects/megaman
qfind git stage --path ~/Projects/megaman --file README.md
qfind tasks --path ~/Projects/megaman      # list available build/package commands
qfind tasks cargo-check --path ~/Projects/megaman
qfind storage ~/Projects                  # weights plus free/total disk capacity
qfind storage --global                    # all indexed storage roots
qfind component                           # discover shared GUI components
qfind component git '{"action":"status"}' --path ~/Projects/megaman
qfind config --path                       # shared preferences file
qfind config --edit                       # edit shared preferences
```

`projects`, `storage`, and `component` print JSON. Git and task output is plain text by default; use `--json` for structured output. Project discovery requires an index; file operations, Git, and tasks work without one. Failed project commands return a nonzero exit status. Storage folder weights use the same cached/indexed measurements as the GUI. File-list JSON reports an unmeasured folder size as `null`, rather than its directory metadata size. `copy-path`, `copy-name`, and `copy-uri` print values for piping to a clipboard tool.

An empty Query in the TUI lists the Catalog. Fuzzy matching is the default; use `--match substring`, `--match exact`, or `Ctrl+M` in the TUI to change it. `*.wav` is a glob, while `.wav` filters by extension.

## TUI controls

| Input | Action |
|---|---|
| `Tab` | switch Search and Hits focus |
| arrows / wheel | move through Hits; the Preview follows |
| `Space`, `F3` | Preview the focused Hit |
| `Enter` / double-click | open a file or browse a folder |
| `F4` | dual-pane folder browser |
| `F6` | cycle WeightMap size / file types / off |
| `F8` | appearance, visibility, ignore rules, and folder Excludes |
| `F9` / `Ctrl+P` | Projects, Storage, Git, and Tasks workspaces |
| `Ctrl+G` | global indexed search from the browser or a workspace |
| `F10` | file actions, batch rename, copy/move, archives, and scripts |
| `Ctrl+B` / `Ctrl+Shift+B` | pin current folder / browse pins |
| `F1` | shortcuts |
| `Ctrl+E` | cycle Auto / desktop / editor opening |
| `Ctrl+scroll`, `+`, `-` | change list or grid density |
| `Ctrl+O` | show the Hit in Files |
| `Ctrl+Y` | copy the path |
| `Ctrl+F` / `Ctrl+Shift+F` | search the browsed folder / search everywhere |
| `Alt+Left` / `Alt+Right` | browser Back / Forward |
| `Ctrl+L` | enter an absolute or relative directory path |
| right-click | Open, Preview, copy, and file-manager actions |
| mouse drag | hand a Hit to another desktop app in Kitty 0.47+ |

In the workspace, keys `1`–`4` switch Projects, Storage, Git, and Tasks. Projects supports `/` filtering, `s` sorting, and `c` cleanup review. Git supports `s`/`u` staging, `Tab` for staged changes, `v` for split/unified diff, `w` wrapping, `e` an expanded pane, and `[`/`]` hunk navigation. Task output stays available to scroll after completion.

Drag uses Kitty's native OSC 72 protocol on Linux and macOS. Visual files reuse the current Preview as the cursor image; other files use a compact icon-and-name card. Windows terminals do not currently expose a compatible native file-drag protocol.

The Preview pane can be resized with its divider. Scrolling over it scrolls wrapped Preview content; scrolling over Hits moves the selection and updates the Preview. Grid density can be reduced until tiles become compact text cards or increased for larger visual thumbnails.

## Preview support

Raster images and text work without helper programs. Qfind uses installed desktop tools for other visual formats:

| Content | Helper |
|---|---|
| SVG | `rsvg-convert` |
| PDF, PostScript, comics, DjVu | `evince-thumbnailer` |
| video | `ffmpegthumbnailer` |
| audio waveform and extended images | `ffmpeg` |
| office documents | `gsf-office-thumbnailer` |
| fonts | ImageMagick `magick` |

Preview work runs away from input handling, stale jobs are discarded, and external helpers have a two-second deadline.
Helpers are optional and discovered on `PATH`; `Space` falls back to Quick Look on macOS or the registered desktop app on Windows.

## Folder browser and WeightMap

Press `F4` to open the browser. Use `Tab`, `Left`, or `Right` to switch panes, `Enter` to descend, `Backspace` to move to the parent folder, and `Alt+Left` / `Alt+Right` for history. `Ctrl+L` opens a direct location entry. Entering a folder updates the WeightMap to that folder.

Typing from Browse or pressing `Ctrl+F` starts an instant recursive Query scoped to the displayed folder. `Ctrl+Shift+F` widens the Query back to the whole Catalog.

The WeightMap follows the current Query or browsed folder. Size mode uses file bytes; File Types mode counts files by extension. Click a folder tile to browse it or an extension tile to search it.

## Config

Press `F8` to edit appearance and Catalog visibility. Theme previews apply while moving through the list; confirmation is not required. Hidden-file and ignore-rule changes rerun the current Query immediately.

<p align="center">
  <img src="docs/images/qfind-settings.png" alt="Qfind Settings panel" width="640">
</p>

Config and Catalog locations follow each platform:

| Platform | Config | Catalog |
|---|---|---|
| Linux | `$XDG_CONFIG_HOME/qfind/config.toml` or `~/.config/qfind/config.toml` | `$XDG_CACHE_HOME/qfind/catalog` or `~/.cache/qfind/catalog` |
| macOS | `~/Library/Application Support/qfind/config.toml` | `~/Library/Caches/qfind/catalog` |
| Windows | `%APPDATA%\qfind\config.toml` | `%LOCALAPPDATA%\qfind\catalog` |

Example:

```toml
theme = "grok"             # grok | titanium | catppuccin | gruvbox | dracula | nord | aurora
preview_width = 36          # 20..70
zebra = true
weight_map = true
show_hidden = true
respect_gitignore = false
respect_ignore = false
open = "auto"              # auto | desktop (legacy: xdg) | editor
editor = "nvim"            # empty = $EDITOR, then $VISUAL
```

Auto opening sends text files to the configured editor and folders or media to the platform default app. Selected folders can be added to or removed from exact Catalog Excludes from Search or the F4 browser.

## Desktop integrations

### GTK

Run `qfind-gtk` for the desktop frontend. **Classic** shows only the current folder's immediate files and folders; **Qfind** switches to recursive indexed results below that location. The window includes Places and pinned folders on the left, resizable results in the middle, and a right pane that switches between file Preview and an interactive Chart. Chart can follow the current directory or show every indexed local Mount (including NTFS/fuseblk); drilling in either the file surface or Chart keeps the other synchronized. Use the header star to pin or unpin the current folder.

The connected navigation shell keeps Places and the header on one brighter theme surface while files stay on the quieter content surface. The header separates folder navigation, Classic/Qfind scope, location, Search, result filters, View settings, Preview, and app Settings. The funnel holds matching, sort, folders-first grouping, and FileClass filters; the three-dot View menu holds List/Grid, Zoom, spacing, zebra rows, Tree, WeightMap, Preview, hidden files, and ignore rules. `F3` closes or restores the preview pane, `Ctrl+L` selects the location, `Ctrl+F` searches below it, `Ctrl+Shift+F` searches everywhere, `Alt+Left` / `Alt+Right` move through session history, and `Alt+Up` opens the parent. Typing updates Hits immediately, and rows can be dragged into other Wayland or X11 applications.

Places are read directly from Qfind pins, GTK/Nautilus bookmarks, and Dolphin's `user-places.xbel`. The context menu provides the built-in file actions and discovers nested executable actions from `$XDG_DATA_HOME/qfind/actions` plus existing Nautilus Scripts from `$XDG_DATA_HOME/nautilus/scripts`; subdirectories become submenus and commands receive the selected path as both an argument and `QFIND_SELECTED_PATHS`.

Space previews the hovered or selected Hit through GNOME Sushi when available, with a built-in window as fallback. Ctrl+scroll moves between compact lists, roomier rows, and a visual grid. The GTK settings include Excludes, Mounts, default Zoom, spacing, and reset. The virtual ListView/GridView does not `stat` during bind, keeps its scrollbars visible, and uses GSK composition.

### Nautilus and Vicinae

Install Nautilus and Vicinae support without rebuilding Qfind:

```bash
./packaging/install-plugins.sh
```

Nautilus adds **Ctrl+F** search for the current folder and **Search with Qfind** to the context menu. It needs `nautilus-python` on Arch or `python3-nautilus` on Debian-based systems. Vicinae adds a launcher command named **Qfind** and requires Node.js/npm for the TypeScript extension.

See [docs/plugins.md](docs/plugins.md) for manual installation and the script-only Vicinae fallback.

### Native macOS and Windows apps

Both native apps use the same memory-mapped Rust manager as the CLI and TUI. They provide platform Places, Classic/Qfind folder scope, list and grid views, native thumbnails, a resizable Preview, and an interactive directory/global Chart with sizes inside useful segments.

On macOS 14 or newer, build the SwiftUI/AppKit app with Xcode command-line tools installed:

```bash
./packaging/build-macos-app.sh
open target/release/Qfind.app
```

On Windows 10 1809 or newer, build the unpackaged WinUI 3 app with the .NET 8 SDK and MSVC Rust toolchain:

```powershell
dotnet build .\apps\windows\Qfind.Windows.csproj -c Release -p:Platform=x64
.\apps\windows\bin\x64\Release\net8.0-windows10.0.19041.0\Qfind.Windows.exe
```

Live folder browsing works before indexing. Run `qfind index` to enable indexed global search and storage analysis. Native shells call `qfind-native` in-process; GTK is not required on macOS or Windows.

The shared Rust core owns folder queries, filtering, sorting, navigation, and indexed search. GTK calls it directly; SwiftUI/AppKit, WinUI, and Qt adapters use the C ABI in `crates/native/include/qfind_native.h`. Platform controls, file dialogs, clipboard, drag/drop, and native file operations belong to each adapter. Feature parity across frontends must be verified separately.

The shared shell registry lives in `crates/core/src/components.json`. Native component hosts discover its labels and commands through `qfind_manager_component`; GTK uses the same core services and registry titles. Projects, Git, build/package commands, storage, batch operations, and archives have shared behavior with native toolkit views. Add behavior in the core once, then implement any new view type in the native adapters. Existing view types pick up registry command and label changes when the shared library is rebuilt.

Project discovery caches GitHub-account-scoped workspaces for ten minutes, invalidates when the file index changes, and rescans on Refresh. Native archive support enables the core `archives` feature and requires libarchive; Unix uses its writer binding, Windows uses `tar.exe` for writing. CLI and TUI builds also enable archives and require libarchive development files (for example `libarchive-dev` on Debian/Ubuntu, or `brew install libarchive pkg-config` on macOS). Folder measurements also share a bounded background worker and persistent cache, including generated directories excluded from filename search; native views poll the cache revision without rescanning on hover.

## Packages

- `qfind`: portable CLI and TUI for Linux, macOS, and Windows
- `qfind-gtk` / `qfind-gtk-bin`: GTK frontend and desktop integration
- `qfind-qt`: optional Qt6/Breeze frontend built by the installer and release script
- `Qfind.app`: native macOS SwiftUI/AppKit manager
- `Qfind.Windows`: native Windows WinUI 3 manager

## License

[MIT](LICENSE)
