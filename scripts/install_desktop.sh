#!/usr/bin/env bash
set -euo pipefail

# Determine repository root directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "Building release binary..."
cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"

# Ensure libpdfium.so is available next to the binary
mkdir -p "$REPO_DIR/target/release/lib"
cp "$REPO_DIR/lib/libpdfium.so" "$REPO_DIR/target/release/lib/"

# Write application desktop entry
mkdir -p ~/.local/share/applications
cat <<EOF > ~/.local/share/applications/pdfview.desktop
[Desktop Entry]
Version=1.0
Type=Application
Name=pdfview
GenericName=PDF Reader
Comment=Minimalistic, high-speed PDF reader
Exec="$REPO_DIR/target/release/pdfview" %U
Path=$REPO_DIR
Icon=$REPO_DIR/assets/icon.svg
Terminal=false
Categories=Office;Viewer;
MimeType=application/pdf;application/x-pdf;
StartupWMClass=pdfview
EOF

# Copy to Desktop if ~/Desktop exists
if [ -d "$HOME/Desktop" ]; then
    cp ~/.local/share/applications/pdfview.desktop "$HOME/Desktop/pdfview.desktop"
    chmod +x "$HOME/Desktop/pdfview.desktop"
    if command -v gio >/dev/null 2>&1; then
        gio set "$HOME/Desktop/pdfview.desktop" metadata::trusted true || true
    fi
    echo "✓ Desktop shortcut created at ~/Desktop/pdfview.desktop"
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database ~/.local/share/applications/ 2>/dev/null || true
fi

echo "✓ Application installed to ~/.local/share/applications/pdfview.desktop"
echo "You can now launch pdfview directly from your Desktop, the Application Menu, or the Dock."

