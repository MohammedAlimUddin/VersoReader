#!/usr/bin/env bash
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
LIB_DIR="$HOME/.local/lib/pdfview"
APPS_DIR="$HOME/.local/share/applications"
ICON_BASE="$HOME/.local/share/icons/hicolor"

mkdir -p "$BIN_DIR" "$LIB_DIR" "$APPS_DIR"
for size in 32x32 48x48 64x64 128x128 256x256; do
    mkdir -p "$ICON_BASE/$size/apps"
done

echo "Installing Verso Reader to $BIN_DIR/pdfview..."
cp "$DIR/pdfview" "$BIN_DIR/pdfview"
chmod +x "$BIN_DIR/pdfview"
ln -sf "$BIN_DIR/pdfview" "$BIN_DIR/verso-reader"

echo "Installing Pdfium engine to $LIB_DIR/libpdfium.so..."
cp "$DIR/lib/libpdfium.so" "$LIB_DIR/libpdfium.so"

[ -f "$DIR/assets/icon.png" ] && cp "$DIR/assets/icon.png" "$ICON_BASE/256x256/apps/verso-reader.png" && cp "$DIR/assets/icon.png" "$ICON_BASE/256x256/apps/pdfview.png"
[ -f "$DIR/assets/icon-128.png" ] && cp "$DIR/assets/icon-128.png" "$ICON_BASE/128x128/apps/verso-reader.png"
[ -f "$DIR/assets/icon-64.png" ] && cp "$DIR/assets/icon-64.png" "$ICON_BASE/64x64/apps/verso-reader.png"
[ -f "$DIR/assets/icon-48.png" ] && cp "$DIR/assets/icon-48.png" "$ICON_BASE/48x48/apps/verso-reader.png"
[ -f "$DIR/assets/icon-32.png" ] && cp "$DIR/assets/icon-32.png" "$ICON_BASE/32x32/apps/verso-reader.png"

cat <<DESKTOP > "$APPS_DIR/verso-reader.desktop"
[Desktop Entry]
Version=1.0
Type=Application
Name=Verso Reader
GenericName=PDF Reader
Comment=Modern, lightweight, high-performance desktop PDF reader
Exec=$BIN_DIR/pdfview %U
Icon=verso-reader
Terminal=false
Categories=Office;Viewer;
MimeType=application/pdf;application/x-pdf;
Keywords=PDF;Viewer;Reader;Documents;Verso;
StartupWMClass=verso-reader
DESKTOP

# Remove legacy desktop file if present to prevent duplicate launcher icons
rm -f "$APPS_DIR/pdfview.desktop" "$HOME/Desktop/pdfview.desktop" 2>/dev/null || true
chmod +x "$APPS_DIR/verso-reader.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APPS_DIR" 2>/dev/null || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
fi

echo "✓ Verso Reader successfully installed!"
echo "You can now launch Verso Reader from your application launcher or run 'verso-reader <file.pdf>'."
