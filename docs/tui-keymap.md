# Qfind TUI keymap

The F1 help popup and the footer chips mirror this file. If they disagree,
this file wins — update all three together.

`Ctrl` is the `Cmd` equivalent (same as Finder muscle memory, one key over).

## Files (the daily-driver core)

| Keys | Action |
| --- | --- |
| `Delete` | Trash focused row, or every marked row. Recoverable: `Ctrl+Z`, or restore from `$XDG_DATA_HOME/qfind/Trash/files`. Triggers a Catalog rebuild. |
| `Insert` | Mark / unmark the focused row (`+` glyph, `N marked` chip). Marks live per tab. |
| `Ctrl+A` | Mark all visible rows, or clear marks when everything is marked. |
| `F2` | Rename the focused item (prompt; `Enter` applies, `Esc` cancels). |
| `F7` | New folder inside the current directory (prompt). |
| `Ctrl+Z` | Undo the last trash / rename / mkdir (up to 32 deep). |

Right-click a result for `Open With`, `Open Folder`, copy path/name/URI,
batch rename, copy, move, compress, extract, and action/script execution. Batch rename prompts for
find, replace, prefix, suffix, and numbering start, then shows a review before applying. Copy, move,
compression, and extraction run in the background and reject occupied or
nested destinations.

Opening a supported archive enters its persistent extracted workspace. `Ctrl+S`
saves changes back atomically when inside a writable archive workspace.

## List columns

List rows read `NAME | MODIFIED | SIZE` with fixed right-hand columns.
`MODIFIED` is a relative age (`5m`, `3h`, `9d`, `—` when unknown).
Click the header row to cycle sort (`Ctrl+S` does the same).

## Tabs

Each tab keeps its own query, cursor, marked set, and browsed directory.
The footer shows `title: i/n`.

| Keys | Action |
| --- | --- |
| `Ctrl+T` | New tab. |
| `Ctrl+W` | Close tab (last tab resets instead of closing). |
| `Ctrl+]` / `Ctrl+[` | Next / previous tab. |

## Navigate & search

| Keys | Action |
| --- | --- |
| `↑↓` / `Ctrl+N` | Move in results. |
| `←→` | Grid columns, or folder / item pane in the browser. |
| `Tab` | Switch Search / Results focus. |
| `Enter` | Open file, or enter folder. |
| `Backspace` | Empty query: go to parent. Otherwise edits the query. |
| `Ctrl+F` | Search the current folder. `Ctrl+Shift+F`: search everywhere. |
| `Ctrl+S` | Cycle sort. `Ctrl+M`: match mode. `Alt+T`: file class. |
| `Alt+←→` | Browser back / forward. |
| `Ctrl+L` | Open location (type a path). |

## View

| Keys | Action |
| --- | --- |
| `Space` / `F3` | Preview. |
| `+` / `−` | Grid density. `Ctrl++` / `Ctrl+−`: zoom. |
| `F4` | Dual-pane browser. `F6`: weight map. `F8`: settings. |
| `Alt+Z` | Zebra stripes. (Used to be `Ctrl+Z`; moved for undo.) |
| `F1` | This shortcut list. Clicking the footer opens it too. |

## System & mouse

| Keys | Action |
| --- | --- |
| `Ctrl+O` | Reveal in the system file manager. |
| `Ctrl+Y` | Copy focused path. `Ctrl+C` with Results focus copies path(s); `Ctrl+Shift+C` copies name(s). |
| `Ctrl+Shift+O` | Open focused item with a command entered in a prompt. |
| `Ctrl+B` / `Ctrl+Shift+B` | Pin the current folder / open pinned folders. |
| `F9` / `Ctrl+P` | Open Projects, Storage, Git, and Tasks workspace. |
| `F5` | Refresh the live browser and rebuild the Catalog. |
| `Ctrl+G` | Search everywhere. |
| `Ctrl+E` | Cycle open mode. |
| `Ctrl+C` without Results focus / `Ctrl+Q` / `Esc` | Quit (Esc also closes popups first). |
| Mouse drag | Drop a result into another app. Right-click or `F10`: actions menu (open, open with, preview, reveal, copy variants, mark, rename, batch rename, transfer, trash). |

## Prompts (rename / new folder / location)

`Enter` applies, `Esc` cancels, `Backspace` edits, `Ctrl+U` clears.
Names must be non-empty and contain no `/`.
