#!/usr/bin/env bash
# Palmbridge — install prebuilt binaries from GitHub Releases
# No Rust, Git, or build tools required.
set -euo pipefail

REPO="KhanhD1nh/palmbridge"
TC_VERSION="0.0.13"
VERSION="${1:-latest}"
PREFIX="${2:-$HOME/.local}"
BIN="$PREFIX/bin"
mkdir -p "$BIN"

OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$ARCH" in
  x86_64)  ARCH_TAG="x86_64" ;;
  aarch64|arm64) ARCH_TAG="aarch64" ;;
  *) echo "Unsupported arch: $ARCH"; exit 1 ;;
esac

ASSET="palmbridge-${OS}-${ARCH_TAG}"
if [ "$OS" = "darwin" ] && [ "$ARCH_TAG" = "x86_64" ]; then
  echo "No x86_64 macOS build (only aarch64). Build from source."
  exit 1
fi

# --- Download palmbridge ---
if [ "$VERSION" = "latest" ]; then
  API_URL="https://api.github.com/repos/$REPO/releases/latest"
else
  API_URL="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
fi

echo "Fetching release info..."
DOWNLOAD_URL=$(curl -fsSL "$API_URL" | grep '"browser_download_url"' | grep "$ASSET" | head -1 | cut -d'"' -f4)
if [ -z "$DOWNLOAD_URL" ]; then
  echo "No asset found for $ASSET in release $VERSION"
  exit 1
fi

echo "Downloading $ASSET..."
curl -fsSL "$DOWNLOAD_URL" -o "$BIN/palmbridge"
chmod +x "$BIN/palmbridge"
"$BIN/palmbridge" --version

# --- Download tunnel-client ---
TC_OS="$OS"
TC_ARCH="amd64"
[ "$ARCH_TAG" = "aarch64" ] && TC_ARCH="arm64"
TC_URL="https://persistent.oaistatic.com/tunnel-client/v$TC_VERSION/tunnel-client-v$TC_VERSION-${TC_OS}-${TC_ARCH}.tar.gz"
TC_TMP=$(mktemp -d)
echo "Downloading tunnel-client v$TC_VERSION..."
curl -fsSL "$TC_URL" | tar xz -C "$TC_TMP"
cp "$TC_TMP/tunnel-client" "$BIN/tunnel-client" 2>/dev/null || cp "$TC_TMP"/*/tunnel-client "$BIN/tunnel-client"
chmod +x "$BIN/tunnel-client"
rm -rf "$TC_TMP"

# --- PATH hint ---
if ! echo "$PATH" | grep -q "$BIN"; then
  SHELL_RC=""
  case "$(basename "${SHELL:-}")" in
    zsh)  SHELL_RC="$HOME/.zshrc" ;;
    bash) SHELL_RC="$HOME/.bashrc" ;;
  esac
  if [ -n "$SHELL_RC" ]; then
    echo "export PATH=\"\$PATH:$BIN\"" >> "$SHELL_RC"
    echo "Added $BIN to PATH in $SHELL_RC"
  else
    echo "Add this to your shell profile: export PATH=\"\$PATH:$BIN\""
  fi
  export PATH="$PATH:$BIN"
fi

echo ""
echo "Installed successfully. Run:"
echo "  palmbridge setup"

</content>