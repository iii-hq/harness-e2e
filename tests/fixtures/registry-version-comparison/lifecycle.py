#!/usr/bin/env python3
"""Fixture lifecycle for the four Registry scenarios."""
import argparse
import base64
import hashlib
import inspect
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import uuid

REGISTRY_SHA = "662eb87c1bdbb395f36264d5d26bf823e2ace783"
REGISTRY_URL = "https://github.com/iii-hq/registry.git"
FIXTURE_URL = "https://github.com/iii-hq/e2e-fixture.git"
RUNNER_IMAGE = "docker:27.5.1-dind@sha256:aa3df78ecf320f5fafdce71c659f1629e96e9de0968305fe1de670e0ca9176ce"
TASKS = {1: "planning", 2: "implementation", 3: "environment", 4: "verification"}
MAX_EVIDENCE_FILES = 64
MAX_EVIDENCE_ENTRIES = 256
MAX_EVIDENCE_FILE_BYTES = 1024 * 1024
MAX_EVIDENCE_TOTAL_BYTES = 8 * 1024 * 1024


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


def artifact_manifest(directory, max_files=MAX_EVIDENCE_FILES,
                      max_entries=MAX_EVIDENCE_ENTRIES,
                      max_file_bytes=MAX_EVIDENCE_FILE_BYTES,
                      max_total_bytes=MAX_EVIDENCE_TOTAL_BYTES,
                      include_data=False):
    if not stat.S_ISDIR(os.lstat(directory).st_mode):
        raise ValueError("evidence root must be a real directory")
    items = []
    total = 0
    entries = 0
    pending = [Path(directory)]
    while pending:
        current = pending.pop()
        if not stat.S_ISDIR(os.lstat(current).st_mode):
            raise ValueError("evidence entries must be real directories and regular files")
        with os.scandir(current) as children:
            for child in children:
                entries += 1
                if entries > max_entries:
                    raise ValueError("too many evidence entries")
                path = Path(child.path)
                metadata = child.stat(follow_symlinks=False)
                if stat.S_ISDIR(metadata.st_mode):
                    pending.append(path)
                    continue
                if not stat.S_ISREG(metadata.st_mode):
                    raise ValueError("evidence entries must be real directories and regular files")
                if len(items) >= max_files:
                    raise ValueError("too many evidence files")
                if metadata.st_size > max_file_bytes:
                    raise ValueError("evidence file exceeds size limit")
                total += metadata.st_size
                if total > max_total_bytes:
                    raise ValueError("evidence files exceed total size limit")
                checksum = hashlib.sha256()
                descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
                opened = os.fstat(descriptor)
                if ((opened.st_dev, opened.st_ino, opened.st_size)
                        != (metadata.st_dev, metadata.st_ino, metadata.st_size)):
                    os.close(descriptor)
                    raise ValueError("evidence file changed while it was inspected")
                remaining = metadata.st_size
                data = bytearray() if include_data else None
                with os.fdopen(descriptor, "rb") as stream:
                    while remaining:
                        chunk = stream.read(min(65536, remaining))
                        if not chunk:
                            break
                        checksum.update(chunk)
                        if data is not None:
                            data.extend(chunk)
                        remaining -= len(chunk)
                    if remaining or stream.read(1):
                        raise ValueError("evidence file changed while it was inspected")
                item = {"path": path.relative_to(directory).as_posix(),
                        "sha256": checksum.hexdigest(), "size_bytes": metadata.st_size}
                if data is not None:
                    item["data_base64"] = base64.b64encode(data).decode()
                items.append(item)
    return sorted(items, key=lambda item: item["path"])


def manifest_script(max_files=MAX_EVIDENCE_FILES,
                    max_entries=MAX_EVIDENCE_ENTRIES,
                    max_file_bytes=MAX_EVIDENCE_FILE_BYTES,
                    max_total_bytes=MAX_EVIDENCE_TOTAL_BYTES):
    source = inspect.getsource(artifact_manifest)
    return f"""import base64,hashlib,json,os,stat,sys
from pathlib import Path
MAX_EVIDENCE_FILES={MAX_EVIDENCE_FILES}
MAX_EVIDENCE_ENTRIES={MAX_EVIDENCE_ENTRIES}
MAX_EVIDENCE_FILE_BYTES={MAX_EVIDENCE_FILE_BYTES}
MAX_EVIDENCE_TOTAL_BYTES={MAX_EVIDENCE_TOTAL_BYTES}
{source}
print(json.dumps(artifact_manifest(Path(sys.argv[1]), max_files={max_files}, max_entries={max_entries}, max_file_bytes={max_file_bytes}, max_total_bytes={max_total_bytes}, include_data=True)))"""


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


def remove_other_stacks(state):
    # Only the fixture's project may run when the delivery is measured: a stack
    # the subject started and left up in the private daemon would hold its ports.
    return container_exec(
        state,
        "docker ps -a --format '{{.ID}} {{.Label \"com.docker.compose.project\"}}'"
        f" | awk '$2 != \"{state['container']}\" {{print $1}}' | xargs -r docker rm -fv")


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
            "-e", f"COMPOSE_PROJECT_NAME={state['container']}",
            # iii telemetry off for the runner's own processes, which a
            # container does not inherit from here. The fixture's compose does
            # not pass it on: its app image sets it (e2e-fixture's Dockerfile).
            "-e", "III_TELEMETRY_ENABLED=false"]
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
    receipt_id = uuid.uuid4().hex
    stage = f"/tmp/harness-evidence-{receipt_id}"
    destination = args.root / "execution-evidence" / receipt_id
    run(["docker", "exec", state["container"], "mkdir", "-p", stage])
    try:
        # coreutils timeout kills the command inside the container, including on transport timeout.
        result = subprocess.run(["docker", "exec", "-i", "-w", "/workspace",
                                 "-e", f"HARNESS_EVIDENCE_DIR={stage}", state["container"],
                                 "timeout", "--signal=KILL", str(args.timeout_ms / 1000),
                                 "sh", "-lc", args.command], stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=args.timeout_ms / 1000 + 15)
        staged = json.loads(run(["docker", "exec", state["container"], "python3", "-c",
                                 manifest_script(), stage]).decode())
        artifacts = []
        if staged:
            destination.mkdir(parents=True)
            for item in staged:
                relative = Path(item["path"])
                if relative.is_absolute() or ".." in relative.parts:
                    raise ValueError("invalid evidence path")
                data = base64.b64decode(item["data_base64"], validate=True)
                if (len(data) != item["size_bytes"]
                        or hashlib.sha256(data).hexdigest() != item["sha256"]):
                    raise ValueError("evidence payload does not match its manifest")
                target = destination / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
                artifacts.append({"path": f"execution-evidence/{receipt_id}/{item['path']}",
                                  "sha256": item["sha256"], "size_bytes": item["size_bytes"]})
    except Exception:
        shutil.rmtree(destination, ignore_errors=True)
        raise
    finally:
        run(["docker", "exec", state["container"], "rm", "-rf", "--", stage])
    evidence = {"command": args.command, "timeout_ms": args.timeout_ms,
                "exit_code": result.returncode, "stdout": result.stdout.decode(errors="replace"),
                "stderr": result.stderr.decode(errors="replace"), "artifacts": artifacts}
    commands = args.root / "commands"
    commands.mkdir(exist_ok=True)
    write_json(commands / f"{receipt_id}.json", evidence)
    return {"exit_code": result.returncode, "stdout": evidence["stdout"][-60000:],
            "stderr": evidence["stderr"][-20000:], "artifacts": artifacts}


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
            # Visible in the delivery: what the subject left running.
            result["removed_containers"] = remove_other_stacks(state).decode().split()
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
