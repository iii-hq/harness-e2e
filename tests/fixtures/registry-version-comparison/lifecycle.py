#!/usr/bin/env python3
"""Fixture lifecycle for the four Registry scenarios."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import uuid

REGISTRY_SHA = "662eb87c1bdbb395f36264d5d26bf823e2ace783"
REGISTRY_URL = "https://github.com/iii-hq/registry.git"
FIXTURE_URL = "https://github.com/iii-hq/e2e-fixture.git"
RUNNER_IMAGE = "docker:27.5.1-dind@sha256:aa3df78ecf320f5fafdce71c659f1629e96e9de0968305fe1de670e0ca9176ce"
TASKS = {1: "planning", 2: "implementation", 3: "environment", 4: "verification"}


def run(argv, *, cwd=None, data=None, env=None, timeout=900):
    result = subprocess.run([str(x) for x in argv], cwd=cwd, input=data,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            env=env, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"{argv[0]} failed ({result.returncode}): "
                           + result.stderr.decode(errors="replace")[-12000:]
                           + result.stdout.decode(errors="replace")[-12000:])
    return result.stdout


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fixture_hashes(directory):
    return {str(p.relative_to(directory)): digest(p)
            for p in sorted(directory.rglob("*")) if p.is_file() and ".git" not in p.parts}


def git_patch(source, base=REGISTRY_SHA):
    # A temporary index includes new and committed files without altering the subject's index.
    with tempfile.TemporaryDirectory() as temp:
        objects = Path(temp) / "objects"
        objects.mkdir()
        env = {**os.environ, "GIT_INDEX_FILE": str(Path(temp) / "index"),
               "GIT_OBJECT_DIRECTORY": str(objects),
               "GIT_ALTERNATE_OBJECT_DIRECTORIES": str(source.resolve() / ".git/objects")}
        run(["git", "read-tree", "HEAD"], cwd=source, env=env)
        run(["git", "add", "-A", "--", "."], cwd=source, env=env)
        return run(["git", "diff", "--cached", "--binary", "--full-index", base, "--"],
                   cwd=source, env=env)


def apply_delivery(source, delivery):
    manifest = json.loads((delivery / "manifest.json").read_text())
    patch = delivery / "implementation.patch"
    if manifest["base_registry_sha"] != REGISTRY_SHA or manifest["patch_sha256"] != digest(patch):
        raise ValueError("Implementation base or patch checksum does not match")
    if run(["git", "rev-parse", "HEAD"], cwd=source).decode().strip() != REGISTRY_SHA:
        raise ValueError("Replay source does not match the required Registry base")
    if run(["git", "status", "--porcelain"], cwd=source):
        raise ValueError("Replay requires a clean Registry checkout")
    if patch.stat().st_size:
        run(["git", "apply", "--check", patch], cwd=source)
        run(["git", "apply", patch], cwd=source)
    return manifest


def container_exec(state, command, timeout=900):
    return run(["docker", "exec", "-i", "-w", "/workspace", state["container"],
                "sh", "-lc", command], timeout=timeout)


def fixture_action(state, action):
    # The fixture is inside the private daemon's namespace, never the host Docker socket.
    return container_exec(state, f"/fixture/fixture.sh {action}", timeout=1800)


def prepare(args):
    root = args.root
    root.mkdir(parents=True, exist_ok=False)
    workspace = root / "workspace"
    inputs = workspace / "inputs"
    inputs.mkdir(parents=True)
    (workspace / "output").mkdir()
    state = {"test": args.test, "container": "registry-task-" + uuid.uuid4().hex,
             "web_port": args.web_port, "api_port": args.api_port,
             "base_registry_sha": REGISTRY_SHA, "runner_image": RUNNER_IMAGE}
    write_json(root / "state.json", state)
    state["fixture_files"] = {}
    if args.test != 1:
        # Always use the newest default branch when an application environment is needed.
        run(["git", "clone", "--depth", "1", FIXTURE_URL, root / "fixture-checkout"])
        fixture = root / "fixture-checkout" / "registry-version-comparison"
        if not fixture.is_dir():
            raise RuntimeError("Latest e2e-fixture default branch lacks registry-version-comparison; merge the fixture PR first")
        state["fixture_files"] = fixture_hashes(fixture)
    write_json(root / "state.json", state)
    run(["git", "clone", "--no-checkout", REGISTRY_URL, workspace / "registry"])
    run(["git", "checkout", "--detach", REGISTRY_SHA], cwd=workspace / "registry")
    if args.test == 4:
        state["implementation"] = apply_delivery(workspace / "registry", args.implementation)
        write_json(root / "state.json", state)
    shutil.copyfile(args.assets / "requirements.md", inputs / "requirements.md")
    if args.test == 2:
        shutil.copyfile(args.assets / "reference-plan.md", inputs / "reference-plan.md")
    if args.test != 1:
        shutil.copyfile(fixture / "seed.sql", inputs / "seed.sql")
        shutil.copytree(fixture / "artifacts", inputs / "artifacts")
    state["input_files"] = fixture_hashes(inputs)
    state["initial_patch_sha256"] = hashlib.sha256(git_patch(workspace / "registry")).hexdigest()
    write_json(root / "state.json", state)
    environment = {
        "test": args.test, "registry_sha": REGISTRY_SHA,
        "workspace": "/workspace", "web_url": f"http://127.0.0.1:{args.web_port}",
        "api_url": f"http://127.0.0.1:{args.api_port}",
        "web_port": args.web_port, "api_port": args.api_port,
        "runtime_requirements": "Linux amd64; Node >=22; pnpm 10.19.0; Bun; iii 0.22.1; PostgreSQL 17 with pgvector; Playwright Chromium. Install from Registry's lockfile. Seed with psql -v artifact_origin=http://127.0.0.1:<web_port> -v ON_ERROR_STOP=1 -f /workspace/inputs/seed.sql. Serve inputs/artifacts at /fixture-artifacts/ on the web origin.",
        "scope": "Only this private container and workspace. No host Docker socket, credentials, or other task workspaces are mounted. Public network access is available. Never contact production services.",
    }
    if args.test == 1:
        environment["runtime_requirements"] = "Planning only: no application runtime, seed.sql, or artifact files are supplied. Use the public fixture data described in requirements.md when planning tests. The implementation scenario receives the prepared runtime and seed assets."
    if args.test in (2, 4):
        environment["commands"] = {
            "start_or_rebuild": "/fixture/fixture.sh up",
            "baseline_check": "/fixture/fixture.sh check /workspace/output/baseline",
            "logs": "/fixture/fixture.sh logs",
            "api_tests": "docker compose -f /fixture/compose.yaml exec -T api pnpm test",
            "web_tests": "docker compose -f /fixture/compose.yaml exec -T web pnpm test",
        }
        environment["build_note"] = "Application containers use a built source snapshot. Rebuild with start_or_rebuild after edits; do not commit Registry HEAD because the fixture checks the starting SHA. Use docker compose exec for installed Node/pnpm/browser tools."
    else:
        environment["build_note"] = "Basic shell, Git and Docker CLI are available. Install additional tools if needed. No prepared application environment is supplied."
    write_json(inputs / "environment.json", environment)
    prompt = (args.assets / f"test-{args.test}-{TASKS[args.test]}.md").read_text()
    (root / "prompt.md").write_text(prompt)
    run(["docker", "pull", RUNNER_IMAGE])
    argv = ["docker", "run", "-d", "--name", state["container"],
            "--mount", f"type=bind,src={workspace},dst=/workspace",
            "-e", "DOCKER_TLS_CERTDIR=", "-e", "DOCKER_HOST=unix:///var/run/docker.sock",
            "-e", f"WEB_PORT={args.web_port}", "-e", f"API_PORT={args.api_port}",
            "-e", "REGISTRY_SOURCE=/workspace/registry", "-e", "FIXTURE_DIR=/fixture",
            "-e", f"COMPOSE_PROJECT_NAME={state['container']}"]
    if args.test != 1:
        argv += ["--privileged", "-p", f"127.0.0.1:{args.web_port}:8080",
                 "-p", f"127.0.0.1:{args.api_port}:8081"]
    if args.test in (2, 4):
        argv += ["--mount", f"type=bind,src={fixture},dst=/fixture,readonly"]
    if args.test == 1:
        argv += ["--entrypoint", "sh", RUNNER_IMAGE, "-c", "sleep infinity"]
    else:
        # Listen only on the private Unix socket; no unauthenticated TCP daemon.
        argv += ["--entrypoint", "dockerd", RUNNER_IMAGE, "--host=unix:///var/run/docker.sock"]
    run(argv)
    state["runner_image_id"] = run(["docker", "inspect", "--format", "{{.Image}}", state["container"]]).decode().strip()
    write_json(root / "state.json", state)
    container_exec(state, "apk add --no-cache git bash curl coreutils socat python3 && git config --global --add safe.directory /workspace/registry")
    if args.test != 1:
        container_exec(state, "for i in $(seq 1 60); do docker info >/dev/null 2>&1 && exit 0; sleep 1; done; exit 1", timeout=90)
    if args.test != 1:
        for exposed, target in ((8080, args.web_port), (8081, args.api_port)):
            run(["docker", "exec", "-d", state["container"], "socat",
                 f"TCP-LISTEN:{exposed},fork,reuseaddr", f"TCP:127.0.0.1:{target}"])
    if args.test in (2, 4):
        (root / "baseline-start.log").write_bytes(fixture_action(state, "up"))
        (root / "baseline-check.log").write_bytes(fixture_action(state, "check /workspace/output/baseline"))
    return state


def execute(args):
    state = json.loads((args.root / "state.json").read_text())
    # coreutils timeout kills the command inside the container, including on transport timeout.
    result = subprocess.run(["docker", "exec", "-i", "-w", "/workspace", state["container"],
                             "timeout", "--signal=KILL", str(args.timeout_ms / 1000),
                             "sh", "-lc", args.command], stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=args.timeout_ms / 1000 + 15)
    evidence = {"command": args.command, "timeout_ms": args.timeout_ms,
                "exit_code": result.returncode, "stdout": result.stdout.decode(errors="replace"),
                "stderr": result.stderr.decode(errors="replace")}
    commands = args.root / "commands"
    commands.mkdir(exist_ok=True)
    write_json(commands / (uuid.uuid4().hex + ".json"), evidence)
    return {"exit_code": result.returncode, "stdout": evidence["stdout"][-60000:],
            "stderr": evidence["stderr"][-20000:]}


def finish(args):
    root = args.root
    state = json.loads((root / "state.json").read_text())
    source = root / "workspace" / "registry"
    patch = git_patch(source)
    result = {"test": state["test"], "source_changed": hashlib.sha256(patch).hexdigest() != state["initial_patch_sha256"]}
    (root / "source.patch").write_bytes(patch)
    (root / "source-status.txt").write_bytes(run(["git", "status", "--short"], cwd=source))
    if state["test"] == 2 and (patch or args.subject_status == "finished"):
        delivery = root / "delivery"
        delivery.mkdir(exist_ok=True)
        (delivery / "implementation.patch").write_bytes(patch)
        write_json(delivery / "manifest.json", {"base_registry_sha": REGISTRY_SHA,
                   "patch_sha256": digest(delivery / "implementation.patch"),
                   "subject_status": args.subject_status,
                   "fixture_files": state["fixture_files"], "input_files": state["input_files"]})
        report = root / "workspace" / "output" / "report.md"
        if report.is_file():
            shutil.copyfile(report, delivery / "implementation-report.md")
    if state["test"] in (1, 4) and result["source_changed"]:
        result["scope_deviation"] = "Registry source changed in a planning or verification task"
    if state["test"] in (2, 4):
        result["runtime_ready"] = False
    if state["test"] in (2, 4) and "scope_deviation" not in result:
        try:
            # Rebuild before the controller captures the host-mapped application.
            (root / "final-start.log").write_bytes(fixture_action(state, "up"))
            result["runtime_ready"] = True
        except (RuntimeError, subprocess.TimeoutExpired) as error:
            result["runtime_error"] = str(error)
    write_json(root / "delivery-status.json", result)
    return result


def cleanup(args):
    state_path = args.root / "state.json"
    if not state_path.exists():
        return {"cleanup": "nothing_created"}
    state = json.loads(state_path.read_text())
    if not run(["docker", "ps", "-aq", "--filter", f"name=^/{state['container']}$"]):
        return {"cleanup": "nothing_created"}
    if state["test"] in (2, 4):
        try:
            (args.root / "compose.log").write_bytes(fixture_action(state, "logs"))
            (args.root / "runtime-images.json").write_bytes(container_exec(
                state, "docker compose -f /fixture/compose.yaml images --format json"))
        except (RuntimeError, subprocess.TimeoutExpired) as error:
            (args.root / "runtime-log-error.txt").write_text(str(error))
    log = subprocess.run(["docker", "logs", state["container"]], capture_output=True, timeout=30)
    (args.root / "container.log").write_bytes(log.stdout + log.stderr)
    # Removing the outer daemon deletes all nested app containers and its anonymous data volume.
    result = subprocess.run(["docker", "rm", "-fv", state["container"]], capture_output=True, timeout=60)
    return {"cleanup_exit_code": result.returncode, "stderr": result.stderr.decode(errors="replace")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "exec", "finish", "cleanup"])
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--assets", type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument("--test", type=int, choices=TASKS)
    parser.add_argument("--web-port", type=int)
    parser.add_argument("--api-port", type=int)
    parser.add_argument("--implementation", type=Path)
    parser.add_argument("--command")
    parser.add_argument("--subject-status", default="not_run")
    parser.add_argument("--timeout-ms", type=int, default=120000)
    args = parser.parse_args()
    args.root = args.root.resolve()
    if args.action == "prepare":
        if args.test is None or args.web_port is None or args.api_port is None:
            parser.error("prepare requires --test, --web-port and --api-port")
        if args.test == 4 and args.implementation is None:
            parser.error("Test 4 requires --implementation")
    if args.action == "exec" and (not args.command or not 0 < args.timeout_ms <= 120000):
        parser.error("exec requires --command and 0 < --timeout-ms <= 120000")
    action = {"prepare": prepare, "exec": execute, "finish": finish, "cleanup": cleanup}[args.action]
    print(json.dumps(action(args)))


if __name__ == "__main__":
    main()
