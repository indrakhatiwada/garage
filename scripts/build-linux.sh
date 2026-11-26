#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
if [ ! -x .venv/bin/python ]; then
    python3 -m venv .venv
fi
.venv/bin/python -c 'import PyInstaller, PIL, numpy, pymupdf' || {
    .venv/bin/python -m pip install -e engine pyinstaller
}
cargo build --release --locked -j "${CARGO_BUILD_JOBS:-2}"
rm -rf dist/garage-engine
.venv/bin/python -m PyInstaller --noconfirm --clean --distpath dist --workpath target/pyinstaller engine/garage-engine.spec
.venv/bin/python scripts/package-linux.py "$@"
