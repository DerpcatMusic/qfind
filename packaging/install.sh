#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
prefix="${PREFIX:-$HOME/.local}"
cargo build --release --manifest-path "$root/Cargo.toml" -p qfind -p qfind-tui -p qfind-gtk
install -Dm755 "$root/target/release/qfind" "$prefix/bin/qfind"
install -Dm755 "$root/target/release/qfind-tui" "$prefix/bin/qfind-tui"
install -Dm755 "$root/target/release/qfind-gtk" "$prefix/bin/qfind-gtk"
install -Dm644 "$root/packaging/qfind.desktop" "$prefix/share/applications/qfind.desktop"
install -Dm644 "$root/packaging/nautilus/qfind.py" \
  "$HOME/.local/share/nautilus-python/extensions/qfind.py"
install -Dm755 "$root/packaging/vicinae/qfind.sh" "$prefix/share/qfind/vicinae/qfind.sh"
if command -v npm >/dev/null; then
  (
    cd "$root/packaging/vicinae"
    npm install --no-fund --no-audit >/dev/null
    npx --yes vici build
  )
  echo "installed Vicinae extension: ~/.local/share/vicinae/extensions/qfind"
fi
if command -v cmake >/dev/null && pkg-config --exists Qt6Widgets 2>/dev/null; then
  cmake -S "$root/packaging/kde" -B "$root/target/kde-build" -DCMAKE_BUILD_TYPE=Release
  cmake --build "$root/target/kde-build" -j
  install -Dm755 "$root/target/kde-build/qfind-qt" "$prefix/bin/qfind-qt"
  echo "installed $prefix/bin/qfind-qt (Breeze/Qt)"
fi
if command -v update-desktop-database >/dev/null; then
  update-desktop-database "$prefix/share/applications" || true
fi
echo "installed $prefix/bin/qfind"
echo "nautilus plugin: ~/.local/share/nautilus-python/extensions/qfind.py"
echo "vicinae script: $prefix/share/qfind/vicinae/qfind.sh"
echo "launcher: $prefix/share/applications/qfind.desktop"
