#!/usr/bin/env bash
# Build a stripped tarball for GitHub Releases / AUR qfind-bin and qfind-gtk-bin.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
ver="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)}"
arch="$(uname -m)"
stage="$root/target/dist/qfind-${ver}-${arch}"
rm -rf "$stage"
mkdir -p "$stage"

cargo build --release --manifest-path "$root/Cargo.toml" -p qfind -p qfind-tui -p qfind-gtk -p qfind-native
native_library="$root/target/release/libqfind_native.so"
if [ "$(uname -s)" = Darwin ]; then
  native_library="$root/target/release/libqfind_native.dylib"
fi
install -Dm755 "$root/target/release/qfind" "$stage/qfind"
install -Dm755 "$root/target/release/qfind-tui" "$stage/qfind-tui"
install -Dm755 "$root/target/release/qfind-gtk" "$stage/qfind-gtk"
install -Dm644 "$root/packaging/qfind.desktop" "$stage/qfind.desktop"
install -Dm644 "$root/assets/megaman.svg" "$stage/megaman.svg"
install -Dm644 "$root/packaging/nautilus/qfind.py" "$stage/qfind.py"
install -Dm644 "$root/LICENSE" "$stage/LICENSE"
if command -v cmake >/dev/null \
  && { pkg-config --exists Qt6Widgets 2>/dev/null || pkg-config --exists Qt5Widgets 2>/dev/null; }; then
  cmake -S "$root/packaging/kde" -B "$root/target/kde-build" -DCMAKE_BUILD_TYPE=Release \
    -DQFIND_NATIVE_LIBRARY="$native_library"
  cmake --build "$root/target/kde-build" -j
  install -Dm755 -s "$root/target/kde-build/qfind-qt" "$stage/qfind-qt"
  install -Dm755 "$native_library" "$stage/$(basename "$native_library")"
fi

out="$root/target/dist/qfind-${ver}-${arch}.tar.zst"
rm -f "$out"
tar -C "$stage" -c . | zstd -19 -o "$out"
echo "$out"
sha256sum "$out"
