#!/usr/bin/env python3
"""Package an existing release binary and frozen sidecar, without installing anything."""
from __future__ import annotations

import argparse
import ctypes.util
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sysconfig
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def run(*args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/garage')
    parser.add_argument('--engine', type=Path, default=ROOT / 'dist/garage-engine')
    parser.add_argument('--runtime', type=Path)
    parser.add_argument('--mksquashfs', type=Path)
    args = parser.parse_args()
    if platform.system() != 'Linux' or platform.machine() not in ('x86_64', 'amd64'):
        parser.error('This packaging target is Linux x86_64.')
    for binary in (args.binary, args.engine):
        if not binary.exists():
            parser.error(f'Missing binary: {binary}; build the app and sidecar first.')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    cache = Path.home() / '.cache/electron-builder/appimage/appimage-12.0.1'
    runtime = args.runtime or Path(os.environ.get('APPIMAGE_RUNTIME', cache / 'runtime-x64'))
    squash = args.mksquashfs or Path(os.environ.get('MKSQUASHFS', shutil.which('mksquashfs') or cache / 'linux-x64/mksquashfs'))
    if not runtime.is_file() or not squash.is_file():
        parser.error('Set APPIMAGE_RUNTIME to a type-2 x86_64 AppImage runtime and MKSQUASHFS to mksquashfs.')
    dist = ROOT / 'dist'
    work = ROOT / 'target/appimage'
    stage = work / 'Garage.AppDir'
    if stage.exists():
        shutil.rmtree(stage)
    (stage / 'usr/bin').mkdir(parents=True)
    lib = stage / 'usr/lib'
    lib.mkdir(parents=True)
    licenses = stage / 'usr/share/licenses'
    licenses.mkdir(parents=True)
    shutil.copy2(args.binary, stage / 'usr/bin/garage')
    if args.engine.is_dir():
        shutil.copytree(args.engine, stage / 'usr/bin/garage-engine')
    else:
        shutil.copy2(args.engine, stage / 'usr/bin/garage-engine')
    shutil.copy2(ROOT / 'assets/brand/garage-512.png', stage / 'garage.png')
    shutil.copy2(ROOT / 'assets/brand/garage-512.png', stage / '.DirIcon')
    shutil.copy2(ROOT / 'assets/garage.desktop', stage / 'garage.desktop')
    launcher = '''#!/bin/sh
set -eu
APPDIR=${APPDIR:-$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)}
export APPDIR
export PATH="$APPDIR/usr/bin:$PATH"
export LD_LIBRARY_PATH="$APPDIR/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
if [ -d "$APPDIR/usr/share/X11/xkb" ]; then
    export XKB_CONFIG_ROOT="$APPDIR/usr/share/X11/xkb"
fi
exec "$APPDIR/usr/bin/garage" "$@"
'''
    (stage / 'AppRun').write_text(launcher)
    (stage / 'AppRun').chmod(0o755)
    (stage / 'Garage').symlink_to('AppRun')
    excluded = re.compile(r'^(?:ld-linux.*|lib(?:c|m|dl|pthread|rt|resolv|util|anl)\.so(?:\..*)?)$')
    bundled = {}
    # The one-directory engine already carries its own libraries; only scan
    # the Rust binary for extra system libraries to bundle.
    queue = [args.binary.resolve()]
    # GPUI loads these at runtime, so they do not all appear in the ELF dependency list.
    for name in ('fontconfig', 'freetype', 'vulkan', 'wayland-client', 'wayland-cursor', 'wayland-egl', 'X11', 'Xcursor', 'Xrandr', 'Xi', 'EGL', 'GL'):
        soname = ctypes.util.find_library(name)
        if soname:
            for directory in ('/usr/lib', '/usr/lib64', '/lib/x86_64-linux-gnu', '/usr/lib/x86_64-linux-gnu'):
                candidate = Path(directory) / soname
                if candidate.exists():
                    shutil.copy2(candidate.resolve(), lib / soname)
                    bundled[soname] = str(candidate.resolve())
                    queue.append(candidate.resolve())
                    break
    inspected = set()
    while queue:
        binary = queue.pop()
        if binary in inspected:
            continue
        inspected.add(binary)
        result = run('ldd', binary, capture_output=True, text=True).stdout
        if 'not found' in result:
            raise RuntimeError(f'Unresolved dependencies for {binary}:\n{result}')
        for soname, filename in re.findall(r'^\s*(\S+)\s+=>\s+(\/\S+)', result, re.MULTILINE):
            soname = Path(soname).name
            if excluded.match(soname) or soname in bundled:
                continue
            path = Path(filename).resolve()
            shutil.copy2(path, lib / soname)
            bundled[soname] = str(path)
            queue.append(path)
        if binary.name.startswith('lib') and not excluded.match(binary.name):
            target = lib / binary.name
            if not target.exists():
                shutil.copy2(binary, target)
                bundled[binary.name] = str(binary)
    if Path('/usr/share/X11/xkb').is_dir():
        shutil.copytree('/usr/share/X11/xkb', stage / 'usr/share/X11/xkb', symlinks=False)
    for binary in set(bundled.values()):
        if shutil.which('pacman'):
            owner = subprocess.run(['pacman', '-Qqo', binary], capture_output=True, text=True).stdout.strip()
            source = Path('/usr/share/licenses') / owner
            if owner and source.is_dir() and not (licenses / owner).exists():
                shutil.copytree(source, licenses / owner, symlinks=False)
    if (ROOT / 'LICENSE').is_file():
        shutil.copy2(ROOT / 'LICENSE', stage / 'LICENSE')
    glibc = set()
    python_lib = Path(sysconfig.get_config_var('LIBDIR') or '/usr/lib') / (sysconfig.get_config_var('LDLIBRARY') or 'libpython.so')
    for binary in [args.binary, python_lib, *map(Path, bundled.values())]:
        if not binary.is_file():
            continue
        symbols = subprocess.run(['objdump', '-T', str(binary)], capture_output=True, text=True).stdout
        glibc.update(re.findall(r'GLIBC_(\d+\.\d+(?:\.\d+)?)', symbols))
    minimum = max(glibc, key=lambda s: tuple(map(int, s.split('.')))) if glibc else 'unknown'
    metadata = {'version': version, 'architecture': 'x86_64', 'built_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                'minimum_glibc': minimum, 'bundled_libraries': bundled,
                'python_bundled': True, 'optional_system_tools': ['tesseract', 'gs']}
    (stage / 'build-info.json').write_text(json.dumps(metadata, indent=2) + '\n')
    (stage / 'README.txt').write_text(f'''Garage {version} — Linux x86_64

Run ./Garage or ./AppRun. The Python engine and document libraries are bundled.
This local build requires glibc {minimum} or later and a working desktop/Vulkan driver.
Tesseract (OCR) and Ghostscript are optional system tools. Nepali OCR needs Nepali language data.
No installation, account, or network connection is used by the app.
License: AGPL-3.0-or-later. Source: the Garage repository used for this build.

AppImage without FUSE: run --appimage-extract, then squashfs-root/AppRun.
For broader distribution, rebuild on the oldest Linux distribution you support.
''')
    image = work / 'garage.squashfs'
    if image.exists():
        image.unlink()
    run(squash, stage, image, '-noappend', '-all-root', '-comp', 'gzip', '-b', '131072', '-processors', '2', '-no-progress')
    output = dist / f'Garage-{version}-x86_64.AppImage'
    with output.open('wb') as handle:
        handle.write(runtime.read_bytes())
        with image.open('rb') as filesystem:
            shutil.copyfileobj(filesystem, handle)
    output.chmod(0o755)
    archive = dist / f'Garage-{version}-x86_64.tar.gz'
    with tarfile.open(archive, 'w:gz') as package:
        package.add(stage, arcname=f'Garage-{version}-x86_64')
    shutil.copy2(args.binary, dist / 'garage')
    artifacts = [output, archive, dist / 'garage', dist / 'garage-engine']
    source_archive = dist / f'Garage-{version}-source.zip'
    if source_archive.is_file():
        artifacts.append(source_archive)
    hashes = []
    for path in artifacts:
        if path.is_dir():
            digest = hashlib.sha256()
            for child in sorted(p for p in path.rglob('*') if p.is_file()):
                digest.update(child.relative_to(path).as_posix().encode())
                with child.open('rb') as handle:
                    digest.update(hashlib.file_digest(handle, 'sha256').digest())
            hashes.append(f'{digest.hexdigest()}  {path.name}/\n')
        else:
            with path.open('rb') as handle:
                hashes.append(f'{hashlib.file_digest(handle, "sha256").hexdigest()}  {path.name}\n')
    (dist / 'SHA256SUMS').write_text(''.join(hashes))
    print(f'Created {output}\nCreated {archive}\nMinimum glibc: {minimum}\nBundled {len(bundled)} shared libraries.')


if __name__ == '__main__':
    main()
