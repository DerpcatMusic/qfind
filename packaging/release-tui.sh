#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
ver="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)}"

case "$(uname -s)" in
  Linux) platform=linux ;;
  Darwin) platform=macos ;;
  *) echo "unsupported platform: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 ;;
  arm64) arch=arm64 ;;
  aarch64) arch=aarch64 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

name="qfind-${ver}-${platform}-${arch}"
stage="$root/target/dist/$name"
rm -rf "$stage"
mkdir -p "$stage"
cargo build --release --manifest-path "$root/Cargo.toml" -p qfind -p qfind-tui
install -m755 "$root/target/release/qfind" "$stage/qfind"
cp "$stage/qfind" "$stage/qfind-cli"
install -m755 "$root/target/release/qfind-tui" "$stage/qfind-tui"
install -m644 "$root/LICENSE" "$stage/LICENSE"
tar -C "$root/target/dist" -czf "$root/target/dist/$name.tar.gz" "$name"
shasum -a 256 "$root/target/dist/$name.tar.gz" 2>/dev/null \
  || sha256sum "$root/target/dist/$name.tar.gz"
