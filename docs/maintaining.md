# Maintainer reference

Run commands from the repository root.

### Portable Linux builds

```bash
./scripts/build-linux.sh
```

This creates `dist/Garage-0.1.1-x86_64.AppImage`, a portable `.tar.gz`,
`dist/garage`, and the frozen `dist/garage-engine` folder. Keep the binary
and the folder together when using the bare binary. The AppImage includes both.

Packaging needs a type-2 AppImage runtime and `mksquashfs`. Set
`APPIMAGE_RUNTIME` and `MKSQUASHFS` to their paths; the script also finds
the cached electron-builder AppImage tools when available. To package an
existing build without recompiling, run `.venv/bin/python scripts/package-linux.py`.

Run the AppImage with `chmod +x Garage-*.AppImage` then `./Garage-*.AppImage`.
Without FUSE, use `./Garage-*.AppImage --appimage-extract` and run
`squashfs-root/AppRun`. The package's `build-info.json` records the minimum
glibc version required by its build host. For wider Linux compatibility,
build on the oldest distribution you support. A desktop graphics driver is
still required; OCR and Ghostscript remain optional system tools.

### Windows executable

Build on Windows x86_64 with Rust's MSVC toolchain, Python 3.12+, and the
Visual Studio C++ Build Tools/Windows SDK. From a developer PowerShell:

```powershell
./scripts/build-windows.ps1
```

The portable ZIP in `dist` contains `Garage.exe` and a `garage-engine` folder
(with `garage-engine.exe` inside). Keep them together after extraction. No separate Python installation is needed
to run it. The **Build and release** GitHub Actions workflow builds both platforms
and uploads downloadable packages as build artifacts.
The Windows build has to run on Windows because the Python sidecar is frozen
for the host platform.

For a headless packaged-engine check, run `garage --check-engine` on Linux.
Both platforms also accept `--version`.

### GitHub releases

The workflow in `.github/workflows/release.yml` builds Linux x86_64 on Ubuntu
24.04 and Windows x86_64 on Windows Server 2022. Pull requests, pushes to `main`,
and manual runs test and package both platforms without publishing a release.
Use **Actions → Build and release → Run workflow** to try a build first.

To publish a release:

1. Set the same version in `Cargo.toml`, `engine/pyproject.toml`, and the
   `VERSION` constant in `engine/garage_engine/main.py`.
2. Commit and push the changes, including the workflow and test fixture.
3. Tag that commit and push the tag (replace the example version as needed):

   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```

The tag must be `vMAJOR.MINOR.PATCH` and match all three version values. Both
builds must pass Rust tests and checks of their extracted packages before
publishing. The release contains the AppImage, Linux portable `.tar.gz`,
Windows portable ZIP, and a combined `SHA256SUMS` file. GitHub also provides
source archives for the tagged commit.

No personal access token or repository secret needs to be created. GitHub
provides `GITHUB_TOKEN` automatically; only the publishing job requests
`contents: write`. Repository or organization policies must allow Actions and
that permission. The job stages all downloads in a draft release and publishes
only after upload succeeds. Reruns can finish a draft; already published
releases are preserved. Use a new version tag for replacement builds.

The CI AppImage is built against Ubuntu 24.04; see its `build-info.json` for
the actual minimum glibc requirement. A working graphics driver is required.
Windows packages are unsigned portable ZIPs, not installers. OCR and
Ghostscript remain optional system tools on both platforms.

