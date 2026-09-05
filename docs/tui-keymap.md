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
| `↑↓` / `Ctrl+N` / `Ctrl+P` | Move in results. |
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
| `Ctrl+Y` | Copy focused path. `Ctrl+E`: open mode. |
| `Ctrl+C` / `Ctrl+Q` / `Esc` | Quit (Esc also closes popups first). |
| Mouse drag | Drop a result into another app. Right-click: actions menu (open, preview, copy path, show in files, mark, rename, trash). |

## Prompts (rename / new folder / location)

`Enter` applies, `Esc` cancels, `Backspace` edits, `Ctrl+U` clears.
Names must be non-empty and contain no `/`.
