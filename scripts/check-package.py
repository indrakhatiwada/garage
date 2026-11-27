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


def verify_features(result: dict, version: str, source: str) -> None:
    features = result.get('features', {})
    if (not features.get('pdf') or not features.get('c2pa')
            or result.get('version') != version):
        raise ValueError(f'Packaged engine from {source} is missing required capabilities/version: {result}')


def check_engine(engine: Path, cwd: Path, env: dict[str, str], version: str) -> None:
    result = subprocess.run(
        [str(engine)], input='{"id":"ping","op":"ping","args":{}}\n',
        capture_output=True, text=True, cwd=cwd, env=env, check=True, timeout=90,
    )
    replies = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    reply = next((item for item in replies if item.get('id') == 'ping'), {})
    if reply.get('event') != 'done':
        raise ValueError(f'Packaged engine did not reply to ping: {reply}; {result.stderr}')
    verify_features(reply.get('result', {}), version, str(engine))


def check_app(app: Path, cwd: Path, env: dict[str, str], version: str) -> None:
    """Launch the packaged app so engine discovery is tested end to end."""
    with tempfile.TemporaryDirectory(prefix='garage-app-check-') as directory:
        report = Path(directory) / 'engine.json'
        result = subprocess.run(
            [str(app), '--check-engine', str(report)], capture_output=True, text=True,
            cwd=cwd, env=env, timeout=120,
        )
        detail = f'stdout={result.stdout.strip()!r} stderr={result.stderr.strip()!r}'
        if result.returncode != 0:
            raise ValueError(
                f'{app.name} --check-engine failed ({result.returncode}): {detail}'
            )
        if not report.is_file():
            raise ValueError(f'{app.name} --check-engine wrote no report: {detail}')
        text = report.read_text().strip()
        if text.startswith('ERROR:'):
            raise ValueError(f'{app.name} --check-engine: {text}: {detail}')
        verify_features(json.loads(text), version, str(app))


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
            app = stage / 'Garage.exe'
            if not app.is_file():
                raise ValueError('Windows package is missing Garage.exe')
            check_engine(engine_binary(stage / 'garage-engine'), work, env, version)
            check_app(app, stage, env, version)
        else:
            archive = ROOT / f'dist/Garage-{version}-x86_64.tar.gz'
            with tarfile.open(archive) as package:
                package.extractall(work, filter='data')
            stage = work / f'Garage-{version}-x86_64'
            check_engine(engine_binary(stage / 'usr/bin/garage-engine'), work, env, version)
            image = ROOT / f'dist/Garage-{version}-x86_64.AppImage'
            subprocess.run([str(image), '--appimage-extract'], cwd=work, env=env,
                           stdout=subprocess.DEVNULL, check=True, timeout=90)
            check_app(work / 'squashfs-root/AppRun', work, env, version)
    print('Extracted packages passed engine checks.', flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('check', choices=['versions', 'packages'])
    args = parser.parse_args()
    version = check_versions()
    if args.check == 'packages':
        try:
            check_packages(version)
        except Exception as error:
            # Surface the failure in the GitHub Actions check annotation.
            print(f'::error::{error}', flush=True)
            raise
