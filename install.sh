#!/bin/sh
set -eu

repo="DerpcatMusic/qfind"
install_dir="${QFIND_INSTALL_DIR:-$HOME/.local/bin}"
tag="${QFIND_VERSION:-}"
if [ -z "$tag" ]; then
  tag="$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
    | sed -n 's/.*"tag_name": "\([^"]*\)".*/\1/p' | head -n 1)"
fi
[ -n "$tag" ] || { echo "Could not resolve the latest Qfind release." >&2; exit 1; }
case "$tag" in v*) ;; *) tag="v$tag" ;; esac
version="${tag#v}"

case "$(uname -s)" in
  Linux) platform=linux ;;
  Darwin) platform=macos ;;
  *) echo "Use install.ps1 on Windows." >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 ;;
  arm64) arch=arm64 ;;
  aarch64) arch=aarch64 ;;
  *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
base="https://github.com/$repo/releases/download/$tag"
asset="qfind-${version}-${platform}-${arch}.tar.gz"
if curl -fLsS "$base/$asset" -o "$tmp/qfind.tar.gz" 2>/dev/null; then
  tar -xzf "$tmp/qfind.tar.gz" -C "$tmp"
elif [ "$platform-$arch" = "linux-x86_64" ] \
  && curl -fLsS "$base/qfind-${version}-x86_64.tar.zst" -o "$tmp/qfind.tar.zst"; then
  tar --zstd -xf "$tmp/qfind.tar.zst" -C "$tmp"
else
  echo "No Qfind $tag build for $platform-$arch." >&2
  exit 1
fi

qfind="$(find "$tmp" -type f -name qfind -print | head -n 1)"
qfind_tui="$(find "$tmp" -type f -name qfind-tui -print | head -n 1)"
[ -n "$qfind" ] && [ -n "$qfind_tui" ] || { echo "Release archive is incomplete." >&2; exit 1; }
mkdir -p "$install_dir"
install -m755 "$qfind" "$install_dir/qfind"
install -m755 "$qfind_tui" "$install_dir/qfind-tui"

echo "Installed Qfind $version in $install_dir"
case ":$PATH:" in *":$install_dir:"*) ;; *) echo "Add $install_dir to PATH." ;; esac
echo "Run: qfind index && qfind"
