#!/usr/bin/env python3
"""Prepare, start, inspect and stop the Linkly stack the `linkly_tutorial` scenario runs against.

The `linkly-agentic` scaffold is the Compose project that hosts the Harness, so it is also the
stack under test. This script encodes the manual steps of the tutorial plus the two template
gaps tracked in MOT-4739 (a provider block without `env_file`, a `harness.start_after` that
cannot see a provider added later):

  scaffold  iii project init <name> --template linkly-agentic, enable providers, write keys
  up        exec `iii compose --up` in the project (foreground; keep the pane alive)
  status    print each container's state from compose::status
  down      SIGINT the compose daemon (graceful; a pane kill orphans the workers)

Provider keys are copied from the caller's environment into `.env` and never printed.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

TEMPLATE = "linkly-agentic"
LOCALIZED_WORKERS = ("console", "shell", "session-manager", "iii-directory", "harness")
PROVIDER_KEYS = {
    "anthropic": "ANTHROPIC_API_KEY",
    "openai": "OPENAI_API_KEY",
    "deepseek": "DEEPSEEK_API_KEY",
    "kimi": "MOONSHOT_API_KEY",
    "xai": "XAI_API_KEY",
    "zai": "ZAI_API_KEY",
    "openrouter": "OPENROUTER_API_KEY",
    "llamacpp": "LLAMACPP_API_KEY",
}
CONTAINER = re.compile(r"^  ([a-z0-9-]+):(\s*#.*)?$")
COMMENTED_CONTAINER = re.compile(r"^  #  ([a-z0-9-]+):(\s*#.*)?$")


def enable_provider(lines: list[str], provider: str) -> list[str]:
    """Uncomment `provider-<name>` (if commented) and make sure it reads `.env`."""
    name = f"provider-{provider}"
    out: list[str] = []
    i = 0
    found = False
    while i < len(lines):
        line = lines[i]
        commented = COMMENTED_CONTAINER.match(line)
        live = CONTAINER.match(line)
        if commented and commented.group(1) == name:
            found = True
            out.append(line.replace("  #  ", "  ", 1))
            i += 1
            block: list[str] = []
            while i < len(lines) and lines[i].startswith("  #    "):
                block.append(lines[i].replace("  #    ", "    ", 1))
                i += 1
            if not any(item.startswith("    env_file:") for item in block):
                block.append("    env_file: ['./.env']")
            out.extend(block)
            continue
        if live and live.group(1) == name:
            found = True
            out.append(line)
            i += 1
            block = []
            while i < len(lines) and (lines[i].startswith("    ") or lines[i].strip() == ""):
                block.append(lines[i])
                i += 1
            trail: list[str] = []
            while block and block[-1].strip() == "":
                trail.insert(0, block.pop())
            if not any(item.startswith("    env_file:") for item in block):
                block.append("    env_file: ['./.env']")
            out.extend(block + trail)
            continue
        out.append(line)
        i += 1
    if not found:
        raise SystemExit(f"{name} has no block in worker-compose.yaml")
    return out


def extend_harness_start_after(lines: list[str], provider: str) -> list[str]:
    """Add `provider-<name>` to the harness container's `start_after` list, alphabetically."""
    name = f"provider-{provider}"
    out: list[str] = []
    i = 0
    done = False
    while i < len(lines):
        line = lines[i]
        match = CONTAINER.match(line)
        if match and match.group(1) == "harness":
            out.append(line)
            i += 1
            while i < len(lines) and lines[i].startswith("    ") and not lines[i].startswith("    start_after:"):
                out.append(lines[i])
                i += 1
            if i < len(lines) and lines[i].startswith("    start_after:"):
                out.append(lines[i])
                i += 1
                items: list[str] = []
                while i < len(lines) and lines[i].startswith("      - "):
                    items.append(lines[i][len("      - "):].strip())
                    i += 1
                if name not in items:
                    items.append(name)
                out.extend(f"      - {item}" for item in sorted(items))
                done = True
            continue
        out.append(line)
        i += 1
    if not done:
        raise SystemExit("the harness container has no start_after list in worker-compose.yaml")
    return out


def localize(lines: list[str], workers: Path) -> list[str]:
    """Run the five locally built workers from `workers/<name>/target/release/<name>`."""
    out: list[str] = []
    i = 0
    done: set[str] = set()
    while i < len(lines):
        line = lines[i]
        match = CONTAINER.match(line)
        if match and match.group(1) in LOCALIZED_WORKERS:
            worker = match.group(1)
            out.append(line)
            i += 1
            block: list[str] = []
            while i < len(lines) and (lines[i].startswith("    ") or lines[i].strip() == ""):
                block.append(lines[i])
                i += 1
            trail: list[str] = []
            while block and block[-1].strip() == "":
                trail.insert(0, block.pop())
            rest = [item for item in block if not re.match(r"^    (worker|version|scripts):", item)
                    and not item.startswith("      run:")]
            new = [
                f"    worker: path://{workers / worker}",
                "    scripts:",
                f"      run: {workers / worker / 'target' / 'release' / worker}",
            ]
            if not any(item.startswith("    working_dir:") for item in rest):
                new.append("    working_dir: .")
            out.extend(new + rest + trail)
            done.add(worker)
            continue
        out.append(line)
        i += 1
    missing = set(LOCALIZED_WORKERS) - done
    if missing:
        raise SystemExit(f"could not localize {sorted(missing)}")
    return out


def patch_compose(text: str, providers: list[str], workers: Path | None = None) -> str:
    lines = text.split("\n")
    for provider in providers:
        lines = enable_provider(lines, provider)
        lines = extend_harness_start_after(lines, provider)
    if workers is not None:
        lines = localize(lines, workers)
    return "\n".join(lines)


def patch_env(text: str, providers: list[str], environ: dict[str, str]) -> str:
    """Write each provider's key line from the environment; the value never reaches stdout."""
    lines = text.split("\n")
    for provider in providers:
        key = PROVIDER_KEYS.get(provider)
        if key is None:
            raise SystemExit(f"unknown provider {provider}; known: {', '.join(sorted(PROVIDER_KEYS))}")
        value = environ.get(key, "").strip()
        if not value:
            raise SystemExit(f"{key} is not set in the environment")
        replaced = False
        for index, line in enumerate(lines):
            if re.match(rf"^#?\s*{re.escape(key)}=", line):
                lines[index] = f"{key}={value}"
                replaced = True
                break
        if not replaced:
            lines.append(f"{key}={value}")
    return "\n".join(lines)


def run(args: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(args, check=True, text=True, **kwargs)


def project_dir(raw: str) -> Path:
    project = Path(raw).resolve()
    if not (project / "worker-compose.yaml").is_file():
        raise SystemExit(f"{project} has no worker-compose.yaml")
    return project


def compose_status(iii: str, project: Path) -> dict:
    """`compose::status` of the daemon that owns `project`; iii picks the namespace from the cwd."""
    result = run([iii, "trigger", "compose::status", "--json", "{}"], capture_output=True, cwd=project)
    return json.loads(result.stdout)


def cmd_scaffold(args: argparse.Namespace) -> None:
    parent = Path(args.dir).resolve()
    project = parent / args.name
    if project.exists():
        raise SystemExit(f"{project} already exists; a run needs a fresh scaffold")
    parent.mkdir(parents=True, exist_ok=True)
    providers = [item.strip() for item in args.provider.split(",") if item.strip()]
    if not providers:
        raise SystemExit("--provider must name at least one provider")
    environ = dict(os.environ)
    for provider in providers:
        patch_env("", [provider], environ)  # fail before scaffolding if a key is missing
    run([args.iii, "project", "init", args.name, "--template", TEMPLATE], cwd=parent)
    compose = project / "worker-compose.yaml"
    workers = Path(args.localize).resolve() if args.localize else None
    compose.write_text(patch_compose(compose.read_text(), providers, workers))
    env = project / ".env"
    env.write_text(patch_env(env.read_text() if env.is_file() else "", providers, environ))
    env.chmod(0o600)
    print(f"scaffolded {project}")
    print(f"providers enabled: {', '.join(providers)}")
    if workers:
        print(f"localized {', '.join(LOCALIZED_WORKERS)} to {workers}")
    print(f"next: python3 {Path(__file__).name} up --dir {project}")


def cmd_up(args: argparse.Namespace) -> None:
    project = project_dir(args.dir)
    os.chdir(project)
    os.execvp(args.iii, [args.iii, "compose", "--up"])


def cmd_status(args: argparse.Namespace) -> None:
    status = compose_status(args.iii, project_dir(args.dir))
    print(f"file: {status.get('file')}  namespace: {status.get('namespace')}  daemon: {status.get('daemon_pid')}")
    for container in status.get("containers", []):
        print(f"  {container.get('state', '?'):10} {container.get('container')}")


def cmd_down(args: argparse.Namespace) -> None:
    status = compose_status(args.iii, project_dir(args.dir))
    pid = status.get("daemon_pid")
    if not pid:
        raise SystemExit("compose::status reports no daemon pid")
    os.kill(int(pid), signal.SIGINT)
    deadline = time.time() + args.timeout
    while time.time() < deadline:
        if not Path(f"/proc/{pid}").exists():
            print(f"compose daemon {pid} exited")
            return
        time.sleep(0.5)
    raise SystemExit(f"compose daemon {pid} still running after {args.timeout}s")


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--iii", default=shutil.which("iii") or "iii", help="iii CLI to use")
    sub = parser.add_subparsers(dest="command", required=True)
    scaffold = sub.add_parser("scaffold", help="scaffold the template and enable providers")
    scaffold.add_argument("--dir", required=True, help="parent directory for the project")
    scaffold.add_argument("--name", default="linkly")
    scaffold.add_argument("--provider", required=True, help="comma-separated provider names, e.g. deepseek")
    scaffold.add_argument("--localize", help="workers checkout; run console/shell/session-manager/iii-directory/harness from its release binaries")
    scaffold.set_defaults(func=cmd_scaffold)
    for name, func, text in [
        ("up", cmd_up, "exec iii compose --up in the project (foreground)"),
        ("status", cmd_status, "print container states"),
        ("down", cmd_down, "stop the compose daemon gracefully"),
    ]:
        command = sub.add_parser(name, help=text)
        command.add_argument("--dir", required=True, help="project directory")
        if name == "down":
            command.add_argument("--timeout", type=float, default=60.0)
        command.set_defaults(func=func)
    args = parser.parse_args(argv)
    args.func(args)


if __name__ == "__main__":
    main()
