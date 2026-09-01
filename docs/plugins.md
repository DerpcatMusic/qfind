# Install Vicinae and Nautilus

Both plugins talk to the Qfind CLI (`qfind --json`). Install the app first:

```bash
git clone https://github.com/DerpcatMusic/qfind.git
cd qfind
./packaging/install.sh
```

That builds `qfind` into `~/.local/bin` (and `qfind-gtk` if gtk4 is available) **and** installs both plugins. Nautilus needs `qfind-gtk` on `PATH`. If the binaries are already installed, plugins only:

```bash
./packaging/install-plugins.sh
```

## Nautilus / Files

**What you get**

- **Ctrl+F** in a folder opens an instant recursive Qfind search scoped to that directory.
- Right-click a folder or the background → **Search with Qfind**.

Nautilus 43+ has no public hook to replace the in-window search box (Tracker/LocalSearch is compiled in). Ctrl+F is captured on the Files window and handed to `qfind-gtk --here`.

**Manual install**

```bash
# Arch: sudo pacman -S nautilus-python
# Debian/Ubuntu: sudo apt install python3-nautilus nautilus

install -Dm644 packaging/nautilus/qfind.py \
  ~/.local/share/nautilus-python/extensions/qfind.py
nautilus -q
```

Open Files again. `qfind-gtk` must be on `PATH` (`~/.local/bin`).

## Vicinae

**What you get**

A launcher command named **Qfind**. Type a filename; `.wav` / `.png` / `.exe` filter by extension.

**From this repo (supported)**

Needs Node.js (npm). `vici build` writes the extension into Vicinae’s user dir:

```bash
cd packaging/vicinae
npm install
npx vici build
```

Output: `~/.local/share/vicinae/extensions/qfind`. Reopen Vicinae, run **Qfind**.

**Script fallback** (no Node)

```bash
install -Dm755 packaging/vicinae/qfind.sh ~/.local/share/qfind/vicinae/qfind.sh
```

Point a Vicinae script command at that file if you do not want the TS extension.

**From another machine**

```bash
git clone https://github.com/DerpcatMusic/qfind.git
cd qfind
./packaging/install-plugins.sh
```

`qfind` must be on `PATH` so the extension can spawn `qfind --json --limit 32 --files …`.
