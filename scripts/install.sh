#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== Installing VersoReader (pdfview) ==="

# 1. Build release binary if not present
if [ ! -f "$REPO_DIR/target/release/pdfview" ]; then
    echo "Building release binary..."
    cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"
fi

# 2. Setup install directories
BIN_DIR="$HOME/.local/bin"
LIB_DIR="$HOME/.local/lib/pdfview"
APPS_DIR="$HOME/.local/share/applications"
ICON_BASE="$HOME/.local/share/icons/hicolor"
MIME_DIR="$HOME/.local/share/mime/packages"

mkdir -p "$BIN_DIR" "$LIB_DIR" "$APPS_DIR" "$MIME_DIR"
for size in 32x32 48x48 64x64 128x128 256x256; do
    mkdir -p "$ICON_BASE/$size/apps"
done

# 3. Copy binary and library
echo "Installing binary to $BIN_DIR/pdfview..."
cp "$REPO_DIR/target/release/pdfview" "$BIN_DIR/pdfview"
chmod +x "$BIN_DIR/pdfview"
ln -sf "$BIN_DIR/pdfview" "$BIN_DIR/verso-reader"

if [ -f "$REPO_DIR/lib/libpdfium.so" ]; then
    echo "Installing Pdfium engine to $LIB_DIR/libpdfium.so..."
    cp "$REPO_DIR/lib/libpdfium.so" "$LIB_DIR/libpdfium.so"
elif [ -f "$REPO_DIR/target/release/lib/libpdfium.so" ]; then
    cp "$REPO_DIR/target/release/lib/libpdfium.so" "$LIB_DIR/libpdfium.so"
fi

# 4. Copy multi-scale icons
[ -f "$REPO_DIR/assets/icon.png" ] && cp "$REPO_DIR/assets/icon.png" "$ICON_BASE/256x256/apps/verso-reader.png" && cp "$REPO_DIR/assets/icon.png" "$ICON_BASE/256x256/apps/pdfview.png"
[ -f "$REPO_DIR/assets/icon-128.png" ] && cp "$REPO_DIR/assets/icon-128.png" "$ICON_BASE/128x128/apps/verso-reader.png"
[ -f "$REPO_DIR/assets/icon-64.png" ] && cp "$REPO_DIR/assets/icon-64.png" "$ICON_BASE/64x64/apps/verso-reader.png"
[ -f "$REPO_DIR/assets/icon-48.png" ] && cp "$REPO_DIR/assets/icon-48.png" "$ICON_BASE/48x48/apps/verso-reader.png"
[ -f "$REPO_DIR/assets/icon-32.png" ] && cp "$REPO_DIR/assets/icon-32.png" "$ICON_BASE/32x32/apps/verso-reader.png"

# 5. Install desktop entry
cat <<EOF > "$APPS_DIR/verso-reader.desktop"
[Desktop Entry]
Version=1.0
Type=Application
Name=Verso Reader
GenericName=PDF Reader
Comment=Modern, lightweight, high-performance desktop PDF reader
Exec=$BIN_DIR/pdfview %U
Icon=verso-reader
Terminal=false
Categories=Office;Viewer;Graphics;
MimeType=application/pdf;application/x-pdf;
Keywords=PDF;Viewer;Reader;Documents;Verso;
StartupWMClass=verso-reader
EOF

cp "$APPS_DIR/verso-reader.desktop" "$APPS_DIR/pdfview.desktop"
chmod +x "$APPS_DIR/verso-reader.desktop" "$APPS_DIR/pdfview.desktop"

# 6. Update desktop & icon databases
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APPS_DIR" 2>/dev/null || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
fi

echo "=== Verso Reader installed successfully! ==="
echo "• Binary:  $BIN_DIR/pdfview (also symlinked as $BIN_DIR/verso-reader)"
echo "• Engine:  $LIB_DIR/libpdfium.so"
echo "• Launcher: $APPS_DIR/verso-reader.desktop"
echo ""
echo "You can now launch Verso Reader from your application launcher or run 'verso-reader <file.pdf>'."

