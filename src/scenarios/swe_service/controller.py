#!/usr/bin/env python3
"""Trusted, serialized Git checkpoints for the SWE service curriculum.

Only the runtime invokes this executable. State, probes, and the private object
store must be outside the subject's filesystem/tool scope. This process does not
claim to sandbox arbitrary Python: the runtime must isolate subject execution.
"""
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import uuid


MAX_BYTES = 64 * 1024 * 1024
MAX_FILES = 10000
SHA = re.compile(r"[0-9a-f]{40}\Z")
OPERATION_DEADLINE = None


class IntegrityError(Exception):
    """A candidate violates the delivery protocol."""


class ControllerInterrupted(RuntimeError):
    """A trusted runtime signal cancelled this controller invocation."""


def interrupted(signum, frame):
    raise ControllerInterrupted("Controller execution interrupted")


def run(args, cwd=None, timeout=120, allowed_codes=(0,)):
    if OPERATION_DEADLINE is not None:
        timeout = min(timeout, max(0.001, OPERATION_DEADLINE - time.monotonic()))
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_TERMINAL_PROMPT="0", GIT_NO_REPLACE_OBJECTS="1")
    process = subprocess.Popen(args, cwd=cwd, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, start_new_session=True)
    try:
        output, error = process.communicate(timeout=timeout)
    except BaseException:
        handlers = {sig: signal.signal(sig, signal.SIG_IGN)
                    for sig in (signal.SIGTERM, signal.SIGINT)}
        try:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.communicate(timeout=1)
            except subprocess.TimeoutExpired:
                pass
            finally:
                # Also kill surviving members if the group leader already exited.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            process.communicate(timeout=1)
        finally:
            for sig, handler in handlers.items():
                signal.signal(sig, handler)
        raise
    if process.returncode not in allowed_codes:
        raise RuntimeError("Trusted command failed")
    if len(output) > MAX_BYTES:
        raise RuntimeError("Command output exceeds evidence limit")
    return output


def git(repo, *args):
    return run(["git", "-c", "core.hooksPath=" + os.devnull,
                "-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false",
                "-c", "diff.external=", "-c", "core.attributesFile=" + os.devnull,
                "-C", str(repo), *args])


def digest(data):
    return hashlib.sha256(data).hexdigest()


def save(path, value):
    path = Path(path)
    fd, temporary = tempfile.mkstemp(prefix=".state-", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream, sort_keys=True)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


@contextmanager
def locked(path):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(str(path) + ".lock", "a") as stream:
        deadline = min(time.monotonic() + 240, OPERATION_DEADLINE or float("inf"))
        while True:
            try:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() > deadline:
                    raise RuntimeError("Checkpoint lock deadline exceeded")
                time.sleep(0.05)
        try:
            yield
        finally:
            fcntl.flock(stream, fcntl.LOCK_UN)


def allowed(name):
    return name.startswith(("src/", "docs/", "tests/agent/"))


def safe_name(name):
    parts = Path(name).parts
    return bool(parts) and not Path(name).is_absolute() and all(
        part not in ("..", ".git") for part in parts)


def files_in(root):
    """Read regular files without following symlinks; bound evidence size."""
    root = Path(root)
    result = {}
    total = 0
    for directory, dirs, files in os.walk(root, followlinks=False):
        if Path(directory) == root:
            dirs[:] = [d for d in dirs if d != ".git"]
        for name in dirs:
            if (Path(directory) / name).is_symlink():
                raise IntegrityError("Symbolic links are not permitted")
        for name in files:
            path = Path(directory) / name
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode):
                raise IntegrityError("Only regular files are permitted")
            total += info.st_size
            if total > MAX_BYTES or len(result) >= MAX_FILES:
                raise IntegrityError("Workspace exceeds size limit")
            relative = path.relative_to(root).as_posix()
            if not safe_name(relative):
                raise IntegrityError("Unsafe repository path")
            result[relative] = path.read_bytes()
    return result


def tree(repo, head):
    result = {}
    for record in git(repo, "ls-tree", "-rz", "--full-tree", head).split(b"\0"):
        if not record:
            continue
        meta, raw_name = record.split(b"\t", 1)
        mode, kind, obj = meta.decode().split()
        name = raw_name.decode("utf-8")
        if mode not in ("100644", "100755") or kind != "blob" or not safe_name(name):
            raise IntegrityError("Candidate contains an unsafe file type or path")
        result[name] = {"mode": mode, "oid": obj}
        if len(result) > MAX_FILES:
            raise IntegrityError("Candidate exceeds file limit")
    return result


def export(repo, head, destination):
    destination = Path(destination)
    entries = tree(repo, head)
    total = 0
    for name, entry in entries.items():
        data = git(repo, "cat-file", "blob", entry["oid"])
        total += len(data)
        if total > MAX_BYTES:
            raise IntegrityError("Candidate exceeds size limit")
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        path.chmod(0o755 if entry["mode"] == "100755" else 0o644)
    return entries


def owned_root(state):
    workspace = Path(state["workspace"])
    if workspace.is_symlink() or not workspace.is_dir():
        raise IntegrityError("Owned workspace is missing or replaced")
    info = workspace.stat()
    if [info.st_dev, info.st_ino] != state["workspace_identity"]:
        raise IntegrityError("Owned workspace has been replaced")
    return workspace


def owned_workspace(state):
    workspace = owned_root(state)
    dotgit = workspace / ".git"
    if dotgit.is_symlink() or not dotgit.is_dir():
        raise IntegrityError("Git directory has been replaced")
    marker = dotgit / "swe-controller-owner"
    if marker.is_symlink() or not marker.is_file() or marker.read_bytes() != state["ownership_token"].encode():
        raise IntegrityError("Workspace ownership marker is invalid")
    return workspace


def refs(repo):
    return git(repo, "for-each-ref", "--format=%(refname)").decode().splitlines()


def validate_live(state, head, entries=None):
    workspace = owned_workspace(state)
    dotgit = workspace / ".git"
    for forbidden in ("objects/info/alternates", "info/grafts", "shallow", "commondir"):
        if (dotgit / forbidden).exists() or (dotgit / forbidden).is_symlink():
            raise IntegrityError("Git history indirection is not permitted")
    for relative in ("config", "HEAD", "objects", "refs", "index"):
        if (dotgit / relative).is_symlink():
            raise IntegrityError("Git metadata indirection is not permitted")
    if digest((dotgit / "config").read_bytes()) != state["git_config_digest"]:
        raise IntegrityError("Git configuration was modified")
    if refs(workspace) != state["refs"]:
        raise IntegrityError("Repository references were added or removed")
    if (dotgit / "HEAD").read_bytes().strip() != ("ref: " + state["branch"]).encode():
        raise IntegrityError("Active Git branch was changed")
    if git(workspace, "rev-parse", "--verify", "HEAD").decode().strip() != head:
        raise IntegrityError("Submitted commit must be the current HEAD")
    if git(workspace, "status", "--porcelain=v1", "--untracked-files=all"):
        raise IntegrityError("Commit all changes before submitting a checkpoint")
    if any(line[:1] != b"H" for line in git(workspace, "ls-files", "-v").splitlines()):
        raise IntegrityError("Git index visibility flags are not permitted")
    if entries is not None:
        live = files_in(workspace)
        for name, entry in entries.items():
            path = workspace / name
            if name not in live or digest(live[name]) != entry["sha256"]:
                raise IntegrityError("Working files differ from the submitted commit")
            executable = bool(path.stat().st_mode & 0o111)
            if executable != (entry["mode"] == "100755"):
                raise IntegrityError("Working file modes differ from the submitted commit")


def probe(state, export_path, through, canary):
    for component in ("probes", "isolation"):
        if digest(Path(state[component]).read_bytes()) != state[component + "_digest"]:
            raise RuntimeError("Trusted verification component changed during this run")
    args = [sys.executable, "-I", state["isolation"], "--probes", state["probes"],
            "--workspace", str(export_path), "--through", str(through)]
    if canary:
        args.append("--canary")
    result = json.loads(run(args, cwd=state["assets"], timeout=150))
    if not isinstance(result, dict) or type(result.get("passed")) is not bool:
        raise RuntimeError("Trusted verifier returned an invalid result")
    # Raw check output is private evidence, never forwarded to subject/publisher.
    return result["passed"]


def public_ticket(state, number):
    ticket = state["tickets"][number - 1]
    return {key: ticket[key] for key in ("number", "id", "title", "prompt")}


def response(state, status, feedback, next_ticket=None):
    return {"status": status, "current_ticket": state["current_ticket"],
            "accepted_tickets": state["accepted_tickets"][:],
            "accepted_head": state["accepted_head"], "feedback": feedback,
            "next_ticket": next_ticket}


def prepare(args):
    if Path(args.workspace).is_symlink():
        raise RuntimeError("Workspace cannot be a symbolic link")
    workspace = Path(args.workspace).resolve()
    state_file = Path(args.state_file).resolve()
    fixture_root = Path(args.fixture_root).resolve()
    fixture = fixture_root / "swe-service"
    probes = Path(args.probes).resolve()
    isolation = Path(args.isolation).resolve()
    if not SHA.fullmatch(args.fixture_revision):
        raise RuntimeError("Fixture revision must be a full commit SHA")
    if not 1 <= args.ticket <= 8 or (args.mode == "journey" and args.ticket != 1):
        raise RuntimeError("Invalid initial ticket")
    if workspace.exists() and any(workspace.iterdir()):
        raise RuntimeError("Workspace must be empty")
    for protected in (state_file, fixture_root, probes, isolation):
        if protected == workspace or protected.is_relative_to(workspace) or workspace.is_relative_to(protected):
            raise RuntimeError("Trusted paths must be outside the subject workspace")
    if state_file.exists():
        raise RuntimeError("State already exists")
    if not probes.is_file() or not isolation.is_file():
        raise RuntimeError("Trusted verifier or isolation wrapper is missing")
    tickets = json.loads((fixture / "curriculum.json").read_text())["tickets"]
    if [ticket["number"] for ticket in tickets] != list(range(1, 9)):
        raise RuntimeError("Invalid curriculum")
    stage = 0 if args.mode == "journey" else args.ticket - 1
    snapshot = fixture / "snapshots" / f"{stage:02}"
    snapshot_files = files_in(snapshot)
    if not snapshot_files or (snapshot / ".git").exists():
        raise RuntimeError("Snapshot is empty or contains Git metadata")
    manifest_path = fixture / "manifest.json"
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text())
        entry = next(item for item in manifest["snapshots"] if item["stage"] == stage)
        actual = {name: digest(data) for name, data in snapshot_files.items()}
        if actual != entry["files"] or digest(json.dumps(actual, sort_keys=True, separators=(",", ":")).encode()) != entry["sha256"]:
            raise RuntimeError("Fixture snapshot digest mismatch")
    token = uuid.uuid4().hex
    assets = state_file.parent / ("swe-assets-" + token)
    assets.mkdir(mode=0o700)
    (assets / "owner").write_text(token)
    workspace.mkdir(parents=True, exist_ok=True)
    for name, data in snapshot_files.items():
        path = workspace / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        path.chmod(0o755 if (snapshot / name).stat().st_mode & 0o111 else 0o644)
    git(workspace, "init", "-q", "--initial-branch=main", "--template=")
    git(workspace, "config", "user.name", "SWE Developer")
    git(workspace, "config", "user.email", "developer@example.invalid")
    git(workspace, "add", "--force", "-A")
    git(workspace, "-c", "user.name=SWE Fixture", "-c", "user.email=fixture@example.invalid",
        "commit", "-qm", "Initial service snapshot")
    head = git(workspace, "rev-parse", "HEAD").decode().strip()
    (workspace / ".git/swe-controller-owner").write_text(token)
    objects = assets / "repository.git"
    git(assets, "init", "--bare", "-q", "--template=", str(objects))
    git(objects, "fetch", "--no-tags", str(workspace), head)
    git(objects, "update-ref", "refs/heads/accepted", head)
    initial_tree = tree(objects, head)
    info = workspace.stat()
    state = {"schema": "swe-service-state/v1", "workspace": str(workspace),
             "workspace_identity": [info.st_dev, info.st_ino], "ownership_token": token,
             "assets": str(assets), "objects": str(objects), "probes": str(probes),
             "isolation": str(isolation), "probes_digest": digest(probes.read_bytes()),
             "isolation_digest": digest(isolation.read_bytes()),
             "git_config_digest": digest((workspace / ".git/config").read_bytes()),
             "refs": refs(workspace), "branch": "refs/heads/main", "mode": args.mode,
             "fixture_revision": args.fixture_revision, "run_id": args.run_id or token,
             "initial_head": head, "accepted_head": head, "accepted_tickets": [],
             "current_ticket": args.ticket, "initial_ticket": args.ticket, "tickets": tickets,
             "protected": {name: entry for name, entry in initial_tree.items() if not allowed(name)},
             "started_at": time.time(), "checkpoints": [], "responses": {}, "rejections": 0,
             "canary_revealed": args.mode == "isolated" and args.ticket > 5,
             "revision_id": None, "terminal_status": None, "cleaned": False}
    save(state_file, state)
    return {"workspace": str(workspace), "initial_head": head, "current_ticket": args.ticket,
            "prompt": public_ticket(state, args.ticket)["prompt"], "ticket": public_ticket(state, args.ticket)}


def checkpoint(args, state):
    key = f"{args.ticket}:{args.head}:{args.revision_id or ''}"
    if key in state["responses"]:
        return state["responses"][key]
    if state["terminal_status"]:
        status = "completed" if state["terminal_status"] == "completed" else "capability_failure"
        return response(state, status, "This task is already closed.")
    if state.get("quiesced"):
        return response(state, "capability_failure", "This execution is quiesced; checkpoint acceptance is closed.")
    if args.ticket != state["current_ticket"]:
        return response(state, "rejected", "Submit the current ticket before advancing.")
    status = "rejected"
    canary_observation = None
    feedback = "Candidate does not satisfy the current or previously accepted service contracts."
    try:
        if args.revision_id is not None and (
                args.ticket != 5 or args.revision_id != state.get("revision_id")):
            raise IntegrityError("Revision acknowledgement is invalid for this run and ticket")
        if not SHA.fullmatch(args.head):
            raise IntegrityError("Checkpoint requires a full commit SHA")
        validate_live(state, args.head)
        objects = Path(state["objects"])
        git(objects, "fetch", "--no-tags", state["workspace"], args.head)
        if git(objects, "rev-parse", args.head + "^{commit}").decode().strip() != args.head:
            raise IntegrityError("Submitted object is not a commit")
        try:
            git(objects, "merge-base", "--is-ancestor", state["accepted_head"], args.head)
        except RuntimeError:
            raise IntegrityError("Candidate must retain the entire accepted commit history") from None
        if args.head == state["accepted_head"]:
            raise IntegrityError("Submit a new commit for this ticket")
        entries = tree(objects, args.head)
        protected = {name: entry for name, entry in entries.items() if not allowed(name)}
        if protected != state["protected"]:
            raise IntegrityError("Public tests and benchmark control files must remain unchanged")
        with tempfile.TemporaryDirectory(prefix="candidate-", dir=state["assets"]) as temporary:
            export(objects, args.head, temporary)
            immutable = files_in(temporary)
            if args.ticket == 8 and not immutable.get("docs/delivery.md", b"").strip():
                raise IntegrityError("Include a nonempty docs/delivery.md handoff document in the committed delivery")
            for name in entries:
                entries[name]["sha256"] = digest(immutable[name])
            validate_live(state, args.head, entries)
            passed = probe(state, temporary, args.ticket, state["canary_revealed"] and args.ticket >= 5)
            if passed and args.ticket == 5 and not state["canary_revealed"]:
                canary_observation = {"passed": probe(state, temporary, 5, True)}
            if files_in(temporary) != immutable:
                raise IntegrityError("Candidate modified its committed verification export")
            validate_live(state, args.head, entries)
        if passed:
            if args.ticket == 5 and not state["canary_revealed"]:
                state["canary_revealed"] = True
                state["revision_id"] = uuid.uuid4().hex
                status = "revision_required"
                feedback = state["tickets"][4].get("canary_prompt", "Preserve the legacy v1 client contract alongside v2.")
                feedback += " Validate this requirement, then resubmit with revision_id to acknowledge it (the same commit is allowed). This revision request does not count as a rejection."
            else:
                state["accepted_head"] = args.head
                state["accepted_tickets"].append(args.ticket)
                git(objects, "update-ref", "refs/heads/accepted", args.head)
                finished = state["mode"] == "isolated" or args.ticket == 8
                status = "completed" if finished else "accepted"
                if finished:
                    state["terminal_status"] = "completed"
                else:
                    state["current_ticket"] += 1
                    state["rejections"] = 0
                feedback = "Committed delivery accepted."
    except IntegrityError as error:
        feedback = str(error)
    if status == "rejected":
        state["rejections"] += 1
        if state["rejections"] >= 3:
            status = "capability_failure"
            state["terminal_status"] = status
            feedback += " Three checkpoint rejections reached; this task is closed."
    next_ticket = public_ticket(state, state["current_ticket"]) if status == "accepted" else None
    result = response(state, status, feedback, next_ticket)
    if status == "revision_required":
        result["revision_id"] = state["revision_id"]
        result["canary_observation"] = canary_observation
    state["checkpoints"].append({"ticket": args.ticket, "id": state["tickets"][args.ticket - 1]["id"],
                                 "head_sha": args.head, "accepted": status in ("accepted", "completed"),
                                 "attempt": 1 + sum(c["ticket"] == args.ticket for c in state["checkpoints"]),
                                 "feedback": feedback, "status": status})
    if status == "revision_required":
        state["checkpoints"][-1]["canary_observation"] = canary_observation
    state["responses"][key] = result
    save(args.state_file, state)
    return result


def copy_evidence(source, destination):
    """Copy untrusted files without dereferencing links or opening special files."""
    total = count = 0
    for directory, dirs, files in os.walk(source, followlinks=False):
        if Path(directory) == source:
            dirs[:] = [name for name in dirs if name != ".git"]
            files = [name for name in files if name != ".git"]
        for name in list(dirs) + files:
            path = Path(directory) / name
            target = destination / path.relative_to(source)
            info = path.lstat()
            target.parent.mkdir(parents=True, exist_ok=True)
            if stat.S_ISLNK(info.st_mode):
                target.symlink_to(os.readlink(path))
                if name in dirs:
                    dirs.remove(name)
            elif stat.S_ISDIR(info.st_mode):
                target.mkdir(exist_ok=True)
            elif stat.S_ISREG(info.st_mode):
                total += info.st_size
                count += 1
                if total > MAX_BYTES or count > MAX_FILES:
                    raise IntegrityError("Unaccepted evidence exceeds size limit")
                shutil.copyfile(path, target, follow_symlinks=False)
                target.chmod(info.st_mode & 0o777)
            else:
                target.write_text("[non-regular pending file]\n")


def unaccepted_patch(state):
    """Produce a private, applicable patch including binary/mode/untracked changes."""
    with tempfile.TemporaryDirectory(prefix="capture-", dir=state["assets"]) as temporary:
        root = Path(temporary)
        (root / "a").mkdir()
        (root / "b").mkdir()
        export(state["objects"], state["accepted_head"], root / "a")
        copy_evidence(owned_root(state), root / "b")
        patch = run(["git", "-c", "core.attributesFile=" + os.devnull, "diff", "--no-index",
                     "--no-prefix", "--no-ext-diff", "--no-textconv", "--binary", "--", "a", "b"],
                    cwd=root, allowed_codes=(0, 1)).decode("utf-8", "replace")
    return patch


def process_directories():
    """Find cwd from the operating system, never from subject-provided PID files."""
    result = {}
    if sys.platform.startswith("linux"):
        for entry in Path("/proc").iterdir():
            if entry.name.isdigit():
                try:
                    if entry.stat().st_uid == os.getuid():
                        result[int(entry.name)] = Path(os.readlink(entry / "cwd")).resolve()
                except OSError:
                    pass
    elif sys.platform == "darwin":
        executable = shutil.which("lsof") or "/usr/sbin/lsof"
        data = run([executable, "-a", "-u", str(os.getuid()), "-d", "cwd", "-Fpn"],
                   timeout=15, allowed_codes=(0, 1)).decode("utf-8", "replace")
        pid = None
        for line in data.splitlines():
            if line.startswith("p") and line[1:].isdigit():
                pid = int(line[1:])
            elif line.startswith("n") and pid is not None:
                result[pid] = Path(line[1:]).resolve()
    else:
        raise RuntimeError("Owned process cleanup requires Linux or macOS")
    return result


def stop_owned_processes(state):
    roots = (Path(state["workspace"]), Path(state["assets"]))
    def owned():
        return {pid for pid, cwd in process_directories().items()
                if pid not in (os.getpid(), os.getppid())
                and any(cwd == root or cwd.is_relative_to(root) for root in roots)}
    initial = owned()
    for pid in initial & owned():
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    if initial:
        deadline = time.monotonic() + 1
        remaining = owned()
        while remaining and time.monotonic() < deadline:
            time.sleep(0.05)
            remaining = owned()
        for pid in remaining:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 1
        while owned():
            if time.monotonic() >= deadline:
                raise RuntimeError("Owned application processes did not stop")
            time.sleep(0.05)
    return len(initial)


def owned_assets(args, state):
    assets = Path(state["assets"])
    if assets.is_symlink() or assets.parent != Path(args.state_file).resolve().parent or assets.name != "swe-assets-" + state["ownership_token"]:
        raise IntegrityError("Owned runtime assets path is invalid")
    if (assets / "owner").is_symlink() or (assets / "owner").read_text() != state["ownership_token"]:
        raise IntegrityError("Runtime assets ownership marker is invalid")
    return assets


def quiesce(args, state):
    if state.get("quiescence_receipt"):
        return state["quiescence_receipt"]
    if state.get("cleaned"):
        return {"quiesced": True, "stopped_processes": 0}
    owned_root(state)
    owned_assets(args, state)
    # Persist the acceptance freeze before shutdown, including interrupted shutdown.
    state["quiesced"] = True
    save(args.state_file, state)
    receipt = {"quiesced": True, "stopped_processes": stop_owned_processes(state)}
    state["quiescence_receipt"] = receipt
    save(args.state_file, state)
    return receipt


def capture(args, state):
    if state.get("cleaned"):
        return state["final_report"]
    terminal = state["terminal_status"] or getattr(args, "terminal_status", None) or "cancelled"
    report = {"schema": "swe-service-report/v1",
              "scenario_id": "swe_service_journey" if state["mode"] == "journey" else state["tickets"][state["initial_ticket"] - 1]["id"],
              "mode": state["mode"], "fixture_revision": state["fixture_revision"],
              "run_id": state["run_id"], "initial_head": state["initial_head"],
              "accepted_head": state["accepted_head"], "accepted_tickets": state["accepted_tickets"][:],
              "checkpoints": state["checkpoints"], "terminal_status": terminal,
              "terminal_ticket": state["current_ticket"],
              "elapsed_ms": max(0, int((time.time() - state["started_at"]) * 1000)),
              "accepted_patch": git(state["objects"], "diff", "--no-ext-diff", "--no-textconv", "--binary",
                                    state["initial_head"], state["accepted_head"], "--").decode("utf-8", "replace"),
              "unaccepted_patch": ""}
    try:
        report["unaccepted_patch"] = unaccepted_patch(state)
    except (IntegrityError, OSError, RuntimeError) as error:
        report["capture_error"] = str(error)
    state["final_report"] = report
    if getattr(args, "terminal_status", None):
        state["terminal_status"] = terminal
    save(args.state_file, state)
    save(str(args.state_file) + ".report.json", report)
    return report


def cleanup(args, state):
    if state.get("cleaned"):
        return {"cleaned": True, "report_file": str(args.state_file) + ".report.json"}
    workspace = owned_root(state)
    assets = owned_assets(args, state)
    stopped = stop_owned_processes(state)
    capture(args, state)
    # Evidence is persisted before removing either execution-owned directory.
    shutil.rmtree(workspace)
    shutil.rmtree(assets)
    state["cleaned"] = True
    save(args.state_file, state)
    return {"cleaned": True, "stopped_processes": stopped,
            "report_file": str(args.state_file) + ".report.json"}


def main():
    global OPERATION_DEADLINE
    OPERATION_DEADLINE = time.monotonic() + 240
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prep = sub.add_parser("prepare")
    for name in ("fixture-root", "workspace", "state-file", "probes", "isolation", "fixture-revision"):
        prep.add_argument("--" + name, required=True)
    prep.add_argument("--mode", choices=("journey", "isolated"), required=True)
    prep.add_argument("--ticket", type=int, required=True)
    prep.add_argument("--run-id")
    checkpoint_parser = sub.add_parser("checkpoint")
    checkpoint_parser.add_argument("--state-file", required=True)
    checkpoint_parser.add_argument("--ticket", type=int, required=True)
    checkpoint_parser.add_argument("--head", required=True)
    checkpoint_parser.add_argument("--revision-id")
    cap = sub.add_parser("capture")
    cap.add_argument("--state-file", required=True)
    cap.add_argument("--terminal-status", choices=("completed", "capability_failure", "resource_limit", "cancelled", "infrastructure_error"))
    clean = sub.add_parser("cleanup")
    clean.add_argument("--state-file", required=True)
    quiet = sub.add_parser("quiesce")
    quiet.add_argument("--state-file", required=True)
    args = parser.parse_args()
    previous_handlers = {sig: signal.signal(sig, interrupted)
                         for sig in (signal.SIGTERM, signal.SIGINT)}
    try:
        with locked(args.state_file):
            if args.command == "prepare":
                result = prepare(args)
            else:
                state = json.loads(Path(args.state_file).read_text())
                result = globals()[args.command](args, state)
        print(json.dumps(result))
    except (OSError, ValueError, KeyError, RuntimeError, IntegrityError, subprocess.TimeoutExpired) as error:
        print(json.dumps({"status": "infrastructure_error", "feedback": str(error)}))
        return 1
    finally:
        for sig, handler in previous_handlers.items():
            signal.signal(sig, handler)
    return 0


if __name__ == "__main__":
    sys.exit(main())
