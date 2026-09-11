#!/usr/bin/env python3
"""Run-owned Git delivery and isolated application lifecycle for trending_topics_build."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import subprocess
import tempfile
import uuid

FIXTURE_URL = "git@github.com:iii-hq/e2e-fixture.git"
FIXTURE_SHA = "3ee24f7ace3c014db35423f14939ad3f6ce0c3d2"
BASELINE_SHA = "76a23abebff553a8ccf1397bd2f991c273bac03c"
APP_TREE = "78bced7344359876226012a786c45cec58649280"
REMOTE_URL = "git://127.0.0.1:9418/origin.git"
RUNTIME_ASSETS = ("acceptance.spec.mjs", "playwright.config.mjs", "evaluate.mjs", "package.json", "package-lock.json")
PROTECTED_INPUTS = ("content/feed.json", "package.json", "package-lock.json", ".npmrc", "playwright.config.js")
OUTPUT_LIMIT = 8 * 1024 * 1024


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def run(argv, *, cwd=None, timeout=120, check=True, log=None):
    # Bound both pipes on disk: a subject command cannot exhaust controller RAM.
    with tempfile.TemporaryDirectory(prefix="ttb-command-") as temporary:
        destination = log if log is not None else Path(temporary) / "command"
        destination.parent.mkdir(parents=True, exist_ok=True)
        stdout_path = destination.with_suffix(".stdout.log")
        stderr_path = destination.with_suffix(".stderr.log")
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            result = subprocess.run([str(arg) for arg in argv], cwd=cwd, stdout=stdout,
                                    stderr=stderr, timeout=timeout,
                                    env={**os.environ, "III_TELEMETRY_ENABLED": "false"},
                                    preexec_fn=lambda: resource.setrlimit(resource.RLIMIT_FSIZE, (OUTPUT_LIMIT, OUTPUT_LIMIT)))
        result.stdout = stdout_path.read_bytes()
        result.stderr = stderr_path.read_bytes()
        if check and result.returncode:
            raise RuntimeError(f"{argv[0]} exited {result.returncode}: " + result.stderr.decode(errors="replace")[-12000:])
        return result


def git(directory, *args, check=True):
    return run(["git", "--no-replace-objects", "-c", "core.hooksPath=/dev/null",
                "-c", "core.fsmonitor=false", "-C", directory, *args], check=check)


def mount(source, target, readonly=False):
    if "," in str(source):
        raise ValueError("Docker bind paths must not contain commas")
    return ["--mount", f"type=bind,src={source},dst={target}" + (",readonly" if readonly else "")]


def container_args(state, role, network="none"):
    return ["docker", "run", "--pull=never", "--init", "--name", f"ttb-{state['token']}-{role}",
            "--env", "III_TELEMETRY_ENABLED=false",
            "--label", f"harness.trending-topics.run={state['token']}",
            "--network", network, "--read-only", "--cap-drop=ALL",
            "--security-opt", "no-new-privileges", "--user", f"{os.getuid()}:{os.getgid()}",
            "--pids-limit", "256", "--memory", "2g", "--memory-swap", "2g", "--cpus", "2",
            "--shm-size", "256m", "--tmpfs", "/tmp:rw,nosuid,nodev,size=512m,mode=1777"]


def prepare(args):
    root = args.root
    root.mkdir(mode=0o700, parents=False, exist_ok=False)
    state = {"token": uuid.uuid4().hex, "fixture_revision": FIXTURE_SHA, "baseline_sha": BASELINE_SHA}
    write_json(root / "state.json", state)
    fixture = root / "fixture"
    git(root, "init", "--quiet", fixture)
    git(fixture, "remote", "add", "origin", FIXTURE_URL)
    git(fixture, "fetch", "--quiet", "--depth=1", "origin", FIXTURE_SHA)
    git(fixture, "checkout", "--quiet", "--detach", FIXTURE_SHA)
    exported = run(["node", fixture / "trending-topics-build/prepare.mjs", root / "remote"])
    receipt = json.loads(exported.stdout)
    if (receipt["fixture_revision"], receipt["initial_commit"], receipt["fixture_app_tree"]) != (FIXTURE_SHA, BASELINE_SHA, APP_TREE):
        raise ValueError("Exported fixture does not match the pinned baseline")
    state["baseline"] = receipt
    write_json(root / "state.json", state)

    context = root / "runtime-context"
    context.mkdir()
    public = fixture / "trending-topics-build/app"
    shutil.copyfile(args.assets / "Dockerfile", context / "Dockerfile")
    for filename in ("package.json", "package-lock.json"):
        shutil.copyfile(public / filename, context / filename)
    image_hash = hashlib.sha256(b"".join((context / name).read_bytes() for name in ("Dockerfile", "package.json", "package-lock.json"))).hexdigest()
    image_tag = f"harness-trending-topics:{image_hash}"
    if run(["docker", "image", "inspect", image_tag], check=False).returncode:
        run(["docker", "build", "--tag", image_tag, context], timeout=1800, log=root / "runtime-build")
    state["image"] = run(["docker", "image", "inspect", "--format", "{{.Id}}", image_tag]).stdout.decode().strip()
    state["runtime_context_sha256"] = image_hash
    write_json(root / "state.json", state)

    evaluator = root / "evaluator"
    evaluator.mkdir()
    for filename in RUNTIME_ASSETS:
        shutil.copyfile(args.assets / filename, evaluator / filename)
    (evaluator / "node_modules").symlink_to("/opt/public-deps/node_modules", target_is_directory=True)
    workspace = root / "workspace"
    workspace.mkdir()
    task = (public / "README.md").read_text() + f"\n## This attempt\n\nClone `{REMOTE_URL}` into `/workspace/app`, on branch `build`. The initial SHA is `{BASELINE_SHA}`. All commands execute inside your isolated container, starting at `/workspace`. Dependencies and Chromium are pre-provisioned; `npm ci` uses the frozen offline cache. Internet access is disabled. Read and modify only this task's workspace.\n"
    (workspace / "TASK.md").write_text(task)
    git_name = f"ttb-{state['token']}-git"
    run(container_args(state, "git") + ["-d"] + mount(root / "remote", "/remote") +
        [state["image"], "git", "daemon", "--reuseaddr", "--export-all", "--enable=receive-pack",
         "--listen=127.0.0.1", "--port=9418", "--base-path=/remote", "/remote/origin.git"])
    run(container_args(state, "subject", f"container:{git_name}") + ["-d"] + mount(workspace, "/workspace") +
        [state["image"], "sh", "-c", "cp -R /opt/npm-cache /tmp/npm-cache && sleep infinity"])
    state["subject"] = f"ttb-{state['token']}-subject"
    state["git_service"] = git_name
    write_json(root / "state.json", state)
    ready = run(["docker", "exec", state["subject"], "sh", "-c",
                 f"for n in $(seq 1 30); do git ls-remote {REMOTE_URL} refs/heads/build && exit 0; sleep 1; done; exit 1"], timeout=40)
    if ready.stdout.decode().split()[0] != BASELINE_SHA:
        raise ValueError("Git service advertises an unexpected initial SHA")
    return state


def execute(args):
    if not 1 <= args.timeout_ms <= 120000:
        raise ValueError("timeout_ms must be 1..=120000")
    state = json.loads((args.root / "state.json").read_text())
    command_id = uuid.uuid4().hex
    log = args.root / "commands" / command_id
    argv = ["docker", "exec", "-w", "/workspace", state["subject"], "timeout", "--signal=KILL",
            str(args.timeout_ms / 1000), "sh", "-lc", args.command]
    try:
        result = run(argv, timeout=args.timeout_ms / 1000 + 15, check=False, log=log)
        output = {"exit_code": result.returncode, "stdout": result.stdout.decode(errors="replace")[-64000:],
                  "stderr": result.stderr.decode(errors="replace")[-20000:]}
    except subprocess.TimeoutExpired:
        output = {"exit_code": 124, "stdout": "", "stderr": "Command transport exceeded its bounded deadline"}
    write_json(log.with_suffix(".json"), {"command": args.command, "timeout_ms": args.timeout_ms, **output})
    return output


def delivery_checks(remote, initial, delivered):
    reasons = []
    if git(remote, "merge-base", "--is-ancestor", initial, delivered, check=False).returncode:
        return ["Remote delivery does not descend from the baseline"]
    commits = git(remote, "rev-list", "--reverse", f"{initial}..{delivered}").stdout.decode().splitlines()
    if not commits:
        reasons.append("No implementation commit was delivered")
    for commit in commits:
        parents = git(remote, "rev-list", "--parents", "-n", "1", commit).stdout.decode().split()[1:]
        if len(parents) != 1:
            reasons.append(f"Merge commit is not permitted: {commit}")
            continue
        changes = git(remote, "diff", "--name-only", "-z", parents[0], commit).stdout.decode().split("\0")[:-1]
        if not changes:
            reasons.append(f"Empty commit is not permitted: {commit}")
        for name in changes:
            if name not in ("index.html", "DELIVERY.md") and not name.startswith(("src/", "public/", "tests/agent/")):
                reasons.append(f"Protected path changed in {commit}: {name}")
    return reasons


INSPECT_WORKSPACE = r'''
import json, os, subprocess
def git(*args, env=None):
    return subprocess.run(['git', '--no-replace-objects', '-c', 'core.fsmonitor=false', '-c', 'core.hooksPath=/dev/null', *args], env=env, capture_output=True)
root = '/subject/app'
result = {}
for key, args in [('head', ['rev-parse', '--verify', 'HEAD']), ('branch', ['symbolic-ref', '--short', 'HEAD']), ('origin', ['config', '--local', '--no-includes', '--get-all', 'remote.origin.url'])]:
    command = git('-C', root, *args)
    result[key] = command.stdout.decode(errors='replace').strip() if command.returncode == 0 else None
sha = os.environ['DELIVERED_SHA']
result['index_clean'] = git('-C', root, 'diff', '--cached', '--no-ext-diff', '--quiet', sha).returncode == 0
env = {**os.environ, 'GIT_INDEX_FILE': '/tmp/inspection-index'}
prefix = ['--git-dir=/verifier/.git', '--work-tree=' + root]
read = git(*prefix, 'read-tree', sha, env=env)
if read.returncode:
    raise RuntimeError(read.stderr.decode(errors='replace'))
result['tracked_clean'] = git(*prefix, 'diff', '--no-ext-diff', '--quiet', env=env).returncode == 0
untracked = git(*prefix, 'ls-files', '--others', '-z', '--exclude=/.git/', '--exclude=/node_modules/', '--exclude=/dist/', '--exclude=/test-results/', '--exclude=/playwright-report/', env=env)
if untracked.returncode:
    raise RuntimeError(untracked.stderr.decode(errors='replace'))
result['untracked'] = untracked.stdout.decode(errors='replace').split('\0')[:-1]
print(json.dumps(result))
'''


def inspect_workspace(root, state, delivered):
    result = run(container_args(state, "inspect") + ["--rm", "-e", f"DELIVERED_SHA={delivered}"] +
                 mount(root / "workspace", "/subject", True) + mount(root / "verifier", "/verifier", True) +
                 [state["image"], "python3", "-c", INSPECT_WORKSPACE])
    return json.loads(result.stdout)


def evaluate_app(args, state, dataset, delivered, feed):
    root = args.root
    app = root / f"application-{dataset}"
    shutil.copytree(root / "verifier", app, symlinks=True)
    if dataset == "varied":
        (app / "content/feed.json").write_bytes(feed.read_bytes())
    evidence = root / "evaluation" / dataset
    evidence.mkdir(parents=True)
    receipt = {"dataset": dataset, "commit": delivered, "feed_sha256": hashlib.sha256(feed.read_bytes()).hexdigest(),
               "report": str(evidence / "results.json")}
    build_name = f"ttb-{state['token']}-build-{dataset}"
    try:
        build = run(container_args(state, f"build-{dataset}") + ["--rm", "-w", "/workspace/app"] +
                    mount(app, "/workspace/app") + [state["image"], "sh", "-c",
                    "cp -R /opt/npm-cache /tmp/npm-cache && npm ci --offline && npm run build"],
                    timeout=300, check=False, log=evidence / "build")
        receipt["build_exit_code"] = build.returncode
    except subprocess.TimeoutExpired:
        run(["docker", "rm", "--force", build_name], check=False)
        receipt["build_exit_code"] = 124
        receipt["infrastructure_error"] = "Build container exceeded its bounded deadline"
        return receipt
    if build.returncode:
        field = "infrastructure_error" if build.returncode >= 125 else "product_error"
        receipt[field] = "Build container failed to execute" if field == "infrastructure_error" else "Frozen install or build failed"
        return receipt
    preview = f"ttb-{state['token']}-preview-{dataset}"
    try:
        launch = run(container_args(state, f"preview-{dataset}") + ["-d", "-w", "/workspace/app"] +
                     mount(app, "/workspace/app", True) + [state["image"], "npm", "run", "preview", "--", "--port", "4173"], check=False)
        receipt["preview_launch_exit_code"] = launch.returncode
        if launch.returncode:
            receipt["infrastructure_error"] = "Preview container failed to launch"
            return receipt
        ready = run(["docker", "exec", preview, "node", "--input-type=module", "-e",
                     "for(let i=0;i<100;i++){try{if((await fetch('http://127.0.0.1:4173/')).ok)process.exit(0)}catch{} await new Promise(r=>setTimeout(r,100));} process.exit(1);"],
                    timeout=20, check=False)
        if ready.returncode:
            field = "infrastructure_error" if ready.returncode >= 125 else "product_error"
            receipt[field] = "Preview readiness check could not execute" if field == "infrastructure_error" else "Application preview did not become ready"
            receipt["server_ready"] = False
            return receipt
        receipt["server_ready"] = True
        env = {"TT_BASE_URL": "http://127.0.0.1:4173", "TT_FEED_PATH": "/input/feed.json",
               "TT_OUTPUT_DIR": "/evidence", "TT_DATASET": dataset, "TT_COMMIT_SHA": delivered}
        argv = container_args(state, f"evaluate-{dataset}", f"container:{preview}") + ["--rm", "-w", "/evaluator"]
        for name, value in env.items():
            argv += ["-e", f"{name}={value}"]
        result = run(argv + mount(root / "evaluator", "/evaluator", True) + mount(feed, "/input/feed.json", True) +
                     mount(evidence, "/evidence") + [state["image"], "node",
                     "/opt/public-deps/node_modules/@playwright/test/cli.js", "test", "--config", "/evaluator/playwright.config.mjs"],
                     timeout=900, check=False, log=evidence / "playwright")
        receipt["test_exit_code"] = result.returncode
        if result.returncode >= 125:
            receipt["infrastructure_error"] = f"Playwright container exited {result.returncode}"
    except (RuntimeError, subprocess.TimeoutExpired) as error:
        if isinstance(error, subprocess.TimeoutExpired):
            run(["docker", "rm", "--force", f"ttb-{state['token']}-evaluate-{dataset}"], check=False)
        receipt["infrastructure_error"] = str(error)
        receipt["test_exit_code"] = 124
    finally:
        run(["docker", "logs", preview], check=False, log=evidence / "preview")
        run(["docker", "rm", "--force", preview], check=False)
    return receipt


def finish(args):
    root = args.root
    state = json.loads((root / "state.json").read_text())
    result = {"criteria": [], "complete": False, "infrastructure_errors": [], "delivery": {}, "evaluations": {}}
    write_json(root / "result.json", result)
    try:
        run(["docker", "stop", "--time", "2", state["subject"]])
        run(["docker", "stop", "--time", "2", state["git_service"]])
        remote = root / "remote/origin.git"
        delivered = git(remote, "rev-parse", "--verify", "refs/heads/build").stdout.decode().strip()
        result["delivery"] = {"remote_sha": delivered, "initial_sha": BASELINE_SHA, "fixture_revision": FIXTURE_SHA,
                              "remote_url": REMOTE_URL, "branch": "build", "runtime_image": state["image"]}
        run(["git", "clone", "--quiet", "--no-checkout", "--no-local", remote, root / "verifier"])
        git(root / "verifier", "checkout", "--quiet", "--detach", delivered)
        reasons = delivery_checks(remote, BASELINE_SHA, delivered)
        workspace = inspect_workspace(root, state, delivered)
        result["delivery"]["workspace"] = workspace
        if workspace["head"] != delivered or workspace["branch"] != "build" or workspace["origin"] != REMOTE_URL:
            reasons.append("Subject HEAD, branch or origin does not match the delivered remote")
        if not workspace["tracked_clean"] or not workspace["index_clean"] or workspace["untracked"]:
            reasons.append("Subject workspace has staged, unstaged or untracked implementation files")
        result["criteria"].append({"id": "B01", "status": "failed" if reasons else "passed", "reason": "; ".join(reasons) or "Remote delivery and subject workspace verified"})
        changed_inputs = [name for name in PROTECTED_INPUTS
                          if git(remote, "show", f"{BASELINE_SHA}:{name}").stdout != git(remote, "show", f"{delivered}:{name}", check=False).stdout]
        if changed_inputs:
            result["criteria"][0]["status"] = "failed"
            result["criteria"][0]["reason"] += "; Protected execution inputs changed: " + ", ".join(changed_inputs)
        write_json(root / "result.json", result)
        if not changed_inputs:
            feeds = root / "inputs"
            feeds.mkdir()
            original_feed = feeds / "original.json"
            original_feed.write_bytes(git(remote, "show", f"{BASELINE_SHA}:content/feed.json").stdout)
            varied_feed = feeds / "varied.json"
            run(["node", args.assets / "evaluate.mjs", "--feed-variant", original_feed, varied_feed])
            for dataset, feed in (("original", original_feed), ("varied", varied_feed)):
                result["evaluations"][dataset] = evaluate_app(args, state, dataset, delivered, feed)
                write_json(root / "result.json", result)
            builds = list(result["evaluations"].values())
            runtime_errors = [evaluation["infrastructure_error"] for evaluation in builds if "infrastructure_error" in evaluation]
            result["infrastructure_errors"].extend({"kind": "runtime_infrastructure", "message": error}
                                                   for error in runtime_errors)
            product_failures = [evaluation["product_error"] for evaluation in builds if "product_error" in evaluation]
            build_status = "failed" if product_failures else "unverified" if runtime_errors else "passed"
            result["criteria"].append({"id": "B02", "status": build_status,
                                        "reason": "Frozen install/build/start observations", "observations": builds})
            summary = json.loads(run(["node", args.assets / "evaluate.mjs", "--summarize",
                                      root / "evaluation/original/results.json", root / "evaluation/varied/results.json",
                                      original_feed, varied_feed,
                                      result["evaluations"]["original"].get("test_exit_code", -1),
                                      result["evaluations"]["varied"].get("test_exit_code", -1)]).stdout)
            if build_status == "failed":
                prerequisite = "; ".join(f"{dataset}: {evaluation['product_error']}"
                                         for dataset, evaluation in result["evaluations"].items()
                                         if "product_error" in evaluation)
                for criterion in summary["criteria"]:
                    if criterion["status"] == "unverified":
                        criterion["reason"] = "B02 failed; dependent acceptance was incomplete: " + prerequisite
            result["criteria"].extend(summary["criteria"])
            result["infrastructure_errors"].extend(summary["infrastructure_errors"])
            result["evidence"] = summary["evidence"]
            result["evidence_complete"] = summary["evidence_complete"]
            write_json(root / "result.json", result)
    except (RuntimeError, ValueError, OSError, KeyError, subprocess.TimeoutExpired) as error:
        result["infrastructure_errors"].append({"kind": "controller_error", "message": str(error)})
    observed = {item["id"] for item in result["criteria"]}
    for index in range(1, 11):
        criterion = f"B{index:02}"
        if criterion not in observed:
            result["criteria"].append({"id": criterion, "status": "unverified", "reason": "Prerequisite failed; dependent work was not evaluated"})
    result["complete"] = (not result["infrastructure_errors"] and result.get("evidence_complete", False)
                          and all(item["status"] == "passed" for item in result["criteria"]))
    write_json(root / "result.json", result)
    return result


def cleanup(args):
    state_path = args.root / "state.json"
    if not state_path.is_file():
        return {"cleanup_exit_code": 0, "removed": []}
    state = json.loads(state_path.read_text())
    token = state["token"]
    if len(token) != 32 or any(character not in "0123456789abcdef" for character in token):
        raise ValueError("Invalid attempt resource identity")
    containers = run(["docker", "ps", "-aq", "--filter", f"label=harness.trending-topics.run={token}"]).stdout.decode().splitlines()
    if containers:
        run(["docker", "rm", "--force", "--volumes", *containers])
    return {"cleanup_exit_code": 0, "removed": containers, "evidence_retained": str(args.root)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "exec", "finish", "cleanup"))
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--assets", type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument("--command")
    parser.add_argument("--timeout-ms", type=int, default=120000)
    args = parser.parse_args()
    if not args.root.is_absolute():
        parser.error("--root must be absolute")
    args.root = args.root.resolve()
    args.assets = args.assets.resolve()
    if args.action == "exec" and args.command is None:
        parser.error("exec requires --command")
    print(json.dumps({"prepare": prepare, "exec": execute, "finish": finish, "cleanup": cleanup}[args.action](args)))


if __name__ == "__main__":
    main()
