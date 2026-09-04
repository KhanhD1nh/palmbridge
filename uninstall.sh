#!/usr/bin/env bash
# Remove Palmbridge binaries and managed local services. Keeps credentials and build cache.
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"

if command -v palmbridge >/dev/null 2>&1; then
  palmbridge disable || true
fi

rm -f "$PREFIX/bin/palmbridge"
echo "removed $PREFIX/bin/palmbridge"
echo "kept configuration and cache; remove ~/.config/palmbridge and ~/.cache/palmbridge manually to purge them"
