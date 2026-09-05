#!/usr/bin/env python3
"""Copy native SWE deliverables before the disposable stack is removed."""
import argparse
import json
from pathlib import Path
import re
import shutil


def extract(native_dir, output_dir):
    source = Path(native_dir).resolve() / "deliverables"
    destination = Path(output_dir).resolve()
    if not source.exists():
        return 0
    if source.is_symlink():
        raise ValueError("Native deliverables cannot be a symbolic link")
    selected = []
    for path in sorted(source.glob("*/swe_service_report.json")):
        if path.is_symlink() or path.parent.is_symlink() or not path.resolve().is_relative_to(source):
            raise ValueError("SWE report escaped the native deliverable directory")
        if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", path.parent.name):
            raise ValueError("Invalid SWE attempt directory")
        if path.stat().st_size > 16 * 1024 * 1024:
            raise ValueError("SWE report exceeds the artifact limit")
        selected.append(path)
    for path in selected:
        target = destination / path.parent.name / path.name
        if target.is_symlink() or target.parent.is_symlink():
            raise ValueError("SWE output path cannot be a symbolic link")
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists() and target.read_bytes() != path.read_bytes():
            raise ValueError("Existing SWE evidence differs from native bytes")
        shutil.copyfile(path, target)
    return len(selected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-dir", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps({"reports": extract(args.native_dir, args.output_dir)}))


if __name__ == "__main__":
    main()
