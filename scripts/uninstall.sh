#!/usr/bin/env bash
set -euo pipefail

echo "=== Uninstalling Verso Reader (pdfview) ==="

BIN_DIR="$HOME/.local/bin"
LIB_DIR="$HOME/.local/lib/pdfview"
APPS_DIR="$HOME/.local/share/applications"
ICON_BASE="$HOME/.local/share/icons/hicolor"

# 1. Remove binaries and symlinks
echo "Removing binary and symlinks..."
rm -f "$BIN_DIR/pdfview" "$BIN_DIR/verso-reader"

# 2. Remove engine libraries
echo "Removing engine libraries..."
rm -rf "$LIB_DIR"

# 3. Remove desktop entries
echo "Removing desktop menu entries..."
rm -f "$APPS_DIR/verso-reader.desktop" "$APPS_DIR/pdfview.desktop"
rm -f "$HOME/Desktop/verso-reader.desktop" "$HOME/Desktop/pdfview.desktop" 2>/dev/null || true

# 4. Remove icons
echo "Removing icons..."
for size in 32x32 48x48 64x64 128x128 256x256; do
    rm -f "$ICON_BASE/$size/apps/verso-reader.png" "$ICON_BASE/$size/apps/pdfview.png"
done

# 5. Update system databases
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APPS_DIR" 2>/dev/null || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$ICON_BASE" 2>/dev/null || true
fi

echo "✓ Verso Reader has been completely uninstalled from your system."
echo "Note: Your user settings and annotations in ~/.config/pdfview and ~/.local/share/pdfview were preserved."
echo "To also delete them, run: rm -rf ~/.config/pdfview ~/.local/share/pdfview"

