# Filelight research for Qfind storage mode

Research snapshot: KDE Filelight commit [`66bad288`](https://invent.kde.org/utilities/filelight/-/tree/66bad2883aaf7875b3109691bbeb44e8d771ab80), inspected 2026-09-01. All behavior below comes from KDE's handbook or upstream source, not screenshots or third-party descriptions.

## What Filelight actually does

Filelight is an on-demand directory scanner. It recursively builds an in-memory `Folder` tree for one requested URL, then projects that tree into concentric proportional rings. It is not a persistent, whole-machine file index. The handbook describes the same model: scan a folder, then inspect a segmented-ring map whose areas are proportional to file size ([handbook: scanning and map interaction](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/doc/index.docbook)).

That distinction matters for Qfind: its Catalog can locate names globally and instantly, while storage accounting needs file sizes and directory totals. Before this work, Qfind rebuilt with `getdents64` without per-file `stat` and wrote size `0` for every item. The implementation now captures apparent file size during the same parallel Catalog walk and persists it in the existing snapshot entry, so the chart does not need a second scanner or sidecar. Older snapshots fall back to a clearly labelled item-count chart until the next Catalog rebuild.

## Radial map geometry

Filelight's model is a sunburst, not a flat pie chart:

- The center is the currently focused folder. Its children occupy the first ring; descendants retain their parent's angular interval in subsequent rings.
- Every segment's angular span is `full_circle * item_size / root_size`. Filelight represents the full circle as 5760 units (16 units per degree). Child recursion receives the parent's start and end angles, so ancestry is visually preserved ([map construction](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/radialMap/map.cpp), [segment angles](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/radialMap/radialMap.h)).
- Visible depth defaults to four. Ring breadth is derived from the shorter viewport dimension and visible depth, clamped to 20-60 pixels. Zoom changes visible depth and rebuilds the projection, rather than geometrically scaling a bitmap ([map sizing and zoom](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/radialMap/map.cpp)).
- Tiny segments are suppressed below a pixel-derived angular threshold. At depth zero, or at every depth when `showSmallFiles` is enabled, the suppressed weight becomes one synthetic "files group" segment. This prevents a haze of unclickable slivers while preserving total area ([small-item grouping](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/radialMap/map.cpp), [setting](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/filelightsettings.kcfg)).
- The current QML renderer uses a useful shortcut: it draws filled pie wedges at increasing radii, orders inner levels above outer levels, and lets smaller circles cover the inner portions of larger wedges. The overlap produces ring sectors without constructing explicit annular paths ([QML stacking](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/qml/MapPage.qml), [segment wedge](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/qml/SegmentShape.qml)).

For GTK/Cairo, explicit annular paths are simpler and cheaper to hit-test: each visible segment can be stored as `(inner_radius, outer_radius, start_angle, end_angle, node_id)`. Hit testing is one radius check plus normalized `atan2`; painting is an outer arc, inward line, reverse inner arc, and close. Keep the layout independent of pixels except for the tiny-segment cutoff.

## Interaction model worth copying

Filelight's chart and folder list are two views of the same nodes:

- Pointer motion finds the topmost shape, darkens it, shows a tooltip, and highlights the corresponding folder-list row. Hovering a list row performs the reverse highlight ([shared hover state and hit testing](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/qml/MapPage.qml)).
- Tooltip content is path, human-readable size, and descendant file count/percentage ([tooltip construction](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/qml/MapPage.qml)).
- Left-clicking a folder re-centers the map there. Clicking a file opens it externally. Clicking the center moves to the parent. Synthetic small-file groups are deliberately not navigable ([click handling](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/qml/MapPage.qml)).
- Right-click exposes open, open terminal, center here, exclude from future scans, rescan branch, copy path, and delete. Dropping a URL starts a scan there ([context actions and drop target](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/qml/MapPage.qml)).
- Back, forward, and up are ordinary filesystem navigation, not chart-specific modes ([navigation context](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/mainContext.cpp)).

Qfind's first useful version should keep only the high-value interactions: hover highlight + tooltip, click to drill, center/back to parent, and synchronized selection with the file view. Destructive context actions belong to the manager operation layer, not the chart.

## Why Filelight misses other mounts

Filelight refreshes its mount list at the start of every scan using `QStorageInfo::mountedVolumes()`. It omits the root volume from the boundary lists and classifies every other mounted volume as remote only when its filesystem type is exactly `smbfs`, `nfs`, or `afs`; everything else is considered local ([mount discovery](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/localLister.cpp), [remote type list](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/Config.h)).

The default `scanAcrossMounts` value is `false`. Consequently, when scanning `/`, Filelight adds every non-root local mount path to the skip list. A mounted NTFS volume is not intrinsically unsupported; it is normally classified as local, but a root scan stops at its mount boundary. The user must scan that mount path directly or enable cross-filesystem scanning ([defaults](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/filelightsettings.kcfg), [boundary exclusion](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/localLister.cpp)). The overview offers Home, Root, and an arbitrary folder picker; it does not present a persistent all-volumes storage root ([overview](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/qml/OverviewPage.qml)).

Additional upstream limitations visible in the source:

- Only currently mounted volumes are discovered. Unmounted block devices are outside the model.
- The remote-filesystem allowlist misses common identifiers such as `cifs`, `smb3`, and `sshfs`; those can be misclassified as local. This is an inference from the exact three-value set above.
- The static local/remote mount lists are appended to but never cleared before refresh. A path from an unmounted volume can remain a boundary exclusion until process restart. This is an inference from `readMounts()`.
- Default ignored paths are `/dev`, `/proc`, `/sys`, and `/root`; scanning `/` therefore intentionally omits the root user's home ([settings defaults](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/filelightsettings.kcfg)).

Qfind already recognizes Linux `ntfs`, `ntfs3`, `fuseblk`, `vfat`, and `exfat` as Catalog roots. Its storage surface should expose a virtual **All storage** root whose immediate children are discovered Catalog mounts plus explicit configured include roots. This avoids pretending `/` contains separate mounted filesystems and makes NTFS volumes first-class. Offline/unmounted configured roots should remain visible but disabled, with no blocking access attempt.

## Scanning, concurrency, cache, and accounting

Filelight's local scanner runs outside the UI thread and recursively scans sibling subdirectories through Qt's global thread pool. It attempts `tryStart`; when the pool is full it scans that branch inline, then waits on a semaphore and sorts children by descending size ([parallel recursion](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/localLister.cpp)). Cancellation is an atomic flag checked while enumerating entries. File count and total size are atomics read by the UI ([scan state](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/scan.h)).

The progress display polls those atomics every 16 ms. It does not stream event-driven tree deltas ([loading placeholder](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/qml/MapPage.qml)). The final radial signature is built synchronously and even sets a wait cursor because upstream labels it a slow operation; only QML shape creation/rendering is asynchronous ([map build](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/radialMap/map.cpp), [asynchronous shapes](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/qml/SegmentShape.qml)).

Its cache is an in-memory list of completed folder trees. A scan can reuse a cached ancestor immediately or graft cached descendant branches into a larger requested scan. Rescan invalidates relevant branches; changing scan settings empties the cache. It is not persistent across launches, and starting a second scan aborts and waits for the first ([cache lookup and scan lifecycle](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/scan.cpp), [settings cache invalidation](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/qml/SettingsPage.qml)).

On POSIX, Filelight reports allocated disk blocks (`st_blocks * DEV_BSIZE`), not apparent file length. It skips symlinks and special files. Its hard-link de-duplication set belongs to one directory walker, so links in different directories may still be counted twice; that last point is an inference from the walker lifetime ([POSIX accounting](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/posixWalker.cpp), [walker state](https://invent.kde.org/utilities/filelight/-/blob/66bad2883aaf7875b3109691bbeb44e8d771ab80/src/posixWalker.h)).

## Implementation guidance for Qfind

Use existing seams; do not create a second file index.

1. Add a **Preview / Storage** switch to the existing right pane. Preview remains the default. Storage owns one drawing area plus one compact breadcrumb/back row.
2. Use a virtual root with one child per mounted/configured Catalog root. Build chart hierarchy and weights from Catalog parent IDs, paths, and persisted size fields.
3. Capture sizes during the existing parallel Catalog walk. Build the in-memory aggregate map on a worker and deliver it through the GLib executor; never poll at 60 Hz or mutate GTK widgets from workers.
4. Reject stale generation results when a newer Catalog arrives. Existing zero-size snapshots stay usable as item-count maps and become byte-weighted after their next rebuild.
5. Keep one accounting meaning visible and explicit: **disk usage** should use allocated blocks when the platform exposes them; **apparent size** can be a later toggle. Never silently mix the two.
6. Layout only the visible root and a bounded number of rings. Aggregate sub-pixel children into **Other**. Re-layout on size/depth/root changes; pointer motion only performs hit testing and queues redraw.
7. Reuse the current theme palette. Derive child colors from their top-level mount/folder hue, vary luminance by depth, and reserve the accent for hover/selection. Preserve a thin background-colored separator between segments.
8. Chart navigation changes the storage focus, not the file-manager directory unless the user double-clicks/activates an explicit **Open folder** action. This avoids surprising location changes during storage exploration.

### Minimum data handed to the renderer

```text
StorageNode { id, path, name, allocated_bytes, child_count }
StorageArc  { node_id, depth, inner, outer, start, end }
StorageView { root_id, total_bytes, arcs, selected_id, hovered_id }
```

`StorageArc` is a transient projection, not stored state. A single flat vector supports paint order, reverse-order hit testing, tooltips, and selection without GTK widget-per-segment overhead.

### Where Qfind can be better than Filelight

- **All volumes are visible:** local mounts, NTFS/fuseblk drives, and configured roots are peers under one virtual root instead of being hidden behind `/` boundary semantics.
- **Instant revisit:** persistent Catalog hierarchy and size fields replace Filelight's process-local cache.
- **Responsive work:** cancellation tokens and coalesced update events replace a 16 ms progress poll and blocking scan replacement.
- **One selection model:** chart, grid/list, search results, Places, and preview share path identity and hover/selection rather than behaving as separate applications.
- **Truthful status:** each mount/node can show `indexed`, `scanning`, `stale`, `offline`, or `permission-limited` instead of presenting incomplete totals as complete.

Do not add GPU rendering, a database, a daemon, filesystem-specific NTFS parsing, or an elaborate chart framework for the first version. Cairo can comfortably draw and hit-test the bounded visible arc vector; add more machinery only after profiling the implemented path.
