#!/usr/bin/env python3
"""Turn one dispatch into the contracts every group of the execution runs.

A dispatch names a suite (what to test), a stack (where), a model and, when
wanted, an agent profile (with whom), plus the Release Control execution the
reports go to. Everything else is resolved here, once for the whole execution:

    dispatch   the inputs as one execution: `execution.json`, the requested
               `stack.yaml`, and `plan.json`, the plan shape older Console
               imports and `report_execution.py` read.
    runtime    `iii: latest` becomes the newest iii release candidate, with
               its archive digest, a template one commit, and every
               container pinned to a `commit:` its repository, folder, full
               commit and the dependency graph of its newest release.
    commits    each of those built at its commit, once per (repository,
               commit, folder) in a build cache, and declared as the
               path:// worker it is now, with what a package would have
               brought: its dependencies, default config and env.
    runner     the `harness-e2e` the stack declares, resolved by Compose and
               pinned in the stack, so the suite is materialized by the very
               runner every group runs. Prints that binary's path.
    contracts  one contract per materialized campaign and the group matrix.
    lock       the stack as Compose assembled it once, with its
               worker-compose.lock, into every contract. Each group starts it
               frozen, so all of them run the same versions.
    runner-binary  the runner a lock resolved, for the finalizer's aggregate.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from exact_stack_campaign import load_yaml, merged, worker_name  # noqa: E402


ROOT = Path(__file__).resolve().parents[1]
STACKS = ROOT / "stacks"
GITHUB_API_URL = os.environ.get("GITHUB_API_URL", "https://api.github.com")
REGISTRY_URL = "https://api.workers.iii.dev"
III_REPOSITORY = "iii-hq/iii"
TEMPLATES_REPOSITORY = "iii-hq/templates"

CONTRACT_SCHEMA = "rc-e2e/v2"
CLI_TARGET = "x86_64-unknown-linux-gnu"
CLI_ASSET = f"iii-{CLI_TARGET}.tar.gz"
#: Stack keys the executor reads; the rest of a stack is the Compose project.
EXECUTOR_KEYS = ("iii", "template")
#: The runner executing the scenarios inside the declared stack.
RUNNER = "harness-e2e"
#: A release candidate tag of iii-hq/iii, as Release Control's release grammar reads one.
RELEASE_CANDIDATE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)-rc\.([1-9]\d*)$")
SHA256 = re.compile(r"^(?:sha256:)?[0-9a-f]{64}$")
#: What a stack may write as `commit:`, short or full.
COMMIT = re.compile(r"^[0-9a-f]{4,40}$")
#: Where the Registry points a release: the repository it was built from.
RELEASE_ASSET = re.compile(r"^https://github\.com/([^/]+/[^/]+)/releases/download/")
#: A pinned container's keys that only say where its commit is.
PIN_KEYS = ("version", "commit", "repository", "path")


class ResolutionError(RuntimeError):
    """A dispatch the execution cannot be assembled from."""


def canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def get_json(url: str, token: str | None = None, body: Any = None) -> Any:
    """The JSON `url` answers; POSTed `body` when there is one."""
    headers = {"Accept": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers)
    # Only load shedding and gateway faults are worth another call; every other
    # answer is the service's considered one about this exact question.
    last: Exception | None = None
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return json.loads(response.read().decode())
        except urllib.error.HTTPError as error:
            if error.code != 429 and error.code < 500:
                raise ResolutionError(f"{url} answered HTTP {error.code}") from error
            last = error
        except (OSError, ValueError) as error:
            last = error
        time.sleep(0.5 * (attempt + 1))
    raise ResolutionError(f"{url} did not answer: {last}")


# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------


def load_stack(value: str) -> tuple[str, dict[str, Any]]:
    """A stack by name (`stacks/<name>.yaml`) or stated as YAML."""
    value = value.strip() or "default"
    if re.fullmatch(r"[A-Za-z0-9._-]+", value):
        name, stack = value, load_yaml((STACKS / f"{value}.yaml").read_text())
    else:
        name, stack = "inline", load_yaml(value)
    if not isinstance(stack, dict):
        raise ResolutionError("stack must name a file in stacks/ or be a YAML mapping")
    return name, stack


def compose_of(stack: dict[str, Any]) -> dict[str, Any]:
    """The Compose project a stack declares, without the executor's keys."""
    return {key: value for key, value in stack.items() if key not in EXECUTOR_KEYS}


def read_dispatch(inputs: dict[str, str]) -> dict[str, Any]:
    """The execution a dispatch asks for."""
    suite = (inputs.get("suite") or "").strip()
    stack_name, stack = load_stack(inputs.get("stack") or "default")
    execution = {
        "execution_id": (inputs.get("execution_id") or "").strip() or None,
        "suite": json.loads(suite) if suite.startswith("{") else suite,
        "stack": stack_name,
        "model": (inputs.get("model") or "").strip(),
        "profile": (inputs.get("profile") or "").strip() or None,
    }
    if not execution["suite"]:
        raise ResolutionError("suite is required: a suite id of config/test-plan.json or one suite as JSON")
    provider, _, model = execution["model"].partition("/")
    if not provider or not model:
        raise ResolutionError("model must be <provider>/<model>")
    suite = execution["suite"]
    # The plan shape older Console imports and the ledger reports read.
    plan = {
        "profile": {"id": suite["id"] if isinstance(suite, dict) else suite},
        "subject": {"provider": provider, "model": model},
        **({"agent_profile": execution["profile"]} if execution["profile"] else {}),
    }
    return {"execution": execution, "stack": stack, "plan": plan}


# ---------------------------------------------------------------------------
# Contracts
# ---------------------------------------------------------------------------


def newest_release_candidate(versions: list[str]) -> str | None:
    """What `iii: latest` means: the newest `X.Y.Z-rc.N` among the `iii/v*`
    tags, the rule Release Control dispatches with
    (`resolveNewestCliReleaseCandidate`). Stable releases and other
    pre-releases are not candidates; candidates order by core, then N."""
    ranked = [
        (tuple(int(part) for part in match.groups()), version)
        for version in versions
        if (match := RELEASE_CANDIDATE.fullmatch(version))
    ]
    return max(ranked)[1] if ranked else None


def resolve_cli(selector: str, token: str | None) -> dict[str, str]:
    """The iii release every group installs, with its archive's digest."""
    version = str(selector).strip()
    if version == "latest":
        refs = get_json(f"{GITHUB_API_URL}/repos/{III_REPOSITORY}/git/matching-refs/tags/iii/v", token=token)
        tags = [str(ref.get("ref", "")).removeprefix("refs/tags/iii/v") for ref in refs]
        version = newest_release_candidate(tags) or ""
        if not version:
            raise ResolutionError(f"{III_REPOSITORY} has no release candidate")
    release = get_json(f"{GITHUB_API_URL}/repos/{III_REPOSITORY}/releases/tags/iii/v{version}", token=token)
    for asset in release.get("assets") or []:
        if asset.get("name") == CLI_ASSET:
            digest = asset.get("digest")
            # The digest is what the groups check the download against.
            if not isinstance(digest, str) or not SHA256.fullmatch(digest):
                raise ResolutionError(f"iii/v{version} {CLI_ASSET} has no SHA-256 digest")
            digest = digest if digest.startswith("sha256:") else f"sha256:{digest}"
            return {"version": version, "target": CLI_TARGET, "asset": CLI_ASSET, "sha256": digest}
    raise ResolutionError(f"release iii/v{version} publishes no {CLI_ASSET}")


def resolve_template(value: Any, token: str | None) -> dict[str, str] | None:
    """`<id>` or `<id>@<revision>` of iii-hq/templates, pinned to one commit.

    Whether the template exists and what it declares, the groups learn from
    `iii project init` on that commit.
    """
    if not value:
        return None
    template_id, _, ref = str(value).partition("@")
    ref = ref or "main"
    revision = get_json(f"{GITHUB_API_URL}/repos/{TEMPLATES_REPOSITORY}/commits/{ref}", token=token).get("sha")
    if not isinstance(revision, str) or not revision:
        raise ResolutionError(f"{TEMPLATES_REPOSITORY}@{ref} did not resolve to a commit")
    return {"id": template_id, "repository": TEMPLATES_REPOSITORY, "ref": ref, "revision": revision}


def release_graph(reference: str) -> dict[str, Any]:
    """What the Registry resolves `<reference>@latest` to, as `compose::add`
    asks it: every worker of the graph, exact, and who calls whom."""
    host, _, name = reference.rpartition("/")
    url = f"https://{host}/resolve" if host else f"{REGISTRY_URL}/resolve"
    return get_json(url, body={"worker": name, "version": "latest", "target": CLI_TARGET})


def resolve_commits(stack: dict[str, Any], token: str | None) -> dict[str, dict[str, Any]]:
    """Every container the stack pins to a `commit:`, by container name.

    The repository is the one the Registry downloads the worker's newest
    release from, the folder the worker's name there, or the root of a
    repository named after it; `repository:` and `path:` only override. The
    commit resolves to its full sha, and one that does not exist is an error.
    Compose brings none of a path:// worker's dependencies, so that release's
    graph goes with the pin: its versions and edges, engine built-ins left out.
    """
    commits: dict[str, dict[str, Any]] = {}
    for name, container in (stack.get("containers") or {}).items():
        if not isinstance(container, dict) or container.get("commit") in (None, ""):
            continue
        source = str(container.get("worker", ""))
        requested = str(container["commit"]).strip().lower()
        if not source.startswith("package://"):
            raise ResolutionError(f"{name} pins a commit, but only a package:// worker is built from one")
        if not COMMIT.fullmatch(requested):
            raise ResolutionError(f"{name}: commit {requested!r} is not a commit sha")
        worker = worker_name(source)
        graph = release_graph(source.removeprefix("package://"))
        nodes = {node["name"]: node for node in graph.get("graph") or [] if isinstance(node, dict)}
        release = nodes.get(worker)
        if release is None:
            raise ResolutionError(f"the Registry has no release of {worker}")
        repository = str(container.get("repository") or "").strip("/")
        if not repository:
            artifact = (release.get("binaries") or {}).get(CLI_TARGET) or {}
            match = RELEASE_ASSET.match(artifact.get("url") or release.get("archive_url") or "")
            if not match:
                raise ResolutionError(f"{worker}'s release names no GitHub repository; state its `repository:`")
            repository = match[1]
        path = container.get("path")
        path = ("" if repository.rsplit("/", 1)[-1] == worker else worker) if path is None else str(path).strip("/")
        if ".." in Path(path).parts:
            raise ResolutionError(f"{name}: path {path!r} leaves the repository")
        try:
            commit = get_json(f"{GITHUB_API_URL}/repos/{repository}/commits/{requested}", token=token).get("sha")
        except ResolutionError as error:
            raise ResolutionError(f"{repository} has no commit {requested} ({error})") from error
        # A branch or tag named like a sha resolves too; only the commit counts.
        if not isinstance(commit, str) or not commit.startswith(requested):
            raise ResolutionError(f"{repository} has no commit {requested}")
        # GitHub serves a fork's commits through the repository it forked, so
        # a commit its default branch never had is said out loud.
        branch = get_json(f"{GITHUB_API_URL}/repos/{repository}", token=token).get("default_branch")
        compared = get_json(f"{GITHUB_API_URL}/repos/{repository}/compare/{branch}...{commit}?per_page=1", token=token)
        on_default_branch = compared.get("status") in ("identical", "behind")
        if not on_default_branch:
            print(f"::warning::{name} pins {repository}@{commit[:12]}, which is not on its default branch "
                  f"{branch}: it may come from a fork", flush=True)
        engine = {node for node, value in nodes.items() if value.get("type") == "engine"}
        commits[name] = {
            "worker": worker,
            "repository": repository,
            "path": path,
            "commit": commit,
            "on_default_branch": on_default_branch,
            "release": {
                "version": release.get("version"),
                "nodes": {node: value["version"] for node, value in nodes.items() if node not in engine},
                "edges": sorted({
                    (edge["from"], edge["to"]) for edge in graph.get("edges") or []
                    if edge["from"] not in engine and edge["to"] not in engine and edge["from"] != edge["to"]
                }),
            },
        }
    return commits


def build_worker(pin: dict[str, Any], cache: Path) -> Path:
    """The worker at a pinned commit, built with its own manifest once per
    (repository, commit, folder): a folder of the cache holding the binary
    under `bin/`, its `iii.worker.yaml`, its `config.yaml` if it has one, and
    `build.json`. Fetched anonymously and built without the token, outside
    the cache: a build runs the commit's code. The entry appears whole, by a
    rename; one another build completed first is kept."""
    built = cache / pin["repository"] / pin["commit"] / (pin["path"] or "_root")
    if (built / "build.json").is_file():
        print(f"{pin['worker']} @{pin['commit'][:12]}: built before, from the cache", file=sys.stderr)
        return built
    built.parent.mkdir(parents=True, exist_ok=True)
    # ponytail: an hour is far longer than copying a build takes; what is
    # older is what an interrupted build left.
    for stale in built.parent.glob(".build-*"):
        if time.time() - stale.stat().st_mtime > 3600:
            shutil.rmtree(stale, ignore_errors=True)
    environment = {key: value for key, value in os.environ.items() if key != "GITHUB_TOKEN"}
    with tempfile.TemporaryDirectory(prefix="harness-e2e-build-") as scratch:
        scratch = Path(scratch)
        source = scratch / "source"
        source.mkdir()
        for command in (["init", "-q"],
                        ["fetch", "-q", "--depth", "1", f"https://github.com/{pin['repository']}.git", pin["commit"]],
                        ["checkout", "-q", "--detach", "FETCH_HEAD"]):
            subprocess.run(["git", "-C", str(source), *command], env=environment, check=True, stdout=sys.stderr)
        folder = source / pin["path"]
        at = f"{pin['repository']}@{pin['commit'][:12]}"
        if not (folder / "iii.worker.yaml").is_file():
            raise ResolutionError(f"{at} has no {pin['path'] or '.'}/iii.worker.yaml")
        manifest = load_yaml((folder / "iii.worker.yaml").read_text()) or {}
        # ponytail: Rust binaries only, what every worker of the Registry is today.
        if manifest.get("language") != "rust":
            raise ResolutionError(f"{pin['worker']} at {at} is not a Rust worker; only those are built from a commit")
        binary = str(manifest.get("bin") or pin["worker"])
        # From the worker's folder, so the repository's rust-toolchain.toml applies.
        subprocess.run(
            ["cargo", "build", "--release", "--locked", "--bin", binary,
             "--manifest-path", str(folder / str(manifest.get("manifest") or "Cargo.toml")),
             "--target-dir", str(scratch / "target")],
            cwd=folder, env=environment, check=True, stdout=sys.stderr,
        )
        # Beside the entry, then renamed into place: the cache may be another
        # filesystem, and a reader never sees half an entry.
        staging = Path(tempfile.mkdtemp(dir=built.parent, prefix=".build-"))
        staging.chmod(0o755)
        (staging / "bin").mkdir()
        shutil.copy2(scratch / "target/release" / binary, staging / "bin" / binary)
        for name in ("iii.worker.yaml", "config.yaml"):
            if (folder / name).is_file():
                shutil.copy2(folder / name, staging / name)
        digest = hashlib.sha256((staging / "bin" / binary).read_bytes()).hexdigest()
        (staging / "build.json").write_text(json.dumps({
            "worker": pin["worker"], "repository": pin["repository"], "path": pin["path"],
            "commit": pin["commit"], "bin": binary, "sha256": f"sha256:{digest}",
        }, indent=2) + "\n")
        try:
            staging.rename(built)
        except OSError:
            shutil.rmtree(staging, ignore_errors=True)
            # Another execution built the same commit first: its entry stands.
            if not (built / "build.json").is_file():
                raise
    return built


def declare_commits(containers: dict[str, Any], commits: dict[str, dict[str, Any]], folders: dict[str, Path]) -> None:
    """Each pinned container becomes the path:// worker its build is.

    Compose runs a path:// worker as the operator's: without the manifest's
    dependencies, the default config the Registry would ship, or its env, and
    from the worker's folder. So it is declared with that config under the
    stack's own `config_override`, that env under its `environment`, the
    compose file's folder to run in, as a package does, and every dependency
    of the newest release the stack does not declare itself, at that
    release's exact version and edges. Those are recorded on the pin as
    `dependencies`: held, never asked for, so `compose::add` keeps their pins.
    """
    by_package = {
        worker_name(container["worker"]): name for name, container in containers.items()
        if isinstance(container, dict) and str(container.get("worker", "")).startswith("package://")
    }
    for name, pin in commits.items():
        folder = folders[name]
        build = json.loads((folder / "build.json").read_text())
        manifest = load_yaml((folder / "iii.worker.yaml").read_text()) or {}
        shipped = manifest.get("config")
        if shipped is None and (folder / "config.yaml").is_file():
            shipped = load_yaml((folder / "config.yaml").read_text())
        container = {key: value for key, value in containers[name].items() if key not in ("worker", *PIN_KEYS)}
        container["worker"] = f"path://{folder}"
        container["scripts"] = {**(container.get("scripts") or {}),
                                "run": f"exec {shlex.quote(str(folder / 'bin' / build['bin']))}"}
        container.setdefault("working_dir", ".")
        if isinstance(shipped, dict) and shipped:
            container["config_override"] = merged(shipped, container.get("config_override") or {})
        if isinstance(manifest.get("env"), dict) and manifest["env"]:
            container["environment"] = {**manifest["env"], **(container.get("environment") or {})}
        # What a template's own declaration of the worker gets under its values.
        pin["config"] = shipped if isinstance(shipped, dict) else {}
        pin["env"] = manifest["env"] if isinstance(manifest.get("env"), dict) else {}

        edges = [tuple(edge) for edge in pin["release"]["edges"]]

        def needs(node: str) -> list[str]:
            return sorted({by_package.get(to, to) for source, to in edges if source == node})

        reachable, visit = set(), [pin["worker"]]
        while visit:
            node = visit.pop()
            if node not in reachable:
                reachable.add(node)
                visit.extend(to for source, to in edges if source == node)
        held = []
        for dependency in sorted(reachable - {pin["worker"]} - set(by_package)):
            if dependency in containers:
                raise ResolutionError(f"container {dependency} is not the {dependency} worker {pin['worker']} depends on")
            containers[dependency] = {"worker": f"package://{dependency}", "version": pin["release"]["nodes"][dependency]}
            by_package[dependency] = dependency
            held.append(dependency)
        for dependency in held:
            if needs(dependency):
                containers[dependency]["start_after"] = needs(dependency)
        start_after = sorted({*(container.get("start_after") or []), *needs(pin["worker"])})
        if start_after:
            container["start_after"] = start_after
        containers[name] = container
        pin["dependencies"] = held


def download(url: str, sha256: str, destination: Path) -> Path:
    """Fetch `url` and check it against the digest the release or lock states.
    Tried three times, as the groups' `curl --retry 3`; a digest that does not
    match is an answer, not a blip, and is never retried."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    for attempt in range(3):
        try:
            with urllib.request.urlopen(url, timeout=300) as response:
                payload = response.read()
            break
        except urllib.error.HTTPError as error:
            if error.code != 429 and error.code < 500 or attempt == 2:
                raise
        except OSError:
            if attempt == 2:
                raise
        time.sleep(5 * (attempt + 1))
    observed = hashlib.sha256(payload).hexdigest()
    if observed != sha256.removeprefix("sha256:"):
        raise ResolutionError(f"{url} does not match its digest {sha256}")
    destination.write_bytes(payload)
    return destination


def install_cli(cli: dict[str, str], work_dir: Path) -> Path:
    archive = download(
        f"https://github.com/{III_REPOSITORY}/releases/download/iii/v{cli['version']}/{cli['asset']}",
        cli["sha256"],
        work_dir / cli["asset"],
    )
    with tarfile.open(archive) as bundle:
        bundle.extractall(work_dir / "bin", filter="data")
    return work_dir / "bin" / "iii"


def runner_declaration(stack: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    """The container that runs `harness-e2e` in a stack, or the one the
    scaffold would add when the stack declares none."""
    for name, container in (stack.get("containers") or {}).items():
        source = str(container.get("worker", ""))
        if source.startswith("package://") and worker_name(source) == RUNNER:
            return name, container
    return RUNNER, {"worker": f"package://{RUNNER}", "version": "latest"}


def fetch_runner(lock: dict[str, Any], work_dir: Path) -> Path:
    """The runner a lock resolved: its Linux artifact, checked against the
    lock's digest and unpacked."""
    entry = next(
        (entry for entry in (lock.get("containers") or {}).values() if worker_name(entry.get("worker", "")) == RUNNER),
        None,
    )
    if entry is None:
        raise ResolutionError(f"the lock resolves no {RUNNER}")
    artifact = entry["resolved"]["artifacts"][CLI_TARGET]
    archive = download(artifact["url"], artifact["sha256"], work_dir / f"{RUNNER}.tar.gz")
    target = work_dir / RUNNER
    with tarfile.open(archive) as bundle:
        bundle.extractall(target, filter="data")
    binary = next((path for path in sorted(target.rglob(RUNNER)) if path.is_file()), None)
    if binary is None:
        raise ResolutionError(f"{artifact['url']} holds no {RUNNER} executable")
    binary.chmod(0o755)
    return binary


def built_runner(contract_dir: Path) -> Path | None:
    """The runner the stack pinned to a commit, as `commits` built it into
    the contract, or None when the stack runs a release of it."""
    execution = contract_dir / "execution.json"
    commits = (json.loads(execution.read_text()).get("commits") or {}) if execution.is_file() else {}
    for name, pin in commits.items():
        if pin["worker"] == RUNNER:
            folder = contract_dir / "workers" / name
            binary = folder / "bin" / json.loads((folder / "build.json").read_text())["bin"]
            # Artifacts lose the executable bit on their way to another job.
            binary.chmod(0o755)
            return binary
    return None


def seal(body: dict[str, Any]) -> dict[str, Any]:
    digest = hashlib.sha256(canonical({**body, "idempotency_key": ""}).encode()).hexdigest()
    return {**body, "idempotency_key": f"rc:e2e:{digest}"}


def suite_groups(campaign: dict[str, Any]) -> list[dict[str, Any]]:
    """A materialized campaign's groups, in the shape the contract states them."""
    return [
        {
            "id": group["id"],
            "execution_kind": group["execution_kind"],
            "runs": group["runs"],
            "technical_retries": group["technical_retries"],
            "scenarios": list(group["scenarios"]),
        }
        for group in campaign.get("groups") or []
    ]


def build_contract(
    campaign: dict[str, Any],
    *,
    execution_key: str,
    snapshot: dict[str, Any],
    execution: dict[str, Any],
    cli: dict[str, str],
    compose: dict[str, Any],
    oidc_audience: str,
    template: dict[str, str] | None = None,
    lock: dict[str, Any] | None = None,
    commits: dict[str, dict[str, Any]] | None = None,
) -> dict[str, Any]:
    provider, _, model = execution["model"].partition("/")
    suite = {
        "id": campaign["campaign_id"],
        "label": f"{snapshot['profile']['label']} · {campaign['campaign_id']}",
        "lane": campaign["lane"],
        # Absent: each scenario keeps the canonical seed the suite materialized
        # it with, so the same slot is the same slot across executions.
        "seed": None,
        "subject": {"provider": provider, "model": model},
        "groups": suite_groups(campaign),
    }
    if execution.get("profile"):
        suite["agent_profile"] = execution["profile"]
    runtime: dict[str, Any] = {"cli": cli, "compose": compose}
    if template:
        runtime["template"] = template
    if lock:
        runtime["lock"] = lock
    if commits:
        # Which containers are built from a commit and what each one holds:
        # the scaffold knows a path:// worker by it, the launcher asks for
        # none of its dependencies, and the Console names its commit.
        runtime["commits"] = {
            name: {key: pin.get(key) for key in (
                "worker", "repository", "path", "commit", "on_default_branch", "dependencies", "config", "env")}
            for name, pin in commits.items()
        }
    return seal(
        {
            "schema": CONTRACT_SCHEMA,
            "campaign_id": execution_key,
            "execution_id": execution_key,
            "attempt": 1,
            "runtime": runtime,
            "security": {"oidc_audience": oidc_audience},
            "suite": suite,
        }
    )


def stack_versions(lock: dict[str, Any]) -> dict[str, str]:
    """Every worker the lock resolved, by the name it is known by."""
    versions = {}
    for entry in (lock.get("containers") or {}).values():
        name = str(entry.get("worker", "")).removeprefix("package://").rsplit("/", 1)[-1]
        versions[name] = str((entry.get("resolved") or {}).get("version"))
    return dict(sorted(versions.items()))


def stack_commits(commits: dict[str, dict[str, Any]]) -> dict[str, dict[str, Any]]:
    """Every worker built from a commit, by the name it is known by: the
    commit, where it was built from, and whether its default branch has it."""
    return {
        pin["worker"]: {key: pin.get(key) for key in ("commit", "repository", "path", "on_default_branch")}
        for pin in sorted(commits.values(), key=lambda pin: pin["worker"])
    }


def builds_key(commits: dict[str, dict[str, Any]]) -> str | None:
    """The build cache entry of an execution: its pins, whatever the
    containers are named. Only executions pinning the same commits share
    one, so a build can only ever reach its own."""
    pins = sorted(canonical({key: pin[key] for key in ("repository", "commit", "path")}) for pin in commits.values())
    return f"worker-builds-{hashlib.sha256(canonical(pins).encode()).hexdigest()[:32]}" if pins else None


def contract_paths(directory: Path) -> list[Path]:
    return sorted(path for path in (directory / "contracts").glob("*.json") if path.name != "resolution.json")


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def command_dispatch(args: argparse.Namespace) -> None:
    import yaml

    names = ("suite", "stack", "model", "profile", "execution_id")
    dispatch = read_dispatch({name: os.environ.get(f"DISPATCH_{name.upper()}", "") for name in names})
    write_json(args.contract_dir / "execution.json", dispatch["execution"])
    write_json(args.contract_dir / "plan.json", dispatch["plan"])
    (args.contract_dir / "stack.yaml").write_text(yaml.safe_dump(dispatch["stack"], sort_keys=False))


def command_runtime(args: argparse.Namespace) -> None:
    token = os.environ.get("GITHUB_TOKEN") or None
    execution = json.loads((args.contract_dir / "execution.json").read_text())
    stack = load_yaml((args.contract_dir / "stack.yaml").read_text())
    cli = resolve_cli(stack.get("iii") or "latest", token)
    commits = resolve_commits(stack, token)
    execution.update(iii=cli["version"], cli=cli, template=resolve_template(stack.get("template"), token),
                     commits=commits, builds=builds_key(commits))
    write_json(args.contract_dir / "execution.json", execution)


def command_commits(args: argparse.Namespace) -> None:
    """Build every pinned worker and declare it in the stack. Each build goes
    into the contract (`workers/<container>/`), which the groups receive."""
    import yaml

    directory = args.contract_dir
    execution = json.loads((directory / "execution.json").read_text())
    commits = execution.get("commits") or {}
    if not commits:
        return
    stack = load_yaml((directory / "stack.yaml").read_text())
    folders = {}
    for name, pin in commits.items():
        built = build_worker(pin, args.cache_dir.resolve())
        folders[name] = (directory / "workers" / name).resolve()
        shutil.rmtree(folders[name], ignore_errors=True)
        shutil.copytree(built, folders[name])
    declare_commits(stack["containers"], commits, folders)
    (directory / "stack.yaml").write_text(yaml.safe_dump(stack, sort_keys=False))
    write_json(directory / "execution.json", execution)


def command_runner(args: argparse.Namespace) -> None:
    """Resolve the stack's runner the way Compose will, pin it, fetch it."""
    import yaml

    directory, work = args.contract_dir, args.work_dir
    execution = json.loads((directory / "execution.json").read_text())
    binary = built_runner(directory)
    if binary is not None:
        pin = next(pin for pin in execution["commits"].values() if pin["worker"] == RUNNER)
        return report_runner(directory, execution, binary, pin["commit"])
    stack = load_yaml((directory / "stack.yaml").read_text())
    iii = install_cli(execution["cli"], work)
    name, container = runner_declaration(stack)
    compose = work / "worker-compose.yaml"
    compose.write_text(yaml.safe_dump({"containers": {name: {
        "worker": container["worker"], **({"version": container["version"]} if "version" in container else {}),
    }}}, sort_keys=False))
    subprocess.run(
        [str(iii), "compose", "build", "-f", str(compose)],
        env={**os.environ, "III_COMPOSE_STATE_DIR": str(work / "state"), "NO_COLOR": "1"},
        check=True, stdout=sys.stderr,
    )
    lock = load_yaml((work / "worker-compose.lock").read_text())
    version = lock["containers"][name]["resolved"]["version"]
    # The assembly then resolves this exact release, whatever `latest` means
    # by the time it runs.
    stack.setdefault("containers", {})[name] = {**container, "version": version}
    (directory / "stack.yaml").write_text(yaml.safe_dump(stack, sort_keys=False))
    report_runner(directory, execution, fetch_runner(lock, work), version)


def report_runner(directory: Path, execution: dict[str, Any], binary: Path, version: str) -> None:
    """Record the runner's identity and print its path."""
    catalog = subprocess.run([str(binary), "catalog"], capture_output=True, text=True, check=True).stdout
    identity = {"name": RUNNER, "version": version, **(json.loads(catalog).get("runner") or {})}
    write_json(directory / "runner.json", identity)
    # What the reports name as the runner: the commit it was built from.
    execution["runner_revision"] = identity.get("revision") or version
    write_json(directory / "execution.json", execution)
    print(binary)


def command_contracts(args: argparse.Namespace) -> None:
    directory = args.contract_dir
    execution = json.loads((directory / "execution.json").read_text())
    snapshot = json.loads((directory / "suite.json").read_text())
    stack = load_yaml((directory / "stack.yaml").read_text())
    cli, template = execution["cli"], execution.get("template")

    include = []
    for campaign in snapshot["campaigns"]:
        contract = build_contract(
            campaign,
            execution_key=args.execution_key,
            snapshot=snapshot,
            execution=execution,
            cli=cli,
            compose=compose_of(stack),
            oidc_audience=args.oidc_audience,
            template=template,
            commits=execution.get("commits"),
        )
        write_json(directory / "contracts" / f"{campaign['campaign_id']}.json", contract)
        for group in contract["suite"]["groups"]:
            include.append(
                {
                    "campaign_id": campaign["campaign_id"],
                    "group_id": group["id"],
                    "execution_kind": group["execution_kind"],
                    "runs_on": ["ubuntu-latest"],
                    **({"template_revision": template["revision"]} if template else {}),
                }
            )
    # The workflow and report_execution.py read this by name.
    image = os.environ.get("HARNESS_E2E_EXECUTOR_IMAGE")
    write_json(
        directory / "contracts" / "resolution.json",
        {
            "matrix": {"include": include},
            "cli_version": cli["version"],
            "campaign_ids": [campaign["campaign_id"] for campaign in snapshot["campaigns"]],
            **({"template": template} if template else {}),
            # What a worker built from a commit reports is its Cargo version;
            # the reports name it by the commit.
            **({"stack_commits": stack_commits(execution["commits"])} if execution.get("commits") else {}),
            # The executor image the execution was prepared in, as
            # scripts/run_in_image.sh resolved it.
            **({"executor_image": image} if image else {}),
        },
    )


def command_lock(args: argparse.Namespace) -> None:
    import yaml

    directory = args.contract_dir
    compose = load_yaml((args.assembled / "worker-compose.yaml").read_text())
    lock_text = (args.assembled / "worker-compose.lock").read_text()
    lock = load_yaml(lock_text)
    for path in contract_paths(directory):
        contract = json.loads(path.read_text())
        contract.pop("idempotency_key", None)
        contract["runtime"].update(compose=compose, lock=lock)
        write_json(path, seal(contract))
    resolution = json.loads((directory / "contracts" / "resolution.json").read_text())
    resolution["stack_versions"] = stack_versions(lock)
    write_json(directory / "contracts" / "resolution.json", resolution)
    # The stack as it ran, and its lock byte for byte, beside the contracts
    # for whoever reads the execution later.
    execution = json.loads((directory / "execution.json").read_text())
    template = execution.get("template")
    head = {"iii": execution["iii"]}
    if template:
        head["template"] = f"{template['id']}@{template['revision']}"
    (directory / "stack.yaml").write_text(yaml.safe_dump({**head, **compose}, sort_keys=False))
    (directory / "worker-compose.lock").write_text(lock_text)


def command_runner_binary(args: argparse.Namespace) -> None:
    # The lock sits in the contract, beside the runner built from a commit.
    print(built_runner(args.lock.parent) or fetch_runner(load_yaml(args.lock.read_text()), args.work_dir))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    dispatch = commands.add_parser("dispatch", help="read the DISPATCH_* inputs")
    runtime = commands.add_parser("runtime")
    commits = commands.add_parser("commits")
    commits.add_argument("--cache-dir", type=Path, required=True, help="builds by repository/commit/folder")
    runner = commands.add_parser("runner")
    runner.add_argument("--work-dir", type=Path, required=True)
    contracts = commands.add_parser("contracts")
    contracts.add_argument("--execution-key", required=True, help="the execution id, or the run id without one")
    contracts.add_argument("--oidc-audience", required=True)
    lock = commands.add_parser("lock")
    lock.add_argument("--assembled", type=Path, required=True, help="directory with the assembled worker-compose.{yaml,lock}")
    for command in (dispatch, runtime, commits, runner, contracts, lock):
        command.add_argument("--contract-dir", type=Path, required=True)
    runner_binary = commands.add_parser("runner-binary")
    runner_binary.add_argument("--lock", type=Path, required=True)
    runner_binary.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args()
    handlers = {
        "dispatch": command_dispatch, "runtime": command_runtime, "commits": command_commits,
        "runner": command_runner,
        "contracts": command_contracts, "lock": command_lock, "runner-binary": command_runner_binary,
    }
    try:
        handlers[args.command](args)
    except (ResolutionError, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
