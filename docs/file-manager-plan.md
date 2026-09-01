# Qfind as a File Manager

Status: accepted direction; foundation work started 2026-08-31. Competitor evidence lives in
[`file-manager-research.md`](file-manager-research.md).

## Decision

Qfind should become a search-first file manager capable of replacing Nautilus for
local-file work. It should not split into four applications named Qfind GUI/TUI and
Manager GUI/TUI.

Keep two frontends:

- `qfind-tui`
- `qfind-gtk`

Each frontend should expose the same three contexts:

- **Search** — global or folder-scoped Catalog Queries.
- **Browse** — direct navigation through a directory.
- **Pick** — choose files or a destination for another application.

The existing `qfind-core` remains the deep module for Catalog, Query, filtering,
classification, and shared presentation geometry. A shared manager module should
own dangerous filesystem behavior once both frontends need it. Do not fork a
second product shell or duplicate operations in each frontend.

## Product thesis

> Find anything instantly, understand it without opening it, then act on one or
> thousands of files safely.

This is not “another Explorer.” Qfind's durable advantage is that Search, Browse,
Preview, grid Zoom, and WeightMap are different views of the same files and the
same selection. A Query should be actionable, not a temporary list users must
reconstruct in another file manager.

Primary assumption: the first replacement target is a Linux desktop power user
currently using Nautilus, with keyboard and mouse equally supported. macOS and
Windows retain the portable TUI and local-file features, but their native system
pickers are not an initial replacement target.

## Where Qfind already leads

These are product/architecture advantages, not unmeasured benchmark claims:

- A memory-mapped Catalog makes global filename Query the starting point rather
  than a slow fallback after folder navigation.
- Fuzzy, substring, exact, glob, extension, FileClass, date, and folder Queries
  share one interaction.
- Continuous Zoom crosses from dense list to visual grid instead of exposing
  unrelated view buttons.
- The Preview pane and grid can show generated images for text, Markdown, PDF,
  office, audio, video, fonts, SVG, and raster formats.
- WeightMap follows the current Query or browsed folder and can measure bytes or
  file count.
- Hidden-file and Git/`.ignore` visibility change immediately.
- The same engine already supports CLI, TUI, GTK, Nautilus, and launcher callers.

## What mature file managers still do better

| Capability | Qfind today | Replacement requirement |
|---|---|---|
| Selection | One focused Hit | Multi-select, ranges, select all/invert, stable selection across refresh |
| File operations | Open, reveal, copy path, outward Drag | Copy, move, rename, duplicate, create folder, Trash, delete |
| Safety | OS open errors and status text | Progress, cancel, conflicts, partial-failure report, Undo |
| Navigation | Search and a folder/items browser | Back/Forward, breadcrumbs/location entry, Places, recent locations |
| Two-folder work | Folder tree beside its items | Optional source/destination split with direct transfer |
| Freshness | Immutable snapshot plus full Rebuild | Immediate mutation overlay and event-driven external updates |
| Storage locations | Local Mount discovery | Trash, mount/eject, removable media state; remote locations later |
| Metadata | Name, path, size, time where requested | Properties, permissions, MIME/content facts, symlink target |
| Bulk workflows | Query and WeightMap filtering | Apply safe operations to selected Hits or a WeightMap group |
| Archives | Preview/classification | Browse and extract; archive creation/editing later |
| File picker | No chooser contract | Pick context first, desktop portal adapter separately |
| Extension ecosystem | Nautilus/Vicinae launch integration | Custom actions only after core operations are complete |

## File picker reality

Replacing Nautilus does not replace application Open/Save dialogs. GTK and
sandboxed applications may use a native chooser or an
`xdg-desktop-portal` FileChooser backend. Therefore:

1. Build **Pick** as a context of `qfind-gtk`, reusing its Search, Browse, grid,
   Preview, and selection behavior.
2. Give Pick an explicit request/result contract: open file(s), save destination,
   choose folder, filters, initial folder/name, multiple selection, accept/cancel.
3. Exercise that contract from Qfind's own CLI and integrations.
4. Only then add a small Linux portal adapter that translates the portal request
   into the Pick contract and returns selected URIs.

The portal adapter is a separate process because the system contract genuinely
varies; it must not create a second file-manager implementation. System-wide
picker replacement on macOS and Windows is not promised because those platforms
do not offer an equivalent general default-picker substitution.

## Target module shape

```text
qfind CLI ───────────────┐
qfind-tui ───────────────┼── qfind-manager ── qfind-core ── local filesystem
qfind-gtk ───────────────┘          │
                                    └── platform adapters

optional later:
desktop FileChooser portal ── qfind-gtk Pick context
```

### `qfind-core`

Retain ownership of:

- Catalog snapshot and Query execution
- filename/FileClass parsing and ignore visibility
- filesystem discovery and Excludes
- shared tree, grid Zoom, and WeightMap geometry
- a small live-delta interface so a rename/delete/create can become searchable
  without a full Rebuild

Do not turn core into an abstract virtual-filesystem framework. Local paths are
the real first implementation.

### `qfind-manager`

Start as a `manager` module rather than an empty crate. Extract it into a crate
when both GTK and TUI consume its operation interface.

It should be a deep module hiding:

- multi-selection independent of rows/widgets
- navigation history and current `Location`
- background operation queue
- copy/move/rename/create/Trash/delete execution
- byte/item progress and cancellation
- conflict decisions: replace, skip, keep both, apply to remaining
- operation journal and bounded Undo
- Catalog delta updates after Qfind operations
- platform-specific Trash and reveal/open adapters

The frontend-facing interface should speak in operations and events, not expose
filesystem loops:

```rust
submit(Operation) -> OperationId
cancel(OperationId)
resolve(OperationId, ConflictDecision)
undo(OperationId)
events() -> OperationEvent
```

This is an interface sketch, not a commitment to those exact Rust signatures.
Its purpose is to keep filesystem mutation, recovery, and Catalog reconciliation
out of widget callbacks and TUI key handlers.

### Frontends

GTK and TUI remain adapters over shared state and operations. They own:

- rendering and focus
- keyboard, mouse, and Drag gestures
- platform-appropriate menus/dialogs
- Preview presentation
- accessibility labels in GTK

They must not independently implement recursive copy, collision policy, Trash,
Undo, or Catalog mutation.

## Interaction model

### One surface, three contexts

- Opening Qfind starts in Search with the last useful location available, never
  a splash or blocking Rebuild.
- Entering a folder moves to Browse without replacing the result/grid language.
- Typing while browsing searches that folder immediately; one explicit action
  widens the same Query to everywhere.
- Zoom continuously changes density and crosses list/grid at a stable threshold.
- Preview follows focus but never steals Search input or scroll gestures.
- Pick uses the same surface and adds only the necessary Accept/Cancel outcome.

### Selection and action

- Click selects; Ctrl-click toggles; Shift-click extends; keyboard equivalents
  behave identically.
- Selection survives harmless sort, Zoom, Preview, and layout changes.
- Operations act on selection, including global Query Hits from many folders.
- Progress appears in a compact operation shelf; successful destructive actions
  offer Undo there.
- Conflicts pause only the affected operation and preserve completed work.
- WeightMap groups can filter first; users explicitly select before mutation.
  Clicking a large rectangle must never delete or move files directly.

### Navigation

- Back/Forward restore Location, Query, selection, scroll, and Zoom.
- A compact location control supports breadcrumbs and direct path entry.
- Places begins with Home, Downloads, Documents, mounted/removable storage,
  Trash, and user pins.
- Split mode is optional and progressive: two Locations with one active pane,
  shared Preview/operation shelf, and direct copy/move actions.

## Performance contract

The manager must preserve Qfind's reason to exist:

- Opening the shell and typing never waits for a filesystem walk.
- Query, directory reading, Preview generation, thumbnailing, watching, and file
  operations never execute on the input/render thread.
- Thumbnail and Preview work is viewport-driven, cancellable, deduplicated, and
  stale results cannot replace the current selection.
- Large directories stream useful rows instead of waiting for every metadata
  lookup.
- File operations emit events; the UI does not poll them on a timer.
- Qfind-originated changes update a mutable Catalog delta immediately. External
  changes enter the same delta through filesystem events. Periodic atomic
  compaction produces a new memory-mapped base snapshot.
- Performance claims require measured cold start, Query latency, large-directory
  navigation, scrolling, memory, and copy/move throughput before publication.

## Delivery sequence

Each slice must work in both GTK and TUI before the next one expands the shared
manager interface.

### 1. Replacement baseline

- Shared `Location`, navigation history, and stable multi-selection
- Home/Places, breadcrumbs/direct path entry, Back/Forward
- Copy, move, rename, create folder, duplicate, and Trash
- Background progress, cancellation, conflicts, partial-failure reporting
- Session Undo for manager-originated operations
- Immediate Catalog reconciliation for those operations

Exit condition: normal local work no longer requires opening Nautilus.

### 2. Live local filesystem

- Event-driven create/delete/rename/update ingestion
- Mutable delta merged with the memory-mapped Catalog during Query
- Atomic delta compaction without blocking interaction
- Mount arrival/removal and Trash state

Exit condition: external changes appear without a manual Rebuild and stale Hits
cannot be acted on silently.

### 3. Search-powered file picker

- Pick request/result contract and `qfind-gtk` Pick context
- Open one/many, select folder, and Save destination flows
- filters, initial location/name, overwrite confirmation
- Linux FileChooser portal adapter after the chooser behavior is solid

Exit condition: a supported Linux application can use Qfind to choose files with
thumbnails, continuous Zoom, folder/global Search, and Preview.

### 4. Power workflows

- Optional two-location split mode
- batch rename
- archive browse/extract
- properties and permissions
- duplicate/large/stale-file workflows driven by Query and WeightMap

Exit condition: Qfind is materially better than legacy managers for bulk local
work, not merely feature-compatible.

### 5. Deliberately deferred

- SMB/SFTP/cloud virtual filesystems
- plugin marketplace or general scripting runtime
- archive editing
- root/admin browser mode
- replacing native macOS or Windows file pickers
- a third frontend or a second “Manager” application

Add these only after replacement-baseline usage exposes a real need.

## First implementation cut

The foundation slice keeps the current architecture honest before mutations:

- `CatalogFolder` resolves an indexed directory once and scopes later Queries by
  Catalog parent IDs rather than rebuilding paths for every Hit.
- Typing from TUI Browse and `Ctrl+F` search the current directory recursively;
  `Ctrl+Shift+F` explicitly returns to the global Catalog.
- TUI Browse keeps session Back/Forward history without changing the Search
  model or creating a second manager shell.
- `Ctrl+L` opens direct absolute or relative directory paths in TUI Browse.
- GTK exposes the same folder scope through its header location, in-place folder
  activation, and session Back/Forward history.
- GTK Classic mode lists immediate children with folders first; Qfind mode keeps
  the recursive indexed view. Places, persistent pinned folders, and an embedded
  preview form the desktop manager shell around those shared results.
- TUI and GTK preserve the focused Hit across compatible Query and sort refreshes
  instead of resetting selection to the first row.
- `qfind-gtk --here` uses that same scope instead of partitioning a global Query
  into artificial “here” and “elsewhere” lists.

The next cut should not create every target crate or operation. Implement one
vertical mutation slice:

1. shared multi-selection and `Location` history;
2. Trash selected files through one manager interface;
3. progress/completion event in both frontends;
4. immediate removal from current Hits plus an Undo affordance.

Trash is the right first mutation because it forces the safety, event, selection,
platform, Catalog, and Undo seams without beginning with irreversible delete or
the much larger recursive-copy problem. Once the same interface is used by both
frontends, extracting `qfind-manager` into its own crate earns its place.

## Verification gates

- Existing CLI/TUI/GTK build checks stay green; no new test files are included in
  this planning change.
- Interactive acceptance is required on the target Linux desktop for selection,
  Drag, cancellation, conflicts, Trash restoration, and external file changes.
- Cross-compilation proves portability, not native Windows/macOS interaction.
- Picker completion requires an actual portal call from a sandboxed application;
  launching `qfind-gtk --pick` by hand is not enough.
- Destructive operations are not released until interruption and partial failure
  leave recoverable, accurately reported state.
