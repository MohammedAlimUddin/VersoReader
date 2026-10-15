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
ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"
MIME_DIR="$HOME/.local/share/mime/packages"

mkdir -p "$BIN_DIR" "$LIB_DIR" "$APPS_DIR" "$ICON_DIR" "$MIME_DIR"

# 3. Copy binary and library
echo "Installing binary to $BIN_DIR/pdfview..."
cp "$REPO_DIR/target/release/pdfview" "$BIN_DIR/pdfview"
chmod +x "$BIN_DIR/pdfview"

if [ -f "$REPO_DIR/lib/libpdfium.so" ]; then
    echo "Installing Pdfium engine to $LIB_DIR/libpdfium.so..."
    cp "$REPO_DIR/lib/libpdfium.so" "$LIB_DIR/libpdfium.so"
elif [ -f "$REPO_DIR/target/release/lib/libpdfium.so" ]; then
    cp "$REPO_DIR/target/release/lib/libpdfium.so" "$LIB_DIR/libpdfium.so"
fi

# 4. Copy icons
if [ -f "$REPO_DIR/assets/icon.png" ]; then
    cp "$REPO_DIR/assets/icon.png" "$ICON_DIR/pdfview.png"
fi

# 5. Install desktop entry
cat <<EOF > "$APPS_DIR/pdfview.desktop"
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
EOF

chmod +x "$APPS_DIR/pdfview.desktop"

# 6. Update desktop & icon databases
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APPS_DIR" 2>/dev/null || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
fi

echo "=== VersoReader installed successfully! ==="
echo "• Binary:  $BIN_DIR/pdfview"
echo "• Engine:  $LIB_DIR/libpdfium.so"
echo "• Launcher: $APPS_DIR/pdfview.desktop"
echo ""
echo "You can now launch VersoReader from your application launcher or run 'pdfview <file.pdf>'."

