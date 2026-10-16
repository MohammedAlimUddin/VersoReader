#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== Packaging VersoReader for Distribution ==="

# 1. Build release binary
echo "Building optimized release binary..."
cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"

# 2. Prepare staging directory
DIST_DIR="$REPO_DIR/dist"
STAGE_DIR="$DIST_DIR/verso-reader"
rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR/lib" "$STAGE_DIR/assets"

# 3. Copy binary and libraries
echo "Copying binary and libraries..."
cp "$REPO_DIR/target/release/pdfview" "$STAGE_DIR/pdfview"
chmod +x "$STAGE_DIR/pdfview"

if [ -f "$REPO_DIR/lib/libpdfium.so" ]; then
    cp "$REPO_DIR/lib/libpdfium.so" "$STAGE_DIR/lib/libpdfium.so"
elif [ -f "$REPO_DIR/target/release/lib/libpdfium.so" ]; then
    cp "$REPO_DIR/target/release/lib/libpdfium.so" "$STAGE_DIR/lib/libpdfium.so"
else
    echo "Error: libpdfium.so not found! Run scripts/setup_pdfium.sh first."
    exit 1
fi

# 4. Copy assets
for img in icon.png icon-128.png icon-64.png icon-48.png icon-32.png; do
    if [ -f "$REPO_DIR/assets/$img" ]; then
        cp "$REPO_DIR/assets/$img" "$STAGE_DIR/assets/$img"
    fi
done
if [ -f "$REPO_DIR/assets/pdfview.desktop" ]; then
    cp "$REPO_DIR/assets/pdfview.desktop" "$STAGE_DIR/assets/verso-reader.desktop"
    cp "$REPO_DIR/assets/pdfview.desktop" "$STAGE_DIR/assets/pdfview.desktop"
fi

# 5. Create portable runner script
cat <<'EOF' > "$STAGE_DIR/run.sh"
#!/usr/bin/env bash
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export LD_LIBRARY_PATH="$DIR/lib:$DIR:${LD_LIBRARY_PATH:-}"
exec "$DIR/pdfview" "$@"
EOF
chmod +x "$STAGE_DIR/run.sh"

# 6. Create portable installer script for the end-user
cat <<'EOF' > "$STAGE_DIR/install.sh"
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
Categories=Office;Viewer;Graphics;
MimeType=application/pdf;application/x-pdf;
Keywords=PDF;Viewer;Reader;Documents;Verso;
StartupWMClass=verso-reader
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
EOF
chmod +x "$STAGE_DIR/install.sh"

# 7. Create README.txt in the bundle
cat <<EOF > "$STAGE_DIR/README.txt"
VersoReader — Minimalistic, Fast, Low-RAM PDF Reader for Linux
============================================================

Option 1: Run Portably (No installation required)
------------------------------------------------
Simply run:
    ./run.sh [document.pdf]
or double-click run.sh in your file manager.

Option 2: Install to System Menu
-------------------------------
To add VersoReader to your desktop application menu and file manager "Open With" list:
    ./install.sh

Shortcuts:
- Ctrl+O / Ctrl+L : Open file / focus path bar
- Ctrl+T / Ctrl+W : New tab / Close tab
- Ctrl+Tab        : Switch tabs
- Ctrl+Scroll     : Zoom around cursor (10% - 800%)
- Trackpad Pinch  : Smooth native pinch-to-zoom
- Ctrl+0          : Fit to page width
- Ctrl+F          : Find text in document
- H               : Toggle highlighter tool
- T               : Toggle text box annotation tool
- E               : Toggle eraser tool
- Ctrl+S          : Save document with embedded annotations
- F11             : Toggle fullscreen mode
EOF

# 8. Create archive
ARCHIVE_NAME="verso-reader-linux-x86_64.tar.gz"
echo "Creating $ARCHIVE_NAME..."
cd "$DIST_DIR"
tar -czf "$ARCHIVE_NAME" "verso-reader"

# 9. Compute SHA256 checksum
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$ARCHIVE_NAME" > "$ARCHIVE_NAME.sha256"
fi

echo "=== Packaging complete! ==="
echo "Distribution archive: $DIST_DIR/$ARCHIVE_NAME"
ls -lh "$DIST_DIR/$ARCHIVE_NAME"

