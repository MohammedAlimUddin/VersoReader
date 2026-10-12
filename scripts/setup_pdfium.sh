#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
LIB_DIR="$PROJECT_ROOT/lib"
TMP_DIR="$(mktemp -d)"

cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

echo "Downloading pdfium-linux-x64.tgz from bblanchon/pdfium-binaries..."
ARCHIVE_URL="https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-linux-x64.tgz"
curl -fsSL -L "$ARCHIVE_URL" -o "$TMP_DIR/pdfium-linux-x64.tgz"

mkdir -p "$LIB_DIR"
echo "Extracting libpdfium.so to $LIB_DIR/..."
tar -xzf "$TMP_DIR/pdfium-linux-x64.tgz" -C "$TMP_DIR"

if [ -f "$TMP_DIR/lib/libpdfium.so" ]; then
    cp "$TMP_DIR/lib/libpdfium.so" "$LIB_DIR/libpdfium.so"
elif [ -f "$TMP_DIR/libpdfium.so" ]; then
    cp "$TMP_DIR/libpdfium.so" "$LIB_DIR/libpdfium.so"
else
    echo "Error: libpdfium.so not found in archive" >&2
    exit 1
fi

chmod +x "$LIB_DIR/libpdfium.so"
echo "Pdfium setup complete: $LIB_DIR/libpdfium.so"

