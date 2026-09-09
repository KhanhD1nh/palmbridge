#!/usr/bin/env bash
# Palmbridge — install prebuilt binaries from GitHub Releases
# No Rust, Git, or build tools required.
set -euo pipefail

REPO="KhanhD1nh/palmbridge"
TC_VERSION="0.0.14"
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
CHECKSUM_URL=$(curl -fsSL "$API_URL" | grep '"browser_download_url"' | grep 'SHA256SUMS' | head -1 | cut -d'"' -f4)
if [ -z "$DOWNLOAD_URL" ]; then
  echo "No asset found for $ASSET in release $VERSION"
  exit 1
fi
if [ -z "$CHECKSUM_URL" ]; then
  echo "Release $VERSION has no SHA256SUMS; refusing unverified install"
  exit 1
fi

echo "Downloading $ASSET..."
PB_TMP=$(mktemp)
SUM_TMP=$(mktemp)
trap 'rm -f "$PB_TMP" "$SUM_TMP"' EXIT
curl -fsSL "$DOWNLOAD_URL" -o "$PB_TMP"
curl -fsSL "$CHECKSUM_URL" -o "$SUM_TMP"
EXPECTED=$(grep "  $ASSET$" "$SUM_TMP" | head -1 | cut -d' ' -f1)
if [ -z "$EXPECTED" ]; then
  echo "No checksum found for $ASSET"
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL=$(sha256sum "$PB_TMP" | cut -d' ' -f1)
else
  ACTUAL=$(shasum -a 256 "$PB_TMP" | cut -d' ' -f1)
fi
if [ "$ACTUAL" != "$EXPECTED" ]; then
  echo "SHA-256 mismatch for $ASSET"
  exit 1
fi
mv "$PB_TMP" "$BIN/palmbridge"
chmod +x "$BIN/palmbridge"
"$BIN/palmbridge" --version

# --- Install/download tunnel-client ---
# OpenAI documents Homebrew as the supported macOS install path; directly
# downloaded release ZIPs are not notarized. Linux uses the signed-release
# checksum manifest and installs the verified binary into the Palmbridge prefix.
if [ "$OS" = "darwin" ]; then
  if ! command -v tunnel-client >/dev/null 2>&1; then
    if ! command -v brew >/dev/null 2>&1; then
      echo "tunnel-client on macOS requires Homebrew: brew install openai/tools/tunnel-client"
      exit 1
    fi
    brew install openai/tools/tunnel-client
  fi
else
  TC_OS="$OS"
  TC_ARCH="amd64"
  [ "$ARCH_TAG" = "aarch64" ] && TC_ARCH="arm64"
  TC_NAME="tunnel-client-v$TC_VERSION-${TC_OS}-${TC_ARCH}.zip"
  TC_BASE="https://github.com/openai/tunnel-client/releases/download/v$TC_VERSION"
  TC_URL="$TC_BASE/$TC_NAME"
  TC_TMP=$(mktemp -d)
  echo "Downloading tunnel-client v$TC_VERSION..."
  curl -fsSL "$TC_URL" -o "$TC_TMP/$TC_NAME"
  curl -fsSL "$TC_BASE/SHA256SUMS.txt" -o "$TC_TMP/SHA256SUMS.txt"
  TC_EXPECTED=$(grep "  $TC_NAME$" "$TC_TMP/SHA256SUMS.txt" | head -1 | cut -d' ' -f1)
  if [ -z "$TC_EXPECTED" ]; then
    echo "No official checksum found for $TC_NAME"
    exit 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    TC_ACTUAL=$(sha256sum "$TC_TMP/$TC_NAME" | cut -d' ' -f1)
  else
    TC_ACTUAL=$(shasum -a 256 "$TC_TMP/$TC_NAME" | cut -d' ' -f1)
  fi
  if [ "$TC_ACTUAL" != "$TC_EXPECTED" ]; then
    echo "SHA-256 mismatch for $TC_NAME"
    exit 1
  fi
  if ! command -v unzip >/dev/null 2>&1; then
    echo "Need unzip to extract tunnel-client"
    exit 1
  fi
  unzip -q "$TC_TMP/$TC_NAME" -d "$TC_TMP/extract"
  cp "$TC_TMP/extract/tunnel-client" "$BIN/tunnel-client"
  chmod +x "$BIN/tunnel-client"
  rm -rf "$TC_TMP"
fi

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
