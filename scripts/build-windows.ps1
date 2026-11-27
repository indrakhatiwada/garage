# Run on Windows with Rust (MSVC), Python 3.12+, and Visual Studio C++ Build Tools.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if (-not (Test-Path '.venv/Scripts/python.exe')) {
    python -m venv .venv
    if ($LASTEXITCODE -ne 0) { throw 'Could not create the Python environment.' }
}
$python = Join-Path $root '.venv/Scripts/python.exe'
& $python -m pip install -e engine pyinstaller
if ($LASTEXITCODE -ne 0) { throw 'Python dependency installation failed.' }
if (Test-Path 'dist/garage-engine') { Remove-Item -Recurse -Force 'dist/garage-engine' }
& $python -m PyInstaller --noconfirm --clean --distpath dist --workpath target/pyinstaller engine/garage-engine.spec
if ($LASTEXITCODE -ne 0) { throw 'Engine packaging failed.' }
$previousRustFlags = $env:RUSTFLAGS
try {
    $env:RUSTFLAGS = "$previousRustFlags -C target-feature=+crt-static".Trim()
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Rust build failed.' }
} finally {
    $env:RUSTFLAGS = $previousRustFlags
}
$version = (& $python -c "import tomllib; print(tomllib.load(open('Cargo.toml','rb'))['package']['version'])").Trim()
$stage = Join-Path $root "dist/Garage-$version-windows-x86_64"
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target/release/garage.exe "$stage/Garage.exe" -Force
if (Test-Path "$stage/garage-engine") { Remove-Item -Recurse -Force "$stage/garage-engine" }
Copy-Item -Recurse -Force dist/garage-engine "$stage/garage-engine"
Copy-Item assets/brand/garage.ico "$stage/garage.ico" -Force
@'
Garage for Windows

Extract this folder and run Garage.exe. Keep the garage-engine folder beside it.
Python is bundled. OCR needs Tesseract on PATH; Ghostscript is optional.
Source: the Garage repository used to build this package. License: AGPL-3.0-or-later.
'@ | Set-Content "$stage/README.txt"
if (Test-Path LICENSE) { Copy-Item LICENSE "$stage/LICENSE.txt" -Force }
$archive = Join-Path $root "dist/Garage-$version-windows-x86_64.zip"
Compress-Archive -Path $stage -DestinationPath $archive -Force
Write-Host "Created $archive"
