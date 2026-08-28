#!/usr/bin/env bash
# Install only the Nautilus + Vicinae adapters. Needs `qfind` already on PATH
# (run packaging/install.sh first, or cargo install the CLI).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"

nautilus_dst="$HOME/.local/share/nautilus-python/extensions/qfind.py"
install -Dm644 "$root/packaging/nautilus/qfind.py" "$nautilus_dst"
echo "Nautilus: $nautilus_dst"
echo "  restart Files:  nautilus -q"
echo "  then Ctrl+F in a folder, or right-click → Search with Qfind"

if ! command -v qfind >/dev/null; then
  echo "warning: qfind is not on PATH. Vicinae needs the CLI (./packaging/install.sh)." >&2
fi

if command -v npm >/dev/null; then
  (
    cd "$root/packaging/vicinae"
    npm install --no-fund --no-audit >/dev/null
    npx --yes vici build
  )
  echo "Vicinae: ~/.local/share/vicinae/extensions/qfind"
  echo "  reopen Vicinae, then run the Qfind command"
else
  echo "Vicinae skipped (npm not found). Install Node, then rerun this script." >&2
fi
