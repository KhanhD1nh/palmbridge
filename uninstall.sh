#!/usr/bin/env bash
# Remove Graft binaries and managed local services. Keeps credentials and build cache.
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"

if command -v graft >/dev/null 2>&1; then
  graft disable || true
elif command -v palmbridge >/dev/null 2>&1; then
  palmbridge disable || true # Legacy cleanup.
fi

rm -f "$PREFIX/bin/graft" "$PREFIX/bin/palmbridge"
echo "removed Graft from $PREFIX/bin"
echo "kept configuration and cache; remove ~/.config/graft and ~/.cache/graft manually to purge them"
