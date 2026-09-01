# Nautilus patterns worth borrowing

Research snapshot: GNOME Nautilus `main` at
[`b89433c2`](https://gitlab.gnome.org/GNOME/nautilus/-/tree/b89433c275b8eafec9b523a6c144444b435d3c86).
Only GNOME help, Nautilus source, and the first-party extension API are used.

## What makes Nautilus feel composed

Nautilus has three stable visual layers:

1. A Places sidebar with its own small header and selected-location state.
2. A content header with Back/Forward, one breadcrumb capsule, Search, and one
   split View Options button.
3. A quiet content surface where system icons, names, metadata, hover, and the
   selected item carry the hierarchy.

The sidebar and content are separate `Adw.ToolbarView` surfaces inside an
`Adw.OverlaySplitView`; below 682sp the sidebar collapses instead of squeezing
the content. The path bar is one subtly raised, rounded surface using only a
10% foreground mix. Selection and hover use the desktop accent and theme
surface colors rather than permanent colored borders
([window composition](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/resources/ui/nautilus-window.blp),
[path bar and view CSS](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/resources/style.css)).

The supplied Qfind view flattens Back, Forward, Up, Pin, Refresh, Settings,
location, tree, chart, preview, zebra, zoom, mode, matching, sorting, class,
folder filtering, and search into two always-visible strips. Most controls have
equal contrast, so the actual location and selected file do not lead the eye.

### Apply to Qfind

- Keep Back/Forward at the start, the current breadcrumb/location in the
  center, and Search plus one View Options split button at the end.
- Move zebra, Chart visibility, preview behavior, match mode, and density out
  of the permanent header. They belong in View Options or Settings.
- Keep **Classic / Qfind** visible because it is Qfind's differentiator, but
  render it as one compact segmented control beside Search.
- Use the theme's normal view and sidebar surfaces. Reserve the accent for the
  current Place, selected row/tile, focus ring, and active mode. Paths and
  metadata stay dim; filenames stay primary.
- Replace the editable location field at rest with breadcrumbs. `Ctrl+L`
  swaps the same space to a text entry with an explicit close/cancel button,
  matching Nautilus's pathbar/location stack
  ([toolbar source](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/resources/ui/nautilus-toolbar.blp)).

## Navigation hierarchy

Nautilus treats browsing and searching as states of the same window. Opening a
folder replaces the current directory; a breadcrumb parent moves back up;
Back/Forward retain history; middle-click or context actions open a new tab.
Typing searches in or below the displayed folder and `Esc` returns to that
folder. The sidebar exposes common Places, bookmarks, devices, network, and
Trash, while the path bar always explains the current directory
([browsing help](https://help.gnome.org/gnome-help/files-browse.html),
[network locations](https://help.gnome.org/gnome-help/nautilus-connect.html)).

### Apply to Qfind

- Make `current_directory` the single navigation truth. Folder activation from
  Places, breadcrumbs, list/grid, or Chart updates that same value, history,
  main model, preview, and Chart root.
- Chart drill-down must navigate the main panel too; Back/Up or clicking the
  Chart center must move both outward. Conversely, folder activation in the
  main panel must recenter a directory-scoped Chart.
- Preserve two scopes, not two unrelated navigators: **Directory** shows only
  the current subtree; **Global** shows every indexed root. A scope toggle may
  change Chart data, but it must not silently change the browsed directory.
- Keep Places stable while only the current-place highlight moves. Follow
  Nautilus's sidebar order: Home and special locations, user bookmarks, then
  mounts/devices. Do not mix transient search controls into Places.

## Grid and list density

Nautilus has five grid thumbnail sizes—48, 64, 96, 168, and 256 px—and a grid
with 18 px outer padding, 6 px gaps, and 6 px cell padding. Its list has 24 px
horizontal insets, 8 px row spacing (4 px compact), and 6 px cell padding (3 px
vertically in compact mode). Grid labels wrap to at most three centered lines
and ellipsize in the middle. Larger zoom levels reveal more optional captions
([size constants](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/nautilus-enums.h),
[grid implementation](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/nautilus-grid-view.c),
[grid cell](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/resources/ui/nautilus-grid-cell.blp),
[view CSS](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/resources/style.css)).

The supplied Qfind grid uses large fixed square allocations even when the
preview is narrow or a generic icon has no visual content. The empty space
dominates the files.

### Apply to Qfind

- Set the smallest visual grid to roughly a 48 px thumbnail plus one compact
  label line, with 6 px inter-item gaps. It should fit materially more items
  than the current minimum.
- Let width determine column count; do not preserve oversized square cells.
  Use a compact portrait cell and cap label lines instead.
- Retain selection by path and the scroll anchor while zoom changes. Zoom
  changes thumbnail/cell geometry; it should not unexpectedly switch scope or
  reset the directory.
- Follow Nautilus's restrained thumbnail treatment: a tiny outline/shadow for
  full-color thumbnails, no heavy card around every unselected item, and the
  system icon theme for generic files and folders.

## Sort, group, and filter

Nautilus uses one View Options split button. Its popover contains inline zoom,
sort order, hidden files, list columns, and grid captions. Grid sorting offers
A–Z, Z–A, modified ascending/descending, size, and type; list columns are also
direct sort controls. Search has a separate filter affordance for date, type,
and filename versus full-text matching, displayed as removable filter tags
([View Options model](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/resources/ui/nautilus-view-controls.blp),
[sort model](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/resources/menu/nautilus-toolbar-view-menu.ui),
[search help](https://help.gnome.org/gnome-help/files-search.html)).

Nautilus does **not** provide a general Group By control in ordinary local
folders. Its main grouping preference is folders-before-files. Qfind should
not copy a capability Nautilus does not have.

### Apply to Qfind

- Replace the always-visible sort/class/folders widgets with one
  `view-filter-symbolic` funnel button.
- Popover sections: **Sort** (Name, Modified, Size, Type, Relevance plus
  direction), **Group** (None, Folder/File, Type, Date), and **Filter** (type,
  hidden, folders only). Show active choices as a short subtitle or removable
  chips beside Search.
- Keep list headers clickable where columns are visible. The funnel is the
  discoverable common control, not the only route.
- Keep filename matching mode close to Search only while Qfind mode is active;
  hide it when it has no effect in Classic mode.

## Context menus and extensions

Nautilus defines background and selection menus declaratively in `GMenu`, with
explicit insertion sections for extensions. A `MenuProvider` receives either
the selected files or current directory, returns menu items, and can attach a
`Nautilus.Menu` submenu. This keeps extension UI model-based instead of letting
extensions mutate arbitrary GTK widgets
([built-in menu model](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/resources/menu/nautilus-files-view-context-menus.ui),
[`MenuProvider`](https://gnome.pages.gitlab.gnome.org/nautilus/iface.MenuProvider.html),
[`Nautilus.Menu`](https://gnome.pages.gitlab.gnome.org/nautilus/class.Menu.html)).

Nautilus also exposes the deliberately simple
`~/.local/share/nautilus/scripts` directory as a **Scripts** submenu. It passes
selected local names as arguments and exports newline-delimited selected paths,
selected URIs, and the current URI through `NAUTILUS_SCRIPT_*` environment
variables
([user documentation](https://help.gnome.org/gnome-help/nautilus-behavior.html),
[launcher source](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/nautilus-files-view.c)).

### Apply to Qfind

- Use one small data model: `label`, optional icon, command, applicability
  (file/folder/background/MIME), and optional `children`. Render the same tree
  into `GMenu`; `children` is the requested context-menu folder/dropdown.
- Load user actions from an XDG config directory, not Rust plugins in the GUI
  process. Execute commands out of process with explicit selected-path and
  current-directory variables.
- Add a **Nautilus Scripts compatibility** switch that reads the existing
  scripts directory and supplies the same arguments and environment. That is
  useful migration with no converter and no duplicate configuration.
- Defer binary/Python extension emulation. Nautilus API 4 exposes columns,
  file info, menus, and property models, but loading foreign in-process
  extensions would couple Qfind to Nautilus internals
  ([extension API surface](https://gnome.pages.gitlab.gnome.org/nautilus/),
[Python extension locations](https://gnome.pages.gitlab.gnome.org/nautilus-python/nautilus-python-overview-example.html)).

## Migration that can be automatic

- **Bookmarks:** Nautilus reads and monitors
  `$XDG_CONFIG_HOME/gtk-3.0/bookmarks` (or
  `~/.config/gtk-3.0/bookmarks`). Qfind can read the same URI-plus-label file
  directly and merge it with Qfind pins, preserving order and labels
  ([bookmark source](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/src/nautilus-bookmark-list.c),
  [bookmark behavior](https://help.gnome.org/gnome-help/nautilus-bookmarks-edit.html)).
- **Scripts:** expose the existing Nautilus Scripts directory as described
  above; do not copy or rewrite user scripts.
- **Preferences:** offer a one-time import of view mode, grid/list zoom, sort,
  reverse sort, thumbnail policy, captions/columns, click policy, and hidden
  files from the documented Nautilus/GTK GSettings keys. Keep it optional and
  translate values into Qfind concepts rather than continuously mirroring
  desktop settings
  ([schema source](https://gitlab.gnome.org/GNOME/nautilus/-/blob/b89433c275b8eafec9b523a6c144444b435d3c86/data/org.gnome.nautilus.gschema.xml)).
- **Extensions:** list detected Nautilus Python extensions during migration,
  but do not claim compatibility. Only scripts and bookmarks have safe,
  deterministic reuse paths.

## Implementation order

1. Make the directory path the shared navigation state across Places,
   breadcrumbs, list/grid, Preview, and Chart.
2. Replace the editable-at-rest location with breadcrumb/location modes and
   simplify the header into navigation, location, Search, and View Options.
3. Compact grid geometry to the 48/64/96/168/256 progression and use theme
   surfaces plus accent selection instead of black cards and equal-weight
   chrome.
4. Add the funnel popover and show active filters as compact chips.
5. Introduce the model-based context action tree, then read Nautilus bookmarks
   and Scripts directly for migration.

Do not reproduce Nautilus's whole extension ABI, Tracker search stack, or
libadwaita window shell. Qfind already has the stronger search/catalog core;
the high-value borrow is its hierarchy, navigation contract, density, and
small model-based menu seam.
