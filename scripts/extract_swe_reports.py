#!/usr/bin/env python3
"""Copy allowlisted native SWE and Kanban evidence before stack cleanup."""
import argparse
import json
from pathlib import Path
import re
import shutil


NAME = re.compile(r"[A-Za-z0-9_-]{1,128}")
LIMIT = 16 * 1024 * 1024


def valid(name):
    return NAME.fullmatch(name) is not None


def safe_source(root, path):
    current = path
    while True:
        if current.is_symlink():
            raise ValueError("Native evidence cannot be reached through a symbolic link")
        if current == root:
            break
        current = current.parent
    if not path.resolve().is_relative_to(root):
        raise ValueError("Native evidence escaped its native directory")
    if path.stat().st_size > LIMIT:
        raise ValueError("Native evidence exceeds the artifact limit")


def copy_immutable(source, target):
    for parent in [target, *target.parents]:
        if parent.is_symlink():
            raise ValueError("Evidence output path cannot be a symbolic link")
    target.parent.mkdir(parents=True, exist_ok=True)
    contents = source.read_bytes()
    if target.exists() and target.read_bytes() != contents:
        raise ValueError("Existing evidence differs from native bytes")
    if not target.exists():
        shutil.copyfile(source, target)


def extract(native_dir, output_dir):
    native = Path(native_dir)
    if not native.exists():
        return 0
    if native.is_symlink():
        raise ValueError("Native directory cannot be a symbolic link")
    native = native.resolve()
    destination = Path(output_dir).resolve()
    selected = []
    deliverables = native / "deliverables"
    if deliverables.exists():
        if deliverables.is_symlink():
            raise ValueError("Native deliverables cannot be a symbolic link")
        for run in sorted(deliverables.iterdir()):
            if not valid(run.name):
                continue
            swe = run / "swe_service_report.json"
            if swe.is_file():
                selected.append((swe, destination / run.name / swe.name))
            if not run.is_dir():
                continue
            for attempt in sorted(run.iterdir()):
                if not valid(attempt.name):
                    continue
                kanban = attempt / "kanban_evaluation.json"
                if kanban.is_file():
                    selected.append((kanban, destination / "kanban" / "deliverables" / run.name / attempt.name / kanban.name))
    evidence = native / "evidence"
    if evidence.exists():
        if evidence.is_symlink():
            raise ValueError("Native evidence cannot be a symbolic link")
        for run in sorted(evidence.iterdir()):
            if not valid(run.name) or not run.is_dir():
                continue
            for attempt in sorted(run.iterdir()):
                if not valid(attempt.name):
                    continue
                diagnostic = attempt / "kanban-controller.json"
                if diagnostic.is_file():
                    selected.append((diagnostic, destination / "kanban" / "evidence" / run.name / attempt.name / diagnostic.name))
    for source, target in selected:
        safe_source(native, source)
        copy_immutable(source, target)
    return len(selected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-dir", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps({"reports": extract(args.native_dir, args.output_dir)}))


if __name__ == "__main__":
    main()
