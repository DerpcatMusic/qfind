# Qfind

Instant filename search across local Mounts. The Catalog is the thing callers talk to; Hits are what they see.

## Language

**Catalog**:
The complete set of findable files and folders Qfind currently knows.
_Avoid_: index, database, engine, corpus

**Hit**:
One file or folder from the Catalog that matched a Query.
_Avoid_: result, match, document, record, entry

**Query**:
The text the user typed to filter the Catalog.
_Avoid_: pattern, filter (as the typed thing), search string

**Exclude**:
A rule that keeps a path out of the Catalog.
_Avoid_: ignore, skip list, denylist

**Mount**:
A filesystem root the Catalog may include (a Linux mount, an NTFS volume).
_Avoid_: drive, volume, root (except as “the `/` Mount”)

**Rebuild**:
Refreshing the Catalog from the live filesystem.
_Avoid_: reindex, rescan, crawl, update database

**Drag**:
Handing selected Hits to another program as files.
_Avoid_: export, share, send

**Scope**:
Whether a Query keeps files, folders, or both.
_Avoid_: mode, view, collapse (as the filter itself)

**FileClass**:
A filename group (image, audio, video, document, archive) derived from extension.
_Avoid_: mime, type (alone), kind

**Sort**:
The order Hits are presented after a Query: Score, Name, NameDesc, Newest, Oldest, Largest, Smallest.
Newest/Oldest/size `stat` the matched Hits (file-manager style). Day/week buckets are not Sort.
_Avoid_: rank (except fuzzy Score)

**DateAge**:
Optional CLI window on mtime. Not the GTK/TUI date control — that is Sort::Newest / Sort::Oldest.
_Avoid_: recency as the date UX

**Surface**:
How Hits are laid out: Auto (list↔grid via Zoom), or Tree. Query typing stays in the search box; preview is a Hits-surface action.
_Avoid_: view (the GTK widget), layout, mode

**Zoom**:
0–100 scale. Ctrl+scroll. Below 40: list with growing row/icon size; 40 and up: grid with growing cells (File Pilot).
_Avoid_: scale (alone), magnification

**WeightMap**:
Folder rectangles for the current Hits, sized by weight (size, else count). WizTree-style strip, not the whole disk.
_Avoid_: heatmap, disk map, treemap (in UI copy)

**Config**:
User settings: extra Exclude, include Mounts, Zoom, spacing, PreviewMode, zebra, WeightMap, OpenMode, editor. `$XDG_CONFIG_HOME/qfind/config.toml`.
_Avoid_: preferences, options dump

**OpenMode**:
How Enter opens a Hit: Auto (`$EDITOR`/`$VISUAL` for text, desktop handler otherwise), Xdg (always MIME default), Editor (always the editor).
_Avoid_: launcher, handler (alone)

**PreviewMode**:
Space previews the hovered Hit or the selected Hit.
_Avoid_: quick look setting
