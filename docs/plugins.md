# Plugin plan — Vicinae and Nautilus

Catalog stays the only deep module. Plugins are adapters that Query it. They do not own Drag, Rebuild, or Surface.

## Seams

| Adapter | Talks to Catalog via | Why this seam |
|---|---|---|
| CLI `qfind --json` | `Catalog::open` + `search_with` | mmap Catalog, no daemon |
| Vicinae extension | spawn `qfind --json --limit 40 …` | Vicinae is React/TS over a C++ core; it already shells out |
| Nautilus | **cannot** replace Files’ in-window search | Nautilus search is Tracker/LocalSearch inside the Files binary |
| GNOME Shell | `org.gnome.Shell.SearchProvider2` | Overview search is a public D-Bus interface |

Two adapters (GTK + TUI) already justified the Catalog seam. CLI JSON is the third. A plugin that opens the snapshot itself would be a fourth — only worth it if spawn latency shows up.

## Vicinae (do this first)

Vicinae extensions are TypeScript/React (`@vicinae/api`), Raycast-shaped. File search in the launcher should:

1. Debounce ~50ms (same as GTK).
2. `qfind --json --limit 40 -- <query>`
3. Map `{name, path, dir}` → `List.Item` with `Action.Open` / copy path.
4. Empty Query: show Catalog stats (`qfind` with no args, non-tty).

Fallback if someone does not want TS: a **script command** that prints paths. Weaker UX, same Catalog.

Stub: `packaging/vicinae/`.

Do not embed nucleo in the extension. The Catalog already has the SIMD prefilter.

## Nautilus / Files (honest)

**Overriding the search box inside Files is not a public interface.** Nautilus 43+ search is Tracker 3 / LocalSearch, compiled in. nautilus-python exposes MenuProvider, InfoProvider, Properties — not “replace search”.

What we *can* ship:

1. **MenuProvider** — “Search with Qfind” on a folder / background. Spawns `qfind-gtk`. Stub: `packaging/nautilus/qfind.py`.
2. **GNOME Shell SearchProvider2** — Overview typing. New small binary `qfind-search-provider` that mmap-opens the snapshot and answers `GetInitialResultSet`. This is the real “system search uses our fuzzy” path, and it is *not* Nautilus.
3. **Fork/patch Files** — only if we later accept a distro overlay. Out of scope.

Hyprland users often do not use GNOME Shell; (1) + Vicinae cover them. (2) is for GNOME sessions.

## Settings

`$XDG_CONFIG_HOME/qfind/config.toml` (GTK Settings window, Reset to default):

- `exclude` — extra junk names/globs on Rebuild
- `include` — Mounts to Rebuild (empty = discover disks)
- `zoom` / `spacing` — compactness and extra row padding
- `preview` — `hovered` (default) or `selected` for Space

CLI `qfind index` reads the same Config.

## Order

1. Keep `qfind --json` stable (this tree).
2. Vicinae List extension calling that CLI.
3. Nautilus context-menu adapter.
4. Optional SearchProvider2 if we care about GNOME Overview.

No daemon until two processes need a live Catalog at once. Plugins open the snapshot; GTK already polls mtime.
