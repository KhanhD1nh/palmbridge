#!/usr/bin/env bash
# Non-interactive install. Agents: curl -fsSL …/install.sh | bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
CACHE="${PALMBRIDGE_CACHE:-${HANDS_CACHE:-${GROK_HARNESS_CACHE:-$HOME/.cache/palmbridge}}}"
GROK_BUILD_URL="${GROK_BUILD_URL:-https://github.com/xai-org/grok-build.git}"
DEFAULT_GROK_BUILD_REF="$(tr -d '[:space:]' < "$REPO_ROOT/GROK_BUILD_REVISION")"
GROK_BUILD_REF="${GROK_BUILD_REF:-$DEFAULT_GROK_BUILD_REF}"
JOBS="${JOBS:-}"

mkdir -p "$CACHE" "$PREFIX/bin"
GROK_BUILD="$CACHE/grok-build"

if [[ -d "$GROK_BUILD/.git" ]]; then
  git -C "$GROK_BUILD" fetch --depth 1 origin "$GROK_BUILD_REF"
  git -C "$GROK_BUILD" checkout --force FETCH_HEAD
  git -C "$GROK_BUILD" clean -fdx
else
  mkdir -p "$GROK_BUILD"
  git -C "$GROK_BUILD" init
  git -C "$GROK_BUILD" remote add origin "$GROK_BUILD_URL"
  git -C "$GROK_BUILD" fetch --depth 1 origin "$GROK_BUILD_REF"
  git -C "$GROK_BUILD" checkout --detach FETCH_HEAD
fi

python3 "$REPO_ROOT/scripts/inject.py" "$REPO_ROOT" "$GROK_BUILD"

if ! command -v rustup >/dev/null 2>&1; then
  echo "rustup is required. Install: https://rustup.rs" >&2
  exit 1
fi

cd "$GROK_BUILD"
CARGO_ARGS=(build --release -p palmbridge)
if [[ -n "$JOBS" ]]; then
  CARGO_ARGS+=(-j "$JOBS")
fi
cargo "${CARGO_ARGS[@]}"

BIN="$GROK_BUILD/target/release/palmbridge"
install -m 0755 "$BIN" "$PREFIX/bin/palmbridge"

echo
echo "installed $PREFIX/bin/palmbridge"
"$PREFIX/bin/palmbridge" --version
echo

if [[ -n "${CONTROL_PLANE_API_KEY:-}" && -n "${CONTROL_PLANE_TUNNEL_ID:-}" ]]; then
  "$PREFIX/bin/palmbridge" setup || true
  echo "tunnel setup attempted (keys found in env)."
else
  echo "Next:"
  echo "  brew install openai/tools/tunnel-client   # once"
  echo "  cd /your/repo && palmbridge setup         # TTY checklist, no browser"
fi
echo
if ! command -v palmbridge >/dev/null 2>&1; then
  echo "Put $PREFIX/bin on PATH, e.g.:"
  echo "  export PATH=\"$PREFIX/bin:\$PATH\""
fi
