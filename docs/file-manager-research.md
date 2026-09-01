# Qfind as a file manager: research and architecture recommendation

Research date: 2026-08-31. Sources are official manuals, specifications, or upstream repositories. Features described as “current” refer to those sources on this date.

## Decision

Qfind can grow into a fast file manager without becoming four independent applications. The useful split is by capability, not by interface:

- Keep `qfind-core` responsible for Catalog construction, indexed Query, ranking, and query-derived views.
- Add a UI-independent manager core when the first filesystem mutation lands. It should own locations, history, tabs and panes, selection, file-operation jobs, conflicts, progress, cancellation, undo receipts, and Catalog change events.
- Keep the existing TUI and GTK frontends. Both should render the same manager session and operations; neither should implement copy, move, trash, conflict, or undo semantics itself.
- Treat a Linux system file picker as a separate adapter process. A file manager window alone cannot replace the toolkit or portal chooser.

The near-term product should still feel like Qfind: search is the primary navigation primitive, the WeightMap explains a result set, and previews remain available without changing modes. Copying a conventional file manager feature-for-feature would erase the parts that already distinguish it.

## What Qfind is today

The current repository already has more than a search box:

- An immutable, memory-mapped Catalog and multiple Query modes provide global filename search across the CLI, TUI, and GTK frontend ([Catalog implementation](../crates/core/src/catalog.rs), [Query implementation](../crates/core/src/search.rs), [workspace layout](../Cargo.toml)).
- Search supports fuzzy, substring, exact, glob, extension, class, folder, and date-oriented filtering, while hidden and ignore-rule settings can be changed from the interface ([README: search](../README.md#search-syntax), [configuration](../crates/core/src/config.rs)).
- The TUI already has list/grid density, a dual-pane folder browser, an interactive size-or-count WeightMap, asynchronous side previews, mouse interaction, and adjustable preview width ([README: features](../README.md#what-it-does), [TUI preview pipeline](../crates/tui/src/preview.rs), [TUI surface](../crates/tui/src/surface.rs)).
- The GTK frontend has list, grid, tree, preview, platform opening, and explicit re-indexing ([GTK surface](../crates/gtk/src/surface.rs), [GTK actions](../crates/gtk/src/actions.rs), [GTK rebuild path](../crates/gtk/src/main.rs)).

The core is nevertheless still a search core. It does not presently define a multi-selection model, filesystem operations, conflict decisions, an operation queue, undo records, live filesystem reconciliation, or a durable manager session. The current rebuild is explicit and snapshot-oriented, which is a good base for reads but not enough to keep a manager view coherent after external or in-app mutations.

## Desktop file-manager baseline

| Capability | Dolphin | GNOME Files (Nautilus) | Thunar | Consequence for Qfind |
|---|---|---|---|---|
| Navigation model | Back/forward history, tabs, split view, Places, tree, terminal and information panels. Views can be stored per folder. ([Dolphin handbook](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/index.html)) | Grid/list views, tabs, Places, recent/starred/network/trash locations, and type-to-search. GNOME’s help describes list-tree expansion and grid captions. ([Files help](https://help.gnome.org/gnome-help/files.html), [display preferences](https://help.gnome.org/gnome-help/nautilus-display.html)) | Tabs, split view, side pane, history, pattern selection, and incremental type selection. ([working with files](https://docs.xfce.org/xfce/thunar/working-with-files-and-folders), [hidden settings](https://docs.xfce.org/xfce/thunar/hidden-settings)) | Qfind needs first-class location/history/tab/pane state. Its current F4 browser is a useful interaction, but not yet the manager model. |
| Search | A local Filter bar narrows the current view. Recursive search can use Baloo for indexed filename/content search or KIO when indexing is unavailable; results can be filtered and saved. ([Dolphin search documentation](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/index.html#search)) | Type-to-search with filename, type, date, and full-text filters; actions work directly on results. Search locations can be configured. ([GNOME search help](https://help.gnome.org/gnome-help/files-search.html)) | “Find in Folder” delegates recursive search to Catfish; typed characters select matching visible items. ([Thunar manual](https://docs.xfce.org/xfce/thunar/working-with-files-and-folders)) | Qfind’s low-latency global filename Catalog, explicit matching modes, extension/class grammar, and query-following WeightMap are real differentiators. Preserve search as a location, not a modal dialog. |
| Thumbnails and previews | Preview toggle, zoom, an Information panel with a large preview, and inline previews through KDE’s file-thumbnail infrastructure. ([Dolphin view and panel documentation](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/index.html#panels)) | Grid thumbnails, configurable thumbnail behavior, and a previewer; current Nautilus also shares its previewer with the chooser. ([display preferences](https://help.gnome.org/gnome-help/nautilus-preview.html), [Nautilus NEWS](https://github.com/GNOME/nautilus/blob/main/NEWS)) | Tumbler is a separate D-Bus thumbnail service with plugins for images, video, PDF/PostScript, OpenDocument, raw images, fonts, and EPUB. ([Tumbler](https://docs.xfce.org/xfce/tumbler/start), [available plugins](https://docs.xfce.org/xfce/tumbler/4.20/available_plugins)) | Qfind’s image-rich TUI grid and broad side preview are unusual. The manager core should request typed preview artifacts; decoding, caching, and terminal/GTK rendering remain frontend services. |
| List/grid and zoom | Icons, Compact, and Details views; zoom changes item size and previews; display style can be remembered per folder. ([Dolphin view properties](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/index.html#view-properties-dialog)) | Grid icon size is zoomable; list view can expose expandable folders and columns. ([GNOME display help](https://help.gnome.org/gnome-help/nautilus-display.html)) | Ctrl+wheel zoom is supported, including view-dependent zoom settings; split orientation is configurable. ([Thunar hidden settings](https://docs.xfce.org/xfce/thunar/hidden-settings)) | Continuous density must preserve scroll anchor and selection. “List versus grid” should be a consequence of available cell geometry, not a surprising mode jump. |
| File operations | Copy/move, duplicate, trash/delete, restore, batch rename, drag-and-drop, conflict handling, progress, and Undo. Split view exposes copy/move-to-inactive-pane actions. ([Dolphin handbook](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/index.html#managing-files-and-folders)) | Copy/move, trash/restore, batch rename, conflict dialogs, progress and cancellation, and undo/redo are implemented as explicit subsystems. ([operations source](https://github.com/GNOME/nautilus/blob/main/src/nautilus-file-operations.c), [conflict dialog](https://github.com/GNOME/nautilus/blob/main/src/nautilus-file-conflict-dialog.c), [undo manager](https://github.com/GNOME/nautilus/blob/main/src/nautilus-file-undo-manager.c), [progress model](https://github.com/GNOME/nautilus/blob/main/src/nautilus-progress-info.c)) | Copy/cut/paste, drag-and-drop, trash/permanent delete, restore, links, permissions, duplicate, and bulk rename. The undo-history limit is configurable and defaults to ten operations. ([working with files](https://docs.xfce.org/xfce/thunar/working-with-files-and-folders), [hidden settings](https://docs.xfce.org/xfce/thunar/hidden-settings), [bulk renamer](https://docs.xfce.org/xfce/thunar/bulk-renamer/start)) | This is the largest gap. Mutations need typed jobs, structured conflicts, progress/cancel, and operation-specific undo receipts before Qfind can responsibly replace a desktop manager. |
| Extensibility and remote files | KIO provides network/protocol access; service menus and plugins add actions and panels. ([KIO overview](https://api.kde.org/kio-index.html), [Dolphin handbook](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/index.html)) | GIO/GVfs provides remote and virtual locations; scripts and extensions integrate with Files. ([GNOME Files help](https://help.gnome.org/gnome-help/files.html), [Nautilus extension API](https://gnome.pages.gitlab.gnome.org/nautilus-python/)) | GVfs supplies mounts, trash, and remote access. Plugins and user-defined custom actions extend the UI. ([Thunar start page](https://docs.xfce.org/xfce/thunar/start), [custom actions](https://docs.xfce.org/xfce/thunar/custom-actions)) | Do not invent a remote-filesystem abstraction in the first local-manager release. On Linux, use the platform’s mounted/GVfs surface first; add a backend trait only when a second implementation proves the seam. |
| Live updates | Dolphin’s file-item model consumes KDirLister change notifications, while Baloo maintains indexed search data. ([Dolphin model source](https://invent.kde.org/system/dolphin/-/blob/master/src/kitemviews/kfileitemmodel.cpp)) | Nautilus has directory-monitoring and search-engine layers rather than requiring a full user-triggered re-index. ([directory source](https://github.com/GNOME/nautilus/blob/main/src/nautilus-vfs-directory.c), [search engine source](https://github.com/GNOME/nautilus/blob/main/src/nautilus-search-engine.c)) | Thunar’s folder model installs a filesystem monitor for displayed directories; Tumbler runs independently for thumbnails. ([folder source](https://gitlab.xfce.org/xfce/thunar/-/blob/master/thunar/thunar-folder.c)) | A manager cannot rely solely on full Catalog rebuilds. Qfind needs direct operation deltas plus filesystem watching and periodic reconciliation. |

### What the incumbents still do better

The three desktop managers already handle the unglamorous correctness work: multi-selection semantics, cross-filesystem copy/move, trash and restore, name collisions, partial failure, cancellable progress, permission errors, external changes, mounts, devices, network locations, tabs/history, and persistent per-folder preferences. Dolphin and Thunar also provide real split panes; Nautilus has a mature operation/conflict/undo pipeline even without that layout emphasis.

### What Qfind can do better

Qfind can make search results behave like ordinary folders without paying a “search mode” tax. A result set can retain thumbnails, side preview, multi-selection, file operations, density zoom, and a WeightMap that explains where the bytes or file types are. Neither the manuals nor the upstream implementations above expose that combination as their primary interaction.

Its second advantage is cross-interface consistency. A single Catalog/query layer already feeds CLI, TUI, and GTK. If manager state is also shared, key behavior—selection, conflicts, undo eligibility, progress, and navigation—does not have to drift between a terminal and graphical frontend.

## Navigation latency: File Pilot and GTK evidence

File Pilot's useful lesson is separation, not its choice of C. Its official project page says
the UI, renderer, file API, threading, memory, and containers are purpose-built, and its early
development notes explicitly call out stoppable background threads for fast panel changes plus
multithreaded filtering ([Handmade project page](https://filepilot.handmade.network/)). Its
current product promises immediate startup, sub-second indexing/search/thumbnails, configurable
thumbnail loading, and no background service; direct NTFS file-table indexing is still listed as
future work, so the responsiveness shown today cannot be attributed only to a persistent index
([manifesto](https://filepilot.tech/about), [roadmap](https://filepilot.tech/roadmap)).
In the creator's engine talk, the concrete method is to profile first, overlap independent startup
work with graphics initialization, keep file/thumbnail fetching in the platform layer, cache slow
platform APIs, and group allocations by lifetime
([BSC 2025 engine talk](https://www.youtube.com/watch?v=bUOOaXf9qIM)). Qfind's measurements below
show why these are principles rather than prescriptions: row-object allocation took only tens of
microseconds, so copying File Pilot's arena strategy would not improve this navigation bottleneck.

GTK already encodes the same split: `GtkDirectoryList` wraps asynchronous GIO child enumeration,
fills a `GListModel` as results arrive, and exposes loading/error state. The gtk-rs book likewise
requires blocking I/O or CPU work to run outside GTK's main loop
([GTK DirectoryList](https://docs.gtk.org/gtk4/class.DirectoryList.html),
[gtk-rs main event loop](https://github.com/gtk-rs/gtk4-rs/blob/main/book/src/main_event_loop.md)).

Profiling Qfind's release build on this machine found that direct directory reading was already
fast: Pictures took 2.7 ms and Downloads 0.2–5.3 ms to enumerate and sort. The freeze was GTK cell
binding decoding full images synchronously: Pictures spent 1,016 ms and Downloads 302 ms in model
notification. Moving raster/vector/PDF/audio/video/office thumbnail generation to background work
reduced cold-cache first-populated-paint latency to about 6.3 ms for Pictures and 16 ms for
Downloads. Direct navigation also no longer pays the 50 ms Query-typing debounce.

The resulting performance rules are:

- Commit the new location immediately; names and icons are the first paint.
- Decode bounded thumbnails off the GTK thread and cache by path, mtime, size, and target geometry.
- Reject stale thumbnail results when a recycled cell now represents another path.
- Do not request metadata for name sorting; use `DirEntry::file_type` and stat only for size/date Sort.
- Keep Query debounce for keystrokes, never for explicit navigation.
- For directories large enough that enumeration becomes visible, stream batches into the model;
  do not add speculative indexing or a daemon before measurement shows the live reader is limiting.

## Rust projects worth learning from

### COSMIC Files: closest full-manager reference

[COSMIC Files](https://github.com/pop-os/cosmic-files) is the most relevant Rust implementation. Its source has several useful boundaries:

- A `Tab` owns a `Location`; locations cover paths, search, trash, network, recents, and desktop, while tabs carry view and selection state ([`tab.rs`](https://github.com/pop-os/cosmic-files/blob/master/src/tab.rs)).
- A typed `Operation` enum covers copy, move, trash/delete, restore, rename, create, compress/extract, and permission changes. `ReplaceResult` represents replace, keep-both, skip, and cancel, while a controller exposes running/paused/cancelled/failed progress ([operation model](https://github.com/pop-os/cosmic-files/blob/master/src/operation/mod.rs), [controller](https://github.com/pop-os/cosmic-files/blob/master/src/operation/controller.rs)).
- The app uses debounced `notify` watchers for active locations, trash, and recents ([`app.rs`](https://github.com/pop-os/cosmic-files/blob/master/src/app.rs)).
- Its reusable `Dialog` uses the same tab machinery for open-file, open-folder, multiple-selection, and save-file modes ([dialog implementation](https://github.com/pop-os/cosmic-files/blob/master/src/dialog.rs), [embedding example](https://github.com/pop-os/cosmic-files/blob/master/examples/dialog.rs)). This is an application component, not evidence that COSMIC Files itself is an xdg portal backend.

The source also shows a boundary Qfind should improve on: the operation module imports app/dialog messages, and the operation performer contains an explicit TODO about producing an inverse operation after an error. Qfind should keep operation policy independent from UI messages and should design undo receipts and partial-success behavior before shipping destructive operations ([operation source](https://github.com/pop-os/cosmic-files/blob/master/src/operation/mod.rs)).

### Yazi: best preview and background-task reference

[Yazi](https://yazi-rs.github.io/) is not a desktop file manager, but it is the strongest terminal reference:

- It has asynchronous file operations, a task manager with progress/cancellation, tabs, multi-selection, visual selection, bulk rename, incremental find, and `fd`/`rg` search whose results can be operated on directly ([feature list](https://yazi-rs.github.io/features/), [quick start](https://yazi-rs.github.io/docs/quick-start/)).
- Previewers and preloaders are MIME- or URL-matched plugins. Built-ins cover code, JSON, images, video, PDF, archives, and cached thumbnails; plugins can run concurrently and implement preview seeking ([preview configuration](https://yazi-rs.github.io/docs/configuration/yazi/), [plugin overview](https://yazi-rs.github.io/docs/plugins/overview/)).
- Its terminal image support spans Kitty, iTerm2, WezTerm, Konsole, Sixel, Windows Terminal, and fallbacks ([image preview documentation](https://yazi-rs.github.io/docs/image-preview/)).

Qfind should borrow the task and preview contracts, not Yazi’s whole architecture. Yazi has no GUI, desktop portal backend, persistent global Catalog, or query WeightMap.

### Broot: search-as-navigation and panel behavior

[Broot](https://dystroy.org/broot/) keeps directory hierarchy visible while fuzzy, regex, exact, and content searches narrow it. It cancels obsolete searches as input changes, supports user-defined verbs, and allows preview/second panels to follow selection and be resized ([input documentation](https://dystroy.org/broot/input/), [panels](https://dystroy.org/broot/panels/), [verbs](https://dystroy.org/broot/verbs/)).

The lesson is interaction-level: search should produce a navigable location with stable focus, and a preview should subscribe to selection rather than take over scrolling. Broot is not a desktop file-operation or portal blueprint.

### Joshuto: useful interaction catalogue, weaker core model

[Joshuto](https://github.com/kamiyaa/joshuto) offers tabs, previews, devicons, bulk rename, chooser output, asynchronous copy/cut/paste, trash integration, bookmarks, `fzf`, and `zoxide` ([README](https://github.com/kamiyaa/joshuto), [keymap reference](https://github.com/kamiyaa/joshuto/blob/main/docs/configuration/keymap.toml.md)). It is valuable for command vocabulary and keyboard affordances, but many capabilities intentionally compose external programs. That makes it a weaker reference for Qfind’s portable manager core.

### termscp: remote transfer boundary only

[termscp](https://github.com/veeso/termscp) is a remote-first TUI supporting SCP/SFTP, FTP, S3, SMB, WebDAV, and Kubernetes, with a local pane, transfer progress, bookmarks, sync, search, and file operations. It demonstrates that remote sessions and queued transfers should sit behind a transport boundary, not leak into navigation widgets. It is not a useful model for local indexing, desktop thumbnails, or a system picker, so remote support should not be in Qfind’s first manager milestone.

### xplr: extensibility reference

[xplr](https://xplr.dev/) is a minimal Rust TUI designed around Lua configuration and a message-based extension model ([upstream repository](https://github.com/sayanarijit/xplr)). It is evidence for a small command/message surface if Qfind later exposes plugins. A stable plugin ABI now would be premature; user actions that invoke commands cover most early extensibility needs.

### XFiler: discarded

The repository named [XFiler](https://github.com/XFiler-Community/XFiler) contains no file-manager implementation or Rust source from which to draw architectural evidence. It should not be treated as a peer project. The similarly named xplr above is the material Rust project.

## File pickers: the important boundary

“Use Qfind as my file picker” has different meanings on Linux, Windows, and macOS. A standalone file manager cannot make every application use its window.

### Linux: frontend portal and backend portal

The public `org.freedesktop.portal.FileChooser` API is called by applications. It carries open/save options such as multiple selection, directory selection, filters, current filter, choices, current folder/file/name, and returns selected URIs. The frontend portal also mediates sandbox access through the document portal ([public FileChooser specification](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.FileChooser.html)).

The desktop-specific chooser is a separate process implementing `org.freedesktop.impl.portal.FileChooser`. A new backend needs:

1. A D-Bus-activatable service exporting the implementation interface and request objects.
2. A `.portal` metadata file naming the interfaces and desktops it supports.
3. Correct OpenFile and SaveFile handling, including modal-parent attachment, cancellation/`Close`, filters, choices, writable intent, and normalized `file://` URI results.
4. Selection through the xdg-desktop-portal backend configuration, for example a user override under `$XDG_CONFIG_HOME/xdg-desktop-portal/` whose `[preferred]` entry maps `org.freedesktop.impl.portal.FileChooser` to Qfind.

Those requirements come from the [backend interface](https://github.com/flatpak/xdg-desktop-portal/blob/main/data/org.freedesktop.impl.portal.FileChooser.xml), [backend authoring guide](https://flatpak.github.io/xdg-desktop-portal/docs/writing-a-new-backend.html), and [`portals.conf` specification](https://github.com/flatpak/xdg-desktop-portal/blob/main/doc/portals.conf.rst.in).

This suggests a small `qfind-portal` executable. It would translate D-Bus requests into a constrained manager session, launch or reuse a Qfind graphical chooser window, and translate the result back to portal URIs/options. A TUI proof of concept is feasible—the [xdg-desktop-portal-termfilechooser project](https://github.com/GermainZ/xdg-desktop-portal-termfilechooser) already adapts terminal pickers, and [Yazi documents using it](https://yazi-rs.github.io/docs/tips/#use-yazi-as-the-file-picker)—but Qfind’s first-class backend should own the protocol rather than depend on a text-file wrapper.

Save mode is not “open mode with another button.” It must validate a prospective filename, honor `current_name`, `current_folder`, `current_file`, filters and choices, distinguish folder selection from file selection, and return the requested writable result. That is why the chooser should reuse the manager model but remain a constrained mode rather than the normal manager window.

### Dolphin, Nautilus, and Thunar are not equivalent here

- **Dolphin is not KDE’s native file-dialog implementation.** KDE’s `KFileWidget` supplies the contents of KDE file dialogs, including inline previews; KDE’s QPA integration can make `QFileDialog` use it ([KIO FileWidgets](https://api.kde.org/kiofilewidgets-module.html), [`KFileWidget`](https://api.kde.org/kfilewidget.html)). KDE’s portal backend has its own [file chooser implementation](https://invent.kde.org/plasma/xdg-desktop-portal-kde/-/blob/master/src/filechooser.cpp).
- **Current GNOME Files is an exception.** Nautilus 47 introduced a chooser UI and FileChooser portal implementation; its current source exports the implementation interface and constructs a dedicated chooser window ([NEWS](https://github.com/GNOME/nautilus/blob/main/NEWS), [`nautilus-portal.c`](https://github.com/GNOME/nautilus/blob/main/src/nautilus-portal.c), [`nautilus-file-chooser.c`](https://github.com/GNOME/nautilus/blob/main/src/nautilus-file-chooser.c)). Older GNOME installations and other desktops may instead use the separate [GTK portal backend](https://github.com/flatpak/xdg-desktop-portal-gtk/blob/main/src/filechooser.c).
- **Thunar is not Xfce’s general picker.** Thunar is the file manager; ordinary GTK applications use GTK’s chooser APIs, which may delegate to a native/portal dialog depending on toolkit and environment. Xfce deployments commonly provide a GTK-compatible portal backend, but installing Thunar alone does not replace those dialogs ([GTK `FileChooserNative`](https://docs.gtk.org/gtk4/class.FileChooserNative.html), [xdg-desktop-portal backend list](https://flatpak.github.io/xdg-desktop-portal/docs/#backends)).

Therefore a Qfind portal backend would replace the chooser for **portal-aware applications when selected as the active backend**. It would not force every unsandboxed GTK application using an in-process chooser, every Qt application using KDE/native QPA dialogs, Electron applications with custom behavior, or applications with their own picker to use Qfind. Toolkit and desktop configuration determines whether the application reaches the portal.

### Windows and macOS

Windows applications invoke the system Common Item Dialog through `IFileOpenDialog`/`IFileSaveDialog`; Microsoft documents it as an application-created COM dialog ([Common File Dialog](https://learn.microsoft.com/en-us/windows/win32/shell/common-file-dialog)). macOS applications invoke `NSOpenPanel`/`NSSavePanel` ([NSOpenPanel](https://developer.apple.com/documentation/appkit/nsopenpanel), [NSSavePanel](https://developer.apple.com/documentation/appkit/nssavepanel)). Neither platform exposes a general user-selectable backend equivalent to xdg-desktop-portal.

Qfind can be a native manager on both platforms and can offer an embeddable chooser API or CLI for applications that opt in. It cannot transparently become the universal system picker there without application-specific integration or unsupported system modification.

## Recommended module boundary

```text
qfind-core
  Catalog snapshot + incremental deltas
  Query/ranking/search locations
  Query-derived WeightMap data

qfind-manager-core  -> qfind-core
  Location + history
  Tab/pane/session state
  Multi-selection
  Operation jobs + conflict decisions
  Progress/cancel + undo receipts
  Watch/reconcile events

qfind-cli            -> qfind-core
qfind-tui            -> qfind-core + qfind-manager-core
qfind-gtk            -> qfind-core + qfind-manager-core
qfind-portal (Linux) -> qfind-manager-core + GTK chooser shell
```

Important constraints:

- `qfind-manager-core` must not import GTK, Ratatui, D-Bus, terminal-image protocols, or dialog widgets.
- An operation is data: sources, destination, collision policy, progress, outcome, and optional inverse receipt. The UI supplies decisions through messages; it does not perform the I/O.
- Search results and folders are both `Location`s. That lets copy, drag, preview, selection, history, and density behave identically in either one.
- Watch events are hints, not truth. Apply known in-app operation deltas immediately, consume filesystem events for external changes, and periodically reconcile against the filesystem/Catalog so dropped or coalesced events cannot leave permanent stale state.
- Undo is operation-specific. Rename/move can store inverse paths; trash can store restore metadata; copy undo deletes only artifacts whose identity still matches; permanent delete is not falsely advertised as undoable. Partial completion must produce a partial receipt.
- Preserve platform semantics: freedesktop trash/GVfs on Linux, native trash and shell integration on macOS/Windows, and platform conflict/permission details behind narrow adapters.

## Feasible delivery sequence

1. **Manager model without mutations.** Introduce `Location`, history, tabs/panes, multi-selection, and stable list/grid anchors. Make directories and search results use the same model in GTK and TUI.
2. **Safe local operations.** Add create, rename, copy, move, duplicate, and trash/restore as cancellable jobs with progress and structured collisions. Update visible locations and Catalog state immediately from their outcomes.
3. **Correctness under change.** Add watched active locations, incremental Catalog deltas, reconciliation, operation-specific undo receipts, and recoverable job reporting. This is the minimum credible Nautilus replacement.
4. **Desktop polish.** Add bulk rename, properties/permissions, tabs and true split panes in both frontends, richer drag-and-drop, mounts/devices, and per-location view preferences.
5. **Linux chooser adapter.** Once constrained selection/save sessions are solid, ship `qfind-portal` with D-Bus activation, `.portal` metadata, backend configuration documentation, and a compatibility matrix. Keep the normal manager executable usable without it.
6. **Only after evidence of demand:** remote transports beyond platform mounts, archive-as-location, content indexing, a plugin ABI, or a persistent always-on daemon.

This order deliberately makes Qfind a trustworthy local manager before expanding its protocol surface. The first release that can plausibly replace Nautilus is not the one with the most previews; it is the one that never loses track of a file operation, collision, undo boundary, or external filesystem change.
