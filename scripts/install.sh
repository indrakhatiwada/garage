#!/usr/bin/env bash
# Build Garage and install it into ~/.local.
#
#   ./scripts/install.sh              # release build + PyInstaller sidecar
#   PREFIX=/usr/local sudo -E ./scripts/install.sh
#
# Everything is built on this machine. Nothing is downloaded by the app.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
prefix="${PREFIX:-$HOME/.local}"
python="${PYTHON:-python3}"

cd "$root"

echo "==> Python environment"
if [ ! -x .venv/bin/python ]; then
    "$python" -m venv .venv
fi
.venv/bin/pip install --quiet --upgrade pip
.venv/bin/pip install --quiet -e engine pyinstaller

echo "==> Engine sidecar"
.venv/bin/pyinstaller --noconfirm --clean \
    --distpath "$root/dist" \
    --workpath "$root/target/pyinstaller" \
    "$root/engine/garage-engine.spec"

echo "==> Rust binary"
cargo build --release

echo "==> Install into $prefix"
mkdir -p "$prefix/bin" \
    "$prefix/share/applications" \
    "$prefix/share/icons/hicolor/scalable/apps" \
    "$prefix/share/icons/hicolor/128x128/apps" \
    "$prefix/share/icons/hicolor/512x512/apps"
install -m 0755 target/release/garage "$prefix/bin/garage"
install -m 0755 dist/garage-engine "$prefix/bin/garage-engine"
install -m 0644 assets/garage.desktop "$prefix/share/applications/garage.desktop"
install -m 0644 assets/brand/garage-app.svg \
    "$prefix/share/icons/hicolor/scalable/apps/garage.svg"
install -m 0644 assets/brand/garage-128.png \
    "$prefix/share/icons/hicolor/128x128/apps/garage.png"
install -m 0644 assets/brand/garage-512.png \
    "$prefix/share/icons/hicolor/512x512/apps/garage.png"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$prefix/share/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$prefix/share/icons/hicolor" 2>/dev/null || true
fi

echo
echo "Installed:"
echo "  $prefix/bin/garage"
echo "  $prefix/bin/garage-engine"
echo
echo "Optional external tools:"
for tool in tesseract gs; do
    if command -v "$tool" >/dev/null 2>&1; then
        echo "  $tool: found"
    else
        echo "  $tool: not installed"
    fi
done
echo
echo "Nepali OCR: install tesseract-data-nep (Arch) or tesseract-ocr-nep (Debian)."
