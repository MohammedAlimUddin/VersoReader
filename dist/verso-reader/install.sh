#!/usr/bin/env bash
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
LIB_DIR="$HOME/.local/lib/pdfview"
APPS_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"

mkdir -p "$BIN_DIR" "$LIB_DIR" "$APPS_DIR" "$ICON_DIR"

echo "Installing VersoReader to $BIN_DIR/pdfview..."
cp "$DIR/pdfview" "$BIN_DIR/pdfview"
chmod +x "$BIN_DIR/pdfview"

echo "Installing Pdfium engine to $LIB_DIR/libpdfium.so..."
cp "$DIR/lib/libpdfium.so" "$LIB_DIR/libpdfium.so"

if [ -f "$DIR/assets/icon.png" ]; then
    cp "$DIR/assets/icon.png" "$ICON_DIR/pdfview.png"
fi

cat <<DESKTOP > "$APPS_DIR/pdfview.desktop"
[Desktop Entry]
Version=1.0
Type=Application
Name=VersoReader
GenericName=PDF Reader
Comment=Minimalistic, high-performance desktop PDF reader
Exec=$BIN_DIR/pdfview %U
Icon=pdfview
Terminal=false
Categories=Office;Viewer;Graphics;
MimeType=application/pdf;application/x-pdf;
Keywords=PDF;Viewer;Reader;Documents;
StartupWMClass=pdfview
DESKTOP
chmod +x "$APPS_DIR/pdfview.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APPS_DIR" 2>/dev/null || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
fi

echo "✓ VersoReader successfully installed!"
echo "You can now launch VersoReader from your application launcher or run 'pdfview <file.pdf>'."
