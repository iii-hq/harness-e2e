#!/usr/bin/env python3
"""Choose the next release version and stamp it into the release tree.

Cutting a release is a version and a tag: this script picks the version from
the release tags and writes it into `Cargo.toml` and `Cargo.lock`; the caller
commits that on main and pushes the tag whose push runs `release.yml`.

The version is derived from the tags, never from `Cargo.toml`: the tags are
the record of what has been published, and the manifest only catches up when
a release is cut. Versions are plain semver. Older tags carry an
`-experimental` suffix and still order correctly, but no new one gets it.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys
from typing import Iterable, NamedTuple

WORKER_NAME = "harness-e2e"
TAG_PREFIX = f"{WORKER_NAME}/v"
EXPERIMENTAL = "experimental"
#: Mirrors release_worker.TAG_RE: MAJOR.MINOR.PATCH, with the legacy suffix
#: accepted so the existing tags keep parsing.
VERSION_RE = re.compile(
    r"^(?P<major>0|[1-9][0-9]*)\.(?P<minor>0|[1-9][0-9]*)\.(?P<patch>0|[1-9][0-9]*)"
    rf"(?P<channel>-{EXPERIMENTAL})?$"
)
BUMPS = ("patch", "minor", "major")


class Version(NamedTuple):
    major: int
    minor: int
    patch: int
    experimental: bool

    def __str__(self) -> str:
        core = f"{self.major}.{self.minor}.{self.patch}"
        return f"{core}-{EXPERIMENTAL}" if self.experimental else core

    @property
    def tag(self) -> str:
        return f"{TAG_PREFIX}{self}"

    def order(self) -> tuple[int, int, int, int]:
        # Semver orders a prerelease below the release that carries the same
        # core, so a stable 0.11.19 outranks 0.11.19-experimental.
        return (self.major, self.minor, self.patch, 0 if self.experimental else 1)


def parse_version(value: str) -> Version:
    match = VERSION_RE.fullmatch(value)
    if match is None:
        raise ValueError(f"not a release version: {value!r}")
    return Version(
        int(match.group("major")),
        int(match.group("minor")),
        int(match.group("patch")),
        match.group("channel") is not None,
    )


def parse_tag(tag: str) -> Version | None:
    """A release tag's version, or None for a tag this scheme does not own."""
    if not tag.startswith(TAG_PREFIX):
        return None
    try:
        return parse_version(tag[len(TAG_PREFIX):])
    except ValueError:
        return None


def latest_version(tags: Iterable[str]) -> Version | None:
    versions = [version for tag in tags if (version := parse_tag(tag)) is not None]
    return max(versions, key=Version.order) if versions else None


def next_version(current: Version | None, bump: str) -> Version:
    if bump not in BUMPS:
        raise ValueError(f"bump must be one of {', '.join(BUMPS)}")
    if current is None:
        # No release has ever been tagged; a first cut starts the line rather
        # than failing, and the bump still says where.
        first = {"patch": (0, 0, 1), "minor": (0, 1, 0), "major": (1, 0, 0)}[bump]
        return Version(*first, False)
    if bump == "patch":
        core = (current.major, current.minor, current.patch + 1)
    elif bump == "minor":
        core = (current.major, current.minor + 1, 0)
    else:
        core = (current.major + 1, 0, 0)
    return Version(*core, False)


def set_cargo_toml_version(text: str, version: str) -> str:
    """Rewrite the version of the `[package]` table, and only that one."""
    lines = text.splitlines(keepends=True)
    in_package = False
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_package = stripped == "[package]"
            continue
        if in_package and re.fullmatch(r'version\s*=\s*".*"\s*', line.rstrip("\n")):
            lines[index] = f'version = "{version}"\n'
            return "".join(lines)
    raise ValueError("Cargo.toml has no version in its [package] table")


def set_cargo_lock_version(text: str, version: str) -> str:
    """Rewrite the version of the harness-e2e entry, and only that one."""
    pattern = re.compile(
        rf'(\[\[package\]\]\nname = "{re.escape(WORKER_NAME)}"\nversion = )"[^"]*"'
    )
    updated, count = pattern.subn(rf'\1"{version}"', text, count=1)
    if count != 1:
        raise ValueError(f"Cargo.lock has no [[package]] entry for {WORKER_NAME}")
    return updated


def git(*args: str, root: pathlib.Path) -> str:
    return subprocess.run(
        ["git", *args], cwd=root, check=True, capture_output=True, text=True
    ).stdout.strip()


def release_tags(root: pathlib.Path) -> list[str]:
    output = git("tag", "--list", f"{TAG_PREFIX}*", root=root)
    return [line.strip() for line in output.splitlines() if line.strip()]


def command_resolve(args: argparse.Namespace) -> int:
    root = args.root
    tags = release_tags(root)
    current = latest_version(tags)
    if args.expected_current:
        expected = parse_version(args.expected_current)
        if current is None or expected.order() != current.order():
            raise SystemExit(
                f"expected current release {expected}, but the tags say "
                f"{current if current else 'none'}"
            )
    upcoming = next_version(current, args.bump)
    if upcoming.tag in tags:
        raise SystemExit(f"{upcoming.tag} already exists")
    payload = {
        "current_version": str(current) if current else "",
        "version": str(upcoming),
        "tag": upcoming.tag,
    }
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as handle:
            for key, value in payload.items():
                handle.write(f"{key}={value}\n")
    json.dump(payload, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


def command_set_version(args: argparse.Namespace) -> int:
    parse_version(args.version)
    root = args.root
    manifest = root / "Cargo.toml"
    lock = root / "Cargo.lock"
    manifest.write_text(
        set_cargo_toml_version(manifest.read_text(encoding="utf-8"), args.version),
        encoding="utf-8",
    )
    lock.write_text(
        set_cargo_lock_version(lock.read_text(encoding="utf-8"), args.version),
        encoding="utf-8",
    )
    print(f"stamped {args.version} into Cargo.toml and Cargo.lock")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path("."))
    sub = parser.add_subparsers(dest="command", required=True)

    resolve = sub.add_parser("resolve", help="choose the next version from the tags")
    resolve.add_argument("--bump", required=True, choices=BUMPS)
    resolve.add_argument(
        "--expected-current",
        default="",
        dest="expected_current",
        help="refuse unless this is the newest release tag",
    )
    resolve.add_argument("--github-output", type=pathlib.Path, default=None)
    resolve.set_defaults(func=command_resolve)

    stamp = sub.add_parser("set-version", help="write a version into Cargo.toml and Cargo.lock")
    stamp.add_argument("--version", required=True)
    stamp.set_defaults(func=command_set_version)

    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except ValueError as error:
        raise SystemExit(str(error)) from error


if __name__ == "__main__":
    raise SystemExit(main())
