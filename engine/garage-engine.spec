# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller recipe for the Garage engine.

Produces a `garage-engine` folder containing the executable and its libraries. It still shells out to the
system `tesseract` and `gs` binaries: those are optional features, not
bundled, so the app can tell you honestly when they are missing.
"""

from pathlib import Path

root = Path(SPECPATH).parent

# The official C2PA SDK loads a native library at runtime; PyInstaller does
# not discover it from the ctypes call, so stage it explicitly.
try:
    import c2pa as _c2pa

    _c2pa_libs = Path(_c2pa.__file__).parent / "libs"
    c2pa_binaries = [
        (str(p), str(Path("c2pa/libs") / p.parent.relative_to(_c2pa_libs)))
        for p in _c2pa_libs.rglob("*")
        if p.is_file() and p.suffix in (".so", ".dll", ".dylib")
    ]
except Exception:  # noqa: BLE001 - verification degrades to "unavailable"
    c2pa_binaries = []

a = Analysis(
    [str(root / "engine" / "run.py")],
    pathex=[str(root / "engine")],
    binaries=c2pa_binaries,
    datas=[],
    hiddenimports=[
        "c2pa",
        "pillow_heif",
        "pillow_avif",
        "PIL._tkinter_finder",
    ],
    # Our hook trims PyMuPDF's unused layout models and development headers.
    hookspath=[str(root / "engine" / "hooks")],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[
        "tkinter",
        "matplotlib",
        "pytest",
        "setuptools",
        "pip",
        "IPython",
        "notebook",
    ],
    noarchive=False,
)

pyz = PYZ(a.pure)

# One-directory build: no ~75 MB archive is unpacked into /tmp on every
# launch, so the first job is not held up by PyInstaller's bootloader.
exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name="garage-engine",
    debug=False,
    bootloader_ignore_signals=False,
    strip=True,
    upx=False,
    console=True,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)
coll = COLLECT(
    exe,
    a.binaries,
    a.datas,
    strip=True,
    upx=False,
    name="garage-engine",
)
