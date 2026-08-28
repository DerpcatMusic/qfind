# Qfind

Instant filename search. One Catalog module: Rebuild from local Mounts, Query by name.

Rebuild uses parallel `getdents64` (no per-file `stat`). Names-first, like Everything.
Open mmaps `~/.cache/qfind/catalog` — the window shows before any search.

```
./packaging/install.sh   # ~/.local/bin + desktop launcher
qfind-gtk                # GTK app — drag files out of the list
qfind                    # TUI
qfind index              # Rebuild (~/.cache/qfind/catalog)
qfind kick wav           # print paths
qfind --folders --class image cat
```

Search is fuzzy (nucleo / fzf scoring). Switch to substring or exact on the search bar
(GTK) / `--match` (CLI) / `Ctrl+M` (TUI). `*.wav` is still a glob. Empty Query
browses files immediately.

GTK: type, double-click / Enter opens, **drag a row**, **right-click** (at the cursor) for
Open / Open With / Preview / Show in Files / Copy path.
**Space** previews (GNOME Sushi, else a built-in window). Sort is Score, Name,
**Newest / Oldest / size** (live `stat` of Hits, like Files — not day/week buckets).
Scroll uses virtual ListView/GridView, cached row GObjects, and **no `stat` on bind** (Zed/GPUI rule: the scroll hot path must not syscall). Permanent scrollbar + kinetic flick. GTK4 GSK already GPU-composites; we are not rewriting to GPUI (Wayland Drag needs GTK).

**Ctrl+scroll** zooms the Hits Surface like File Pilot: tight list → roomy list → grid.

Settings (gear): extra Exclude / include Mounts, compactness (Zoom), spacing, Space previews **hovered** or selected, Reset to default. `~/.config/qfind/config.toml`.

Plugins (`docs/plugins.md`): `./packaging/install.sh` drops the Nautilus
“Search with Qfind” menu and builds the Vicinae extension into
`~/.local/share/vicinae/extensions/qfind`. KDE: `qfind-qt` (Qt6 Widgets, Breeze)
if Qt6 is installed.
Experimental **tree** toggle; bottom **WeightMap** of folders (WizTree-style).
TUI (`qfind`): **f3** preview, **f4** tree, **f6** WeightMap, **Ctrl+scroll / ^+ ^-** zoom,
**^o** show in Files, **^y** copy path, **tab / f2 / ctrl-d** Drag (`ripdrag`).
