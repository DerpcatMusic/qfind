# Dolphin patterns worth adopting in Qfind

Research date: 2026-09-01. Sources are KDE's current Dolphin tree at commit
[`03ac3e3c`](https://invent.kde.org/system/dolphin/-/tree/03ac3e3ca71b074402f82a132b4284914e48fc13)
and first-party KDE/GTK documentation.

## The useful hierarchy

Dolphin has a stable three-level navigation model:

1. **Window history and location**: Back/Forward plus one location bar per active
   view. The bar is a breadcrumb by default, each crumb can expose its children,
   and `Ctrl+L`/`F6` turns the same control into an editable path.
2. **Persistent Places**: bookmarks, devices/media, recent/search locations and
   mounted storage remain in a left rail. Entries can be reordered by drag and
   drop; folders can be dropped onto the rail; entries can be renamed, hidden or
   removed. If the rail is hidden, the location bar exposes a compact Places
   selector instead of deleting the capability.
3. **Current directory**: the main view contains the immediate children of the
   current directory. Search is a temporary scoped mode, not a replacement for
   the current location.

This is why navigation stays legible: the current path has one owner, Places do
not pretend to be directory contents, and search does not silently change the
meaning of folder traversal. See the official [Dolphin view and location-bar
documentation](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/dolphin-view.html)
and [Places panel documentation](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/panels.html).

**Qfind application:** keep one `current_directory` shared by Classic, Qfind,
Preview and Chart. Opening a folder from any surface changes that value; Back,
Forward, Up and breadcrumb selection change it through the same navigation
function. Classic shows direct children; Qfind searches descendants of that
same directory. A Places click is just another navigation request.

## Split view is two complete views

Dolphin's split view does not bolt a second file list onto shared incidental
state. Each pane is a `DolphinViewContainer` with its own URL navigator and URL;
one pane is explicitly active, `Tab` can switch active panes, and closing split
view has a defined choice of active, inactive or right pane. It also exposes
direct Copy/Move-to-inactive-pane actions. The current implementation is in
[`dolphintabpage.cpp`](https://invent.kde.org/system/dolphin/-/blob/03ac3e3ca71b074402f82a132b4284914e48fc13/src/dolphintabpage.cpp).

**Qfind application:** do not overload the Preview pane into a split file view.
If split browsing is added, instantiate the existing directory surface twice,
give each its own navigation history/selection, and keep exactly one active pane.

## Density, modes and visual hierarchy

- Dolphin exposes Icons, Compact and Details as different layouts over the same
  current-directory model. Details can optionally expand folders in-place.
- Zoom is continuous user intent: toolbar/status controls change icon size while
  remaining in the current view mode. Icon-grid width is derived from icon size,
  text width and font metrics rather than fixed sparse cards; current source is
  [`dolphinitemlistview.cpp`](https://invent.kde.org/system/dolphin/-/blob/03ac3e3ca71b074402f82a132b4284914e48fc13/src/views/dolphinitemlistview.cpp).
- View properties—mode, zoom, previews, hidden files, sort/group roles and order—
  can be global or remembered per directory. Dolphin stores local settings in a
  `.directory` file when appropriate and falls back to its own config storage for
  remote, slow or unwritable locations; see
  [`viewproperties.cpp`](https://invent.kde.org/system/dolphin/-/blob/03ac3e3ca71b074402f82a132b4284914e48fc13/src/views/viewproperties.cpp).
- The persistent chrome is quiet: native theme background, one strong selection
  surface, muted metadata and full-row alignment in Details. The status bar owns
  transient hover/selection facts and zoom, avoiding repeated decoration inside
  every item. These relationships are visible in the official
  [view documentation](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/dolphin-view.html).

**Qfind application:** reduce the minimum grid cell to `max(thumbnail width,
label width)` plus one small gutter, and let zoom alter thumbnail/cell dimensions
without switching List/Grid. Use native theme roles, not a second unrelated
palette: neutral window and item surfaces, one accent for focus/selection, muted
secondary text, and subtle alternating rows only in list/details mode.

## One compact View menu for sort, group and filter

Current Dolphin puts View Mode, Zoom, **Sort By**, **Group By**, Additional
Information, Previews and Hidden Files under one `View Settings` action. Sort has
one exclusive field, Ascending/Descending, Folders First and Hidden Last. Group
has None, Same as Sort, or an independent field. The menu is built in
[`dolphinviewactionhandler.cpp`](https://invent.kde.org/system/dolphin/-/blob/03ac3e3ca71b074402f82a132b4284914e48fc13/src/views/dolphinviewactionhandler.cpp).

Dolphin keeps two text operations distinct: `Ctrl+I` filters the already loaded
current view by name and closes with `Esc`; `Ctrl+F` searches recursively from
the current folder (or everywhere), starts as the user types, and can narrow by
type/time/rating/tag. See [Filtering and Finding Files](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/quick-tips.html).

**Qfind application:** the requested funnel button should open one stateful menu:

- Sort: Name, Size, Type, Modified; Ascending/Descending; Folders first.
- Group: None, Same as sort, Type, Date, Size.
- Scope: Current directory / Everywhere.
- Visibility: Hidden files and ignore rules.

Show the active non-default choice as a small label or dot on the funnel; do not
add a permanent toolbar row. Keep Qfind's query box as recursive search and add a
lightweight in-view filter only if users need both operations simultaneously.

## Context-menu extensibility without a plugin framework

Dolphin's simplest extension system is deliberately data-driven. A ServiceMenu
is a `.desktop` file discovered from `kio/servicemenus`; it declares MIME types,
one or more actions, labels, themed icons and an `Exec` template. `inode/directory`
targets folders, `image/*` targets a family, `%u`/`%U` pass one/many URLs, and
protocol plus selection-count constraints control visibility. `X-KDE-Submenu`
groups all actions from one file into a named dropdown. User-local services live
in `~/.local/share/kio/servicemenus`; system services normally live in
`/usr/share/kio/servicemenus`. KDE's full first-party specification and examples
are in [Creating Dolphin service menus](https://develop.kde.org/docs/apps/dolphin/service-menus/).

**Qfind application:** support this existing ServiceMenu subset directly instead
of inventing a plugin ABI. Load `.desktop` files from Qfind's own actions folder
and, optionally, the two KDE locations. Implement only:

```ini
[Desktop Entry]
Type=Service
MimeType=image/*;inode/directory;
Actions=inspect;convert;
X-KDE-Submenu=Tools

[Desktop Action inspect]
Name=Inspect
Icon=document-properties
Exec=my-tool %U
```

Filter by MIME and selection count before constructing the menu. Expand field
codes into an argument vector; never interpolate selected paths into a shell.
Only run a shell when the manifest explicitly names one. This gives users files,
folders and dropdowns with hot-reloadable configuration and also makes most
Dolphin ServiceMenus portable. Dynamic UI plugins and VCS overlays can wait until
a declarative action genuinely cannot express a needed operation.

## Migration that is worth doing

### Dolphin / KDE

KDE Places are exposed by `KFilePlacesModel` and represented by the XDG data file
`user-places.xbel`; the model includes groups for Places, Remote, Recent, Search,
Devices and Tags ([KFilePlacesModel API](https://api.kde.org/kfileplacesmodel.html)).
Import `$XDG_DATA_HOME/user-places.xbel` (normally
`~/.local/share/user-places.xbel`), preserving URL, label, icon and order. Qfind
can also discover compatible ServiceMenus from the KDE paths above—no copy is
required.

### Nautilus / GTK

GTK moved user bookmarks to `$XDG_CONFIG_HOME/gtk-3.0/bookmarks` with the legacy
fallback `~/.gtk-bookmarks`; this is documented in GTK's
[3.6 migration notes](https://gnome.pages.gitlab.gnome.org/gtk/gtk3/changes.html#changes-in-gtk-36).
Import each URI and optional label, preserving order. GTK's Places model already
defines the expected grouping—home, bookmarks, drives and volumes—and drive
mount/unmount behavior; see the official
[`GtkPlacesSidebar` documentation](https://gnome.pages.gitlab.gnome.org/gtk/gtk3/class.PlacesSidebar.html).

### Import policy

- Present one **Import Places** action with Dolphin and Nautilus as detected
  sources; preview the count before writing.
- Read source files only. Normalize file URLs, deduplicate by URL, preserve custom
  labels/order, and never remove the source manager's bookmarks.
- Import bookmarks first. Do not migrate Dolphin's every-folder display settings
  or Nautilus extensions: those semantics do not map cleanly and are not needed
  to make Qfind immediately familiar.

## Minimum implementation order

1. One shared navigation function and history for Places, list/grid and Chart.
2. Dense grid sizing plus the single funnel View menu.
3. Declarative context actions with `X-KDE-Submenu` compatibility.
4. Read-only Dolphin/Nautilus Places import.
5. Split browsing only after ordinary navigation and file operations are solid.

