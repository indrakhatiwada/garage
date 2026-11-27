# Garage

A local-first desktop toolkit for PDFs, images, OCR, and Bikram Sambat dates. Files stay on your machine; no account or connection is needed.

![Garage](assets/screenshots/garage-app.png)

Compress and edit images, organize and convert PDFs, add watermarks or page numbers, protect or unlock PDFs, redact documents, clean up scans, and extract text with local OCR. Exports create new files.

## Setup

Requires Rust 1.85+, Python 3.11+, and GPUI system dependencies: Vulkan, Wayland/X11, and fontconfig on Linux. Windows builds require the MSVC Rust toolchain, Python 3.12+, and Visual Studio C++ Build Tools with the Windows SDK.

Install on Linux:

```bash
./scripts/install.sh
```

Build a portable Windows application from a developer PowerShell:

```powershell
./scripts/build-windows.ps1
```

Keep `Garage.exe` and `garage-engine.exe` together in the extracted output.

Optional system tools: `tesseract` for OCR (with Nepali language data for Nepali text), and `gs` for additional PDF compression.

## Development

```bash
python3 -m venv .venv
.venv/bin/pip install -e engine
cargo run
```

On Windows, use `.venv\Scripts\python.exe` and `.venv\Scripts\pip.exe` instead of the `.venv/bin/` paths.

## Tests

```bash
cargo test
PYTHONPATH=engine .venv/bin/python tests/smoke.py
PYTHONPATH=engine .venv/bin/python -m unittest discover -s tests -p 'test_*.py'
```

## License

AGPL-3.0-or-later. The PDF engine uses PyMuPDF under the same license. See [LICENSE](LICENSE).
