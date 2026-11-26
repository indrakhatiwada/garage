#!/usr/bin/env python3
"""CI checks for version consistency and packages isolated from the source tree."""
from __future__ import annotations

import argparse
import ast
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def check_versions() -> str:
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    engine_version = tomllib.loads((ROOT / 'engine/pyproject.toml').read_text())['project']['version']
    tree = ast.parse((ROOT / 'engine/garage_engine/main.py').read_text())
    protocol_version = next(
        ast.literal_eval(node.value)
        for node in tree.body
        if isinstance(node, ast.Assign)
        and any(isinstance(target, ast.Name) and target.id == 'VERSION' for target in node.targets)
    )
    if not version == engine_version == protocol_version:
        raise ValueError(f'Versions disagree: Rust={version}, Python={engine_version}, protocol={protocol_version}')
    if os.environ.get('GITHUB_REF_TYPE') == 'tag':
        tag = os.environ.get('GITHUB_REF_NAME', '')
        if not re.fullmatch(r'v\d+\.\d+\.\d+', tag) or tag != f'v{version}':
            raise ValueError(f'Release tag must be v{version}, got {tag!r}')
    print(f'Versions agree: {version}', flush=True)
    return version


def check_engine(engine: Path, cwd: Path, env: dict[str, str], version: str) -> None:
    result = subprocess.run(
        [str(engine)], input='{"id":"ping","op":"ping","args":{}}\n',
        capture_output=True, text=True, cwd=cwd, env=env, check=True, timeout=90,
    )
    replies = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    reply = next((item for item in replies if item.get('id') == 'ping'), {})
    features = reply.get('result', {}).get('features', {})
    if (reply.get('event') != 'done' or not features.get('pdf') or not features.get('c2pa')
            or reply.get('result', {}).get('version') != version):
        raise ValueError(f'Packaged engine is missing required capabilities/version: {reply}; {result.stderr}')


def engine_binary(path: Path) -> Path:
    """One-directory bundles keep the executable inside a named folder."""
    if not path.is_dir():
        return path
    return path / ("garage-engine.exe" if platform.system() == 'Windows' else 'garage-engine')


def check_packages(version: str) -> None:
    env = os.environ.copy()
    for name in ('GARAGE_ENGINE', 'PYTHONPATH', 'PYTHONHOME', 'VIRTUAL_ENV'):
        env.pop(name, None)
    with tempfile.TemporaryDirectory(prefix='garage-package-') as directory:
        work = Path(directory)
        if platform.system() == 'Windows':
            archive = ROOT / f'dist/Garage-{version}-windows-x86_64.zip'
            with zipfile.ZipFile(archive) as package:
                package.extractall(work)
            stage = work / f'Garage-{version}-windows-x86_64'
            if not (stage / 'Garage.exe').is_file():
                raise ValueError('Windows package is missing Garage.exe')
            check_engine(engine_binary(stage / 'garage-engine'), work, env, version)
        else:
            archive = ROOT / f'dist/Garage-{version}-x86_64.tar.gz'
            with tarfile.open(archive) as package:
                package.extractall(work, filter='data')
            stage = work / f'Garage-{version}-x86_64'
            check_engine(engine_binary(stage / 'usr/bin/garage-engine'), work, env, version)
            image = ROOT / f'dist/Garage-{version}-x86_64.AppImage'
            subprocess.run([str(image), '--appimage-extract'], cwd=work, env=env,
                           stdout=subprocess.DEVNULL, check=True, timeout=90)
            result = subprocess.run([str(work / 'squashfs-root/AppRun'), '--check-engine'],
                                    cwd=work, env=env, capture_output=True, text=True,
                                    check=True, timeout=90)
            reply = json.loads(result.stdout)
            if (not all(reply.get('features', {}).get(name) for name in ('pdf', 'c2pa'))
                    or reply.get('version') != version):
                raise ValueError(f'AppImage engine check failed: {reply}')
    print('Extracted packages passed engine checks.', flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('check', choices=['versions', 'packages'])
    args = parser.parse_args()
    version = check_versions()
    if args.check == 'packages':
        check_packages(version)
