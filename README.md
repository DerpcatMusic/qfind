# Qfind

Filename search for Linux. One Catalog: Rebuild from local disks, Query by name.

Indexing walks `getdents64` in parallel and skips per-file `stat` (names first, Everything-style). The GTK window mmaps `~/.cache/qfind/catalog` and opens before you type.

```bash
git clone https://github.com/DerpcatMusic/qfind.git
cd qfind
./packaging/install.sh
```

That puts `qfind` and `qfind-tui` in `~/.local/bin`. If gtk4 is available (`pkg-config --exists gtk4`), it also builds `qfind-gtk`, the desktop launcher, and the Nautilus plugin. If Qt6 is installed, you also get `qfind-qt` (Breeze).

CLI and TUI only, no gtk4:

```bash
cargo build --release
```

GTK GUI: `cargo build --release -p qfind-gtk`.

Prebuilt x86_64 binaries (GitHub Releases):

```bash
curl -L https://github.com/DerpcatMusic/qfind/releases/download/v0.1.2/qfind-0.1.2-x86_64.tar.zst | tar -C /tmp -x --zstd
sudo install -Dm755 /tmp/qfind /tmp/qfind-tui -t /usr/local/bin
# optional GUI:
sudo install -Dm755 /tmp/qfind-gtk -t /usr/local/bin
sudo install -Dm644 /tmp/qfind.desktop /usr/local/share/applications/qfind.desktop
```

Arch (AUR PKGBUILDs in this tree):

```bash
# CLI/TUI, no gtk4 — source
cd packaging/aur/qfind && makepkg -si
# or prebuilt
cd packaging/aur/qfind-bin && makepkg -si

# GTK GUI (depends on qfind + gtk4)
cd packaging/aur/qfind-gtk && makepkg -si
# or prebuilt
cd packaging/aur/qfind-gtk-bin && makepkg -si
```

`qfind` / `qfind-bin` are CLI and TUI. `qfind-gtk` / `qfind-gtk-bin` add the GUI. To publish to the AUR: add an SSH key at https://aur.archlinux.org/account/ then `./packaging/aur/publish.sh qfind`.

```bash
qfind-gtk                      # GUI
qfind                          # TUI
qfind index                    # rebuild Catalog
qfind kick wav                 # print matching paths
qfind --folders --class image cat
```

## Nautilus and Vicinae

Plugins without rebuilding the binaries:

```bash
./packaging/install-plugins.sh
```

| | After install | Use |
|---|---|---|
| Nautilus / Files | `nautilus -q`, open Files | **Ctrl+F** in a folder, or right-click → Search with Qfind |
| Vicinae | reopen Vicinae | command **Qfind** |

Nautilus needs `nautilus-python` (Arch) or `python3-nautilus` (Debian). Vicinae needs Node/`npm`. Full steps: [docs/plugins.md](docs/plugins.md).

## Search

Fuzzy by default (nucleo / fzf). Switch to substring or exact from the GTK search bar, CLI `--match`, or TUI `Ctrl+M`. `*.wav` is a glob; `.wav` with no star filters by extension. An empty query lists files.

## GTK

Type a name. Enter or double-click opens the file; drag a row out. Right-click at the cursor for Open, Open With, Preview, Show in Files, or Copy path.

Hover a Hit and press Space for preview (GNOME Sushi DBus, else a built-in window). Esc or Space closes it. Settings can switch Space to the selected Hit.

Sort: Score, Name, Newest, Oldest, Largest, Smallest. Date and size `stat` the Hits, not the whole Catalog. Newest/Oldest are recency, not day/week buckets.

Ctrl+scroll zooms like File Pilot: tight list, roomy list, then a square grid.

Gear: extra exclude names, include Mounts, default Zoom, spacing, Reset. Stored at `~/.config/qfind/config.toml`.

Tree is experimental. WeightMap at the bottom is a WizTree-style folder heatmap.

The list is a virtual GTK4 ListView/GridView: bind does not `stat`, scrollbars stay visible, and kinetic flick is on. GSK composites on the GPU. GTK stays GTK because Wayland drag needs it.

## TUI

`qfind` (or `qfind-tui`):

| Key | Action |
|---|---|
| `F3` | preview |
| `F4` | tree |
| `F6` | WeightMap |
| `F8` | cycle skin (titanium, grok, catppuccin, gruvbox, dracula, nord, aurora) |
| `Ctrl+E` | cycle open: auto / xdg / editor |
| Ctrl+scroll, `+` / `-` | zoom |
| `Ctrl+O` | show in Files |
| `Ctrl+Y` | copy path |
| Tab, `F2`, `Ctrl+D` | drag (`ripdrag`) |

First launch plays a splash, then an animated setup while the Catalog Rebuilds. `theme` and `splash` live in `~/.config/qfind/config.toml`. `QFIND_NOSPLASH` skips the intro.

Enter uses `$EDITOR` or `$VISUAL` for text (`.rs`, `.toml`, `.md`, …) and `xdg-open` for folders and media. Override in config:

```toml
open = "auto"      # auto | xdg | editor
editor = "nvim"    # empty = $EDITOR, then $VISUAL
```
