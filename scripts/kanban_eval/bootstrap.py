#!/usr/bin/env python3
"""Install the pinned Kanban evaluator runtime and emit its immutable contract."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


IMAGE = 'mcr.microsoft.com/playwright@sha256:cf0daee9b994042e011bc29f20cdff1a9f682a039b43fcd738f7d8a9d3bcd9d6'
PLAYWRIGHT_MODULE = 'playwright/index.mjs'
FIXTURE_REVISION = '0471257a95095da7c5e9d366e26636976472e90d'


def run(*command, **kwargs):
    subprocess.run(command, check=True, **kwargs)


def required_file(path, label):
    path = Path(path).resolve(strict=True)
    if not path.is_file():
        raise ValueError(f'{label} must be a file: {path}')
    return path


def required_directory(path, label):
    path = Path(path).resolve(strict=True)
    if not path.is_dir():
        raise ValueError(f'{label} must be a directory: {path}')
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fixture', type=Path, required=True)
    parser.add_argument('--iii', type=Path, required=True)
    parser.add_argument('--runtime-root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--node', type=Path, required=True)
    parser.add_argument('--npm', type=Path, required=True)
    args = parser.parse_args()
    fixture = required_directory(args.fixture, 'fixture')
    catalog = required_file(fixture / 'scenarios/catalog.json', 'fixture catalog')
    required_file(fixture / 'kanban/pnpm-lock.yaml', 'fixture lockfile')
    head = subprocess.check_output(['git', '-C', str(fixture), 'rev-parse', 'HEAD'], text=True).strip()
    if head != FIXTURE_REVISION:
        raise ValueError(f'fixture revision mismatch: expected {FIXTURE_REVISION}, observed {head}')
    embedded_catalog = Path(__file__).resolve().parents[2] / 'src/scenarios/kanban/catalog.json'
    if catalog.read_bytes() != embedded_catalog.read_bytes():
        raise ValueError('fixture catalog differs from the runner-embedded catalog')
    iii = required_file(args.iii, 'iii')
    node = required_file(args.node, 'node')
    npm = required_file(args.npm, 'npm')
    root = args.runtime_root.resolve()
    if args.output.exists():
        raise ValueError('runtime output must be new')
    root.mkdir(parents=True, exist_ok=True)
    tools = Path(__file__).with_name('tools')
    cache = root / 'npm-cache'
    dependencies = fixture / 'kanban/node_modules'
    browser_dependencies = tools
    browsers = '/ms-playwright'
    environment = {**os.environ, 'npm_config_cache': str(cache)}
    run('docker', 'pull', IMAGE, env=environment)
    digests = json.loads(subprocess.check_output(
        ['docker', 'image', 'inspect', '--format', '{{json .RepoDigests}}', IMAGE], text=True,
    ))
    if not isinstance(digests, list) or IMAGE not in digests:
        raise ValueError('Docker image does not retain the pinned digest')
    run(str(npm), 'ci', '--ignore-scripts', '--no-audit', '--no-fund', '--prefix', str(tools), env=environment)
    pnpm = required_file(tools / 'node_modules/@pnpm/linux-x64/pnpm', 'pinned pnpm')
    run(str(pnpm), '--dir', str(fixture / 'kanban'), 'install', '--frozen-lockfile', '--ignore-scripts',
        '--store-dir', str(root / 'pnpm-store'), env=environment)
    if dependencies.exists() is False:
        raise ValueError('fixture dependency installation did not produce node_modules')
    if not (browser_dependencies / 'node_modules' / PLAYWRIGHT_MODULE).is_file():
        raise ValueError('pinned Playwright module is missing')
    runtime = {
        'fixture': str(fixture), 'image': IMAGE, 'node': str(node), 'iii': str(iii),
        'pnpm': str(pnpm), 'dependencies': str(dependencies),
        'browser-dependencies': str(browser_dependencies / 'node_modules'), 'browsers': browsers,
        'playwright-module': PLAYWRIGHT_MODULE,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(runtime, sort_keys=True) + '\n')


if __name__ == '__main__':
    try:
        main()
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        raise SystemExit(f'Kanban runtime bootstrap failed: {error}') from error
