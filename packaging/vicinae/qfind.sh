#!/usr/bin/env bash
# @vicinae.schemaVersion 1
# @vicinae.title Qfind
# @vicinae.description Fuzzy filename search (Catalog)
# @vicinae.mode view
# @vicinae.argument1 {"type":"text","placeholder":"filename"}
set -euo pipefail
qfind --json --limit 40 "${1:-}"
