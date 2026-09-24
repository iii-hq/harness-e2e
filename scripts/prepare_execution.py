#!/usr/bin/env python3
"""Turn one dispatch into the contracts every group of the execution runs.

A dispatch names a suite (what to test), a stack (where), a model and, when
wanted, an agent profile (with whom), plus the Release Control execution the
reports go to. Everything else is resolved here, once for the whole execution:

    dispatch   the inputs as one execution: `execution.json`, the requested
               `stack.yaml`, and `plan.json`, the plan shape the Console import
               and `report_execution.py` read. Release Control's older inputs
               (plan, stack policy, runner_sha, cli_version) are translated
               first, so there is one path after this.
    runtime    `iii: latest` becomes the newest iii release candidate, with
               its archive digest, and a template one commit.
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
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from exact_stack_campaign import load_yaml, worker_name  # noqa: E402


ROOT = Path(__file__).resolve().parents[1]
STACKS = ROOT / "stacks"
GITHUB_API_URL = os.environ.get("GITHUB_API_URL", "https://api.github.com")
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


class ResolutionError(RuntimeError):
    """A dispatch the execution cannot be assembled from."""


def canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def get_json(url: str, token: str | None = None) -> Any:
    headers = {"Accept": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, headers=headers)
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


def translate_legacy(inputs: dict[str, str]) -> tuple[dict[str, Any], dict[str, Any]]:
    """Release Control's older dispatch as the execution it stands for.

    The suite is the plan's profile, the model its subject, the profile its
    agent. The stack is the default one with the policy's versions on the
    workers it declares, the plan's runner release as the runner's version
    unless the policy pins it, the plan's template, and `cli_version` as iii.
    A pinned worker the default stack does not declare (Canvas, the subject's
    provider, a template's package) is declared with its pin: a pin is never
    dropped. The applied pins travel as `stack_overrides`.
    """
    plan = json.loads(inputs["plan"])
    policy = json.loads(inputs.get("stack") or "{}")
    _, stack = load_stack("default")
    pins = {str(worker): str(version) for worker, version in ((policy.get("versions") or {}).items())}
    runner = (plan.get("runner") or {}).get("version")
    if runner:
        pins.setdefault(RUNNER, str(runner))
    containers = stack.setdefault("containers", {})
    left = dict(pins)
    for container in containers.values():
        package = worker_name(container.get("worker", ""))
        if package in left:
            container["version"] = left.pop(package)
    for worker, version in sorted(left.items()):
        if worker in containers:
            raise ResolutionError(f"container {worker} is already used by another worker; cannot pin {worker}@{version}")
        containers[worker] = {"worker": f"package://{worker}", "version": version}
    head = {"iii": inputs.get("cli_version") or "latest"}
    if plan.get("template"):
        head["template"] = plan["template"]
    subject = plan.get("subject") or {}
    execution = {
        "execution_id": inputs.get("execution_id") or None,
        "suite": (plan.get("profile") or {}).get("id"),
        "stack": "default",
        "model": f"{subject['provider']}/{subject['model']}" if subject.get("provider") and subject.get("model") else "",
        "profile": plan.get("agent_profile"),
        "stack_overrides": dict(sorted(pins.items())),
    }
    return {"execution": execution, "stack": {**head, **compose_of(stack)}}, plan


def read_dispatch(inputs: dict[str, str]) -> dict[str, Any]:
    """The execution a dispatch asks for, from the new inputs or the older ones."""
    if (inputs.get("plan") or "").strip():
        translated, plan = translate_legacy(inputs)
        execution, stack = translated["execution"], translated["stack"]
    else:
        suite = (inputs.get("suite") or "").strip()
        stack_name, stack = load_stack(inputs.get("stack") or "default")
        execution = {
            "execution_id": (inputs.get("execution_id") or "").strip() or None,
            "suite": json.loads(suite) if suite.startswith("{") else suite,
            "stack": stack_name,
            "model": (inputs.get("model") or "").strip(),
            "profile": (inputs.get("profile") or "").strip() or None,
        }
        plan = None
    if not execution["suite"]:
        raise ResolutionError("suite is required: a suite id of config/test-plan.json or one suite as JSON")
    provider, _, model = execution["model"].partition("/")
    if not provider or not model:
        raise ResolutionError("model must be <provider>/<model>")
    if plan is None:
        # The plan shape the Console import and the ledger reports read.
        suite = execution["suite"]
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


def download(url: str, sha256: str, destination: Path) -> Path:
    """Fetch `url` and check it against the digest the release or lock states."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(url, timeout=300) as response:
        payload = response.read()
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

    names = ("suite", "stack", "model", "profile", "execution_id", "plan", "cli_version")
    dispatch = read_dispatch({name: os.environ.get(f"DISPATCH_{name.upper()}", "") for name in names})
    write_json(args.contract_dir / "execution.json", dispatch["execution"])
    write_json(args.contract_dir / "plan.json", dispatch["plan"])
    (args.contract_dir / "stack.yaml").write_text(yaml.safe_dump(dispatch["stack"], sort_keys=False))


def command_runtime(args: argparse.Namespace) -> None:
    token = os.environ.get("GITHUB_TOKEN") or None
    execution = json.loads((args.contract_dir / "execution.json").read_text())
    stack = load_yaml((args.contract_dir / "stack.yaml").read_text())
    cli = resolve_cli(stack.get("iii") or "latest", token)
    execution.update(iii=cli["version"], cli=cli, template=resolve_template(stack.get("template"), token))
    write_json(args.contract_dir / "execution.json", execution)


def command_runner(args: argparse.Namespace) -> None:
    """Resolve the stack's runner the way Compose will, pin it, fetch it."""
    import yaml

    directory, work = args.contract_dir, args.work_dir
    execution = json.loads((directory / "execution.json").read_text())
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
    binary = fetch_runner(lock, work)
    catalog = subprocess.run([str(binary), "catalog"], capture_output=True, text=True, check=True).stdout
    identity = json.loads(catalog).get("runner") or {}
    write_json(directory / "runner.json", {"name": RUNNER, "version": version, **identity})
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
    write_json(
        directory / "contracts" / "resolution.json",
        {
            "matrix": {"include": include},
            "cli_version": cli["version"],
            "campaign_ids": [campaign["campaign_id"] for campaign in snapshot["campaigns"]],
            **({"template": template} if template else {}),
            **({"stack_overrides": execution["stack_overrides"]} if execution.get("stack_overrides") else {}),
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
    print(fetch_runner(load_yaml(args.lock.read_text()), args.work_dir))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    dispatch = commands.add_parser("dispatch", help="read the DISPATCH_* inputs")
    runtime = commands.add_parser("runtime")
    runner = commands.add_parser("runner")
    runner.add_argument("--work-dir", type=Path, required=True)
    contracts = commands.add_parser("contracts")
    contracts.add_argument("--execution-key", required=True, help="the execution id, or the run id without one")
    contracts.add_argument("--oidc-audience", required=True)
    lock = commands.add_parser("lock")
    lock.add_argument("--assembled", type=Path, required=True, help="directory with the assembled worker-compose.{yaml,lock}")
    for command in (dispatch, runtime, runner, contracts, lock):
        command.add_argument("--contract-dir", type=Path, required=True)
    runner_binary = commands.add_parser("runner-binary")
    runner_binary.add_argument("--lock", type=Path, required=True)
    runner_binary.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args()
    handlers = {
        "dispatch": command_dispatch, "runtime": command_runtime, "runner": command_runner,
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
