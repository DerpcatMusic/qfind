#!/usr/bin/env bash
# Build a stripped tarball for GitHub Releases / AUR qfind-bin and qfind-gtk-bin.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
ver="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)}"
arch="$(uname -m)"
stage="$root/target/dist/qfind-${ver}-${arch}"
rm -rf "$stage"
mkdir -p "$stage"

cargo build --release --manifest-path "$root/Cargo.toml" -p qfind -p qfind-tui -p qfind-gtk
install -Dm755 "$root/target/release/qfind" "$stage/qfind"
install -Dm755 "$root/target/release/qfind-tui" "$stage/qfind-tui"
install -Dm755 "$root/target/release/qfind-gtk" "$stage/qfind-gtk"
install -Dm644 "$root/packaging/qfind.desktop" "$stage/qfind.desktop"
install -Dm644 "$root/packaging/nautilus/qfind.py" "$stage/qfind.py"
install -Dm644 "$root/LICENSE" "$stage/LICENSE"
if command -v cmake >/dev/null && pkg-config --exists Qt6Widgets 2>/dev/null; then
  cmake -S "$root/packaging/kde" -B "$root/target/kde-build" -DCMAKE_BUILD_TYPE=Release
  cmake --build "$root/target/kde-build" -j
  install -Dm755 "$root/target/kde-build/qfind-qt" "$stage/qfind-qt"
fi

out="$root/target/dist/qfind-${ver}-${arch}.tar.zst"
tar -C "$stage" -c . | zstd -19 -o "$out"
echo "$out"
sha256sum "$out"
