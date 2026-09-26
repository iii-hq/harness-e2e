#!/usr/bin/env python3
"""Validate and materialize the exact-stack campaign contract.

`prepare_execution.py` writes one contract per campaign: the suite's groups,
the model, the iii release and the stack Compose assembled once, with its
lock. This tool turns a contract into what one group runs — its Compose
project, its run request, its evidence — and nothing is verified twice:
unknown fields are ignored so either side can add one and ship alone.
"""

from __future__ import annotations

import argparse
import base64
import copy
import hashlib
import json
import os
import re
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


CONTRACT_SCHEMA = "rc-e2e/v2"
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
EXECUTION_KINDS = {
    "harness_turn",
    "scripted_dialogue",
    "composite_flow",
    "adaptive_flow",
}
#: The application under test. Its package graph is the stack being measured.
APPLICATION = "harness"
#: The runner that executes the scenarios inside that stack.
RUNNER = "harness-e2e"
#: What a declaration means when it does not name a version.
DEFAULT_SELECTOR = "latest"
# The `database` package's own default for its `primary` database.
DATABASE_PRIMARY_URL = "sqlite:./data/iii.db"
#: The stack an execution runs on when it names none.
DEFAULT_STACK = Path(__file__).resolve().parents[1] / "stacks" / "default.yaml"
#: Stack keys the executor reads; the rest of a stack is the Compose project.
EXECUTOR_KEYS = ("iii", "template")
#: The key each provider reads, and the other credentials workers read.
CREDENTIAL_CATALOG = Path(__file__).resolve().parents[1] / "config" / "provider-credentials.json"
#: A credential is an environment variable.
CREDENTIAL_NAME = re.compile(r"^[A-Z][A-Z0-9_]*$")
#: Taken from a phase's own environment when set, as groups always were.
ENVIRONMENT_CREDENTIALS = ("DEEPSEEK_API_KEY", "ZAI_API_KEY", "TYPESAFE_API_KEY")
#: Shorter values are not looked for in evidence: they would match anywhere
#: and break the JSON they sit in.
REDACTION_MIN_LENGTH = 8
#: What the launcher captures from the processes it starts, below a bundle's
#: root: nothing hashes it, so a credential found there is replaced. Every
#: other file is bound by a digest (the runner's references, the aggregator's
#: campaign bundle, Release Control's checks) and is never rewritten.
REWRITABLE_ROOT = "logs"
#: The providers that sign in with a subscription login rather than a key:
#: the variable a group receives its access token in (never a refresh or id
#: token), the one that tells the provider where its login is, and the file
#: it reads there.
SUBSCRIPTION_LOGINS = {
    "openai-codex": ("CODEX_ACCESS_TOKEN", "CODEX_HOME", "auth.json"),
    "claude-code": ("CLAUDE_CODE_ACCESS_TOKEN", "CLAUDE_CONFIG_DIR", ".credentials.json"),
}
#: The ChatGPT account id claim of a Codex access token.
CODEX_AUTH_CLAIM = "https://api.openai.com/auth"


def load_yaml(text: str) -> Any:
    """YAML the way Compose reads it (1.2 core schema).

    PyYAML resolves YAML 1.1: `on`, `no` and `yes` become booleans, an
    unquoted date a datetime, `010` the octal 8 and `1:30` the sexagesimal
    90, which then reach Compose as something the author did not write (or
    do not serialize at all). Here only `true`/`false` are booleans, a date
    stays text, and integers are 1.2's: decimal, `0o` octal, `0x` hex.
    Writing back with `yaml.safe_dump` quotes the strings 1.1 would misread,
    which 1.2 reads as strings too.
    """
    import yaml

    class Loader(yaml.SafeLoader):
        pass

    replaced = {"tag:yaml.org,2002:bool", "tag:yaml.org,2002:timestamp", "tag:yaml.org,2002:int"}
    Loader.yaml_implicit_resolvers = {
        first: [(tag, pattern) for tag, pattern in resolvers if tag not in replaced]
        for first, resolvers in yaml.SafeLoader.yaml_implicit_resolvers.items()
    }
    Loader.add_implicit_resolver(
        "tag:yaml.org,2002:bool", re.compile(r"^(?:true|True|TRUE|false|False|FALSE)$"), list("tTfF")
    )
    Loader.add_implicit_resolver(
        "tag:yaml.org,2002:int", re.compile(r"^(?:[-+]?[0-9]+|0o[0-7]+|0x[0-9a-fA-F]+)$"), list("-+0123456789")
    )

    def integer(loader: Any, node: Any) -> int:
        value = loader.construct_scalar(node)
        if value.startswith(("0o", "0x")):
            return int(value[2:], 8 if value[1] == "o" else 16)
        return int(value, 10)

    Loader.add_constructor("tag:yaml.org,2002:int", integer)
    return yaml.load(text, Loader=Loader)


def worker_name(source: str) -> str:
    """The name a package is known by, whatever registry host its reference
    names: Compose records `package://api.workers.iii.dev/<name>` as readily
    as `package://<name>`."""
    return str(source).removeprefix("package://").rsplit("/", 1)[-1]


def declared_base() -> dict[str, Any]:
    """The Compose project of the default stack."""
    stack = load_yaml(DEFAULT_STACK.read_text())
    return {key: value for key, value in stack.items() if key not in EXECUTOR_KEYS}


def credential_catalog() -> tuple[dict[str, str], set[str]]:
    """The key each provider reads, and every name the catalog knows: a
    subscription login's access token too, which a group job may carry."""
    catalog = json.loads(CREDENTIAL_CATALOG.read_text())
    subscriptions = set(catalog.get("subscriptions", {}).values())
    return catalog["providers"], set(catalog["providers"].values()) | set(catalog["others"]) | subscriptions


def usable_credential(name: Any, value: Any) -> bool:
    """A named, non-empty, one-line value: what an env file can carry."""
    return (
        isinstance(name, str) and CREDENTIAL_NAME.fullmatch(name) is not None
        and isinstance(value, str) and value != "" and "\n" not in value and "\r" not in value
    )


def read_env_file(path: Path) -> dict[str, str]:
    """`NAME=value` lines, as `docker run --env-file` reads them."""
    values = {}
    for line in path.read_text().splitlines():
        name, separator, value = line.lstrip().partition("=")
        if separator and usable_credential(name, value):
            values[name] = value
    return values


def received_credentials(environ: Any, env_file: Path | None = None) -> dict[str, str]:
    """The credentials a phase received, and nothing else of its environment:
    the variables HARNESS_E2E_CREDENTIALS names (the env file
    run_in_image.sh passed the container), the three groups always took from
    their environment, and the entries of `env_file` when it exists."""
    names = set(environ.get("HARNESS_E2E_CREDENTIALS", "").split()) | set(ENVIRONMENT_CREDENTIALS)
    values = {name: environ[name] for name in sorted(names) if usable_credential(name, environ.get(name))}
    if env_file is not None and env_file.is_file():
        values.update(read_env_file(env_file))
    return values


def catalog_credentials(environ: Any) -> dict[str, str]:
    """The catalog's names that `environ` sets, and nothing else of it: a
    workflow step names exactly those secrets, never `toJSON(secrets)`,
    which holds a run for approval and hands the step every other secret."""
    _, known = credential_catalog()
    return {name: environ[name] for name in sorted(known) if usable_credential(name, environ.get(name))}


def write_private(path: Path, values: dict[str, str]) -> None:
    """An env file only its owner reads (mode 600), whatever was there."""
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    os.fchmod(descriptor, 0o600)
    with os.fdopen(descriptor, "w") as file:
        file.write("".join(f"{name}={value}\n" for name, value in sorted(values.items())))


def credential_forms(value: str) -> list[str]:
    """A value as written and as JSON escapes it, longest first."""
    forms = {value, json.dumps(value)[1:-1], json.dumps(value, ensure_ascii=False)[1:-1]}
    return sorted(forms, key=len, reverse=True)


def redact_tree(root: Path, paths: list[str], credentials: dict[str, str]) -> dict[str, Any]:
    """Look for each credential's value in the files at `paths`. Found in the
    launcher's logs, it is replaced by `[redacted:NAME]`; found anywhere else,
    nothing is rewritten and the tree is refused. Says how often per name,
    never the value."""
    scanned = {name: value for name, value in credentials.items() if len(value) >= REDACTION_MIN_LENGTH}
    # Longest first: a value that holds another is replaced whole.
    ordered = sorted(scanned.items(), key=lambda item: (-len(item[1]), item[0]))
    hits = dict.fromkeys(sorted(scanned), 0)
    rewrites: dict[Path, bytes] = {}
    bound = []
    for relative in paths:
        path = root / relative
        payload = original = path.read_bytes()
        found = set()
        for name, value in ordered:
            for form in credential_forms(value):
                count = payload.count(form.encode())
                if count:
                    hits[name] += count
                    found.add(name)
                    payload = payload.replace(form.encode(), f"[redacted:{name}]".encode())
        if payload == original:
            continue
        if Path(relative).parts[0] == REWRITABLE_ROOT:
            rewrites[path] = payload
        else:
            bound.append(f"{relative} ({', '.join(sorted(found))})")
    if bound:
        raise ValueError(
            "provider credentials found in files bound by digests, which are never rewritten: "
            + "; ".join(bound)
        )
    for path, payload in rewrites.items():
        path.write_bytes(payload)
    return {
        "credentials": hits,
        "files": sorted(path.relative_to(root).as_posix() for path in rewrites),
        "too_short": sorted(set(credentials) - set(scanned)),
    }


def load_object(path: Path, label: str) -> dict[str, Any]:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def canonical_sha256(value: Any) -> str:
    return f"sha256:{hashlib.sha256(canonical(value).encode()).hexdigest()}"


def require_keys(value: Any, required: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")
    missing = sorted(required - set(value))
    if missing:
        raise ValueError(f"{label} is missing fields: {', '.join(missing)}")
    return value


def require_text(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label} must be a non-empty string")
    return value


def require_digest(value: Any, label: str) -> str:
    value = require_text(value, label)
    if not SHA256.fullmatch(value):
        raise ValueError(f"{label} must be sha256:<64 lowercase hex>")
    return value


def require_positive_integer(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 1:
        raise ValueError(f"{label} must be a positive integer")
    return value


def require_nonnegative_integer(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError(f"{label} must be a non-negative integer")
    return value


def validate_identity(suite: dict[str, Any], role: str) -> None:
    identity = require_keys(suite.get(role), {"provider", "model"}, f"suite.{role}")
    require_text(identity.get("provider"), f"suite.{role}.provider")
    require_text(identity.get("model"), f"suite.{role}.model")


def validate_suite(suite: Any) -> dict[str, Any]:
    """Shape of what Release Control asked for.

    Execution policy — run bounds, retry rules per kind, canonical fault
    profiles — is enforced once, by the campaign aggregator that consumes the
    manifest this contract materializes.
    """
    require_keys(
        suite,
        {"id", "label", "lane", "seed", "subject", "groups"},
        "suite",
    )
    suite_id = require_text(suite.get("id"), "suite.id")
    if not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", suite_id):
        raise ValueError("suite.id must be kebab-case")
    require_text(suite.get("label"), "suite.label")
    require_text(suite.get("lane"), "suite.lane")
    if suite.get("seed") is not None:
        require_positive_integer(suite.get("seed"), "suite.seed")
    validate_identity(suite, "subject")
    if suite.get("agent_profile") is not None:
        require_text(suite["agent_profile"], "suite.agent_profile")

    groups = suite.get("groups")
    if not isinstance(groups, list) or not groups:
        raise ValueError("suite.groups must be a non-empty array")
    seen: set[str] = set()
    for index, group in enumerate(groups):
        label = f"suite.groups[{index}]"
        group = require_keys(
            group,
            {"id", "execution_kind", "runs", "technical_retries"},
            label,
        )
        group_id = require_text(group.get("id"), f"{label}.id")
        if not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", group_id) or group_id in seen:
            raise ValueError("suite group ids must be unique kebab-case values")
        seen.add(group_id)
        kind = require_text(group.get("execution_kind"), f"{label}.execution_kind")
        if kind not in EXECUTION_KINDS:
            raise ValueError(f"{label}.execution_kind is unsupported")
        require_positive_integer(group.get("runs"), f"{label}.runs")
        require_nonnegative_integer(group.get("technical_retries"), f"{label}.technical_retries")
        scenarios = group.get("scenarios")
        if not isinstance(scenarios, list) or not scenarios or len(set(scenarios)) != len(scenarios):
            raise ValueError(f"{label}.scenarios must be a non-empty unique array")
        for scenario in scenarios:
            require_text(scenario, f"{label}.scenarios[]")
    return suite


def validate_contract(contract: dict[str, Any]) -> dict[str, Any]:
    require_keys(
        contract,
        {
            "schema",
            "campaign_id",
            "execution_id",
            "attempt",
            "idempotency_key",
            "runtime",
            "security",
            "suite",
        },
        "contract",
    )
    # How a run is named and attributed is the dispatcher's business. These
    # fields are carried and displayed, so the executor asks that they be
    # present and legible, not that they match a particular spelling.
    require_text(contract.get("schema"), "schema")
    require_text(contract.get("campaign_id"), "campaign_id")
    require_text(contract.get("execution_id"), "execution_id")
    require_positive_integer(contract.get("attempt"), "attempt")
    require_text(contract.get("idempotency_key"), "idempotency_key")

    cli = require_keys(
        require_keys(contract.get("runtime"), {"cli"}, "runtime").get("cli"),
        {"version", "target", "asset", "sha256"},
        "runtime.cli",
    )
    require_text(cli.get("version"), "runtime.cli.version")
    require_text(cli.get("target"), "runtime.cli.target")
    asset = require_text(cli.get("asset"), "runtime.cli.asset")
    if not asset.startswith("iii-") or asset.startswith("iii-" + "worker"):
        raise ValueError("runtime.cli.asset must name the iii CLI archive")
    # The download is checked against it: integrity, not preference.
    require_digest(cli.get("sha256"), "runtime.cli.sha256")
    template = contract["runtime"].get("template")
    if template is not None:
        require_keys(template, {"id", "revision"}, "runtime.template")
        require_text(template["id"], "runtime.template.id")
        require_text(template["revision"], "runtime.template.revision")

    security = require_keys(contract.get("security"), {"oidc_audience"}, "security")
    audience = require_text(security.get("oidc_audience"), "security.oidc_audience")
    if not re.fullmatch(r"[A-Za-z0-9._:/-]+", audience):
        raise ValueError("security.oidc_audience contains unsupported characters")

    validate_suite(contract.get("suite"))
    return contract


def campaign_manifest(contract: dict[str, Any]) -> dict[str, Any]:
    """The suite, in the shape the campaign aggregator consumes."""
    suite = contract["suite"]
    groups = []
    for group in suite["groups"]:
        materialized = {
            "id": group["id"],
            "execution_kind": group["execution_kind"],
            "runs": group["runs"],
            "technical_retries": group["technical_retries"],
        }
        materialized["scenarios"] = group["scenarios"]
        groups.append(materialized)
    return {
        "kind": "harness-e2e-campaign",
        "campaign_id": suite["id"],
        "lane": suite["lane"],
        "failure_policy": "advisory",
        "groups": groups,
    }


def observed_versions(workers_payload: dict[str, Any], namespace: str | None = None) -> dict[str, str]:
    """What the engine reports it installed, by short worker name."""
    rows = workers_payload.get("workers")
    versions: dict[str, str] = {}
    for row in rows if isinstance(rows, list) else []:
        if not isinstance(row, dict) or row.get("namespace") not in (None, namespace):
            continue
        name, version = row.get("name"), row.get("version")
        if isinstance(name, str) and isinstance(version, str):
            versions[name.rsplit("/", 1)[-1]] = version
    return dict(sorted(versions.items()))


def materialize_request(
    contract: dict[str, Any], catalog: dict[str, Any], group_id: str | None = None,
    installed: dict[str, str] | None = None,
) -> dict[str, Any]:
    runner = require_keys(catalog.get("runner"), {"name", "version", "revision"}, "catalog.runner")
    catalog_sha256 = require_digest(catalog.get("catalog_sha256"), "catalog.catalog_sha256")
    descriptors = catalog.get("scenarios")
    if not isinstance(descriptors, list):
        raise ValueError("scenario catalog scenarios must be a list")
    by_id = {
        item["scenario_id"]: item
        for item in descriptors
        if isinstance(item, dict) and isinstance(item.get("scenario_id"), str)
    }

    suite = contract["suite"]
    group = next((item for item in suite["groups"] if item["id"] == group_id), None)
    if group is None:
        raise ValueError("a valid campaign group id is required")
    selected_cases: list[dict[str, Any]] = []
    for scenario_id in group["scenarios"]:
        descriptor = by_id.get(scenario_id)
        if descriptor is None:
            raise ValueError(f"{runner['name']}@{runner['version']} has no scenario {scenario_id}")
        selected_cases.append(
            {
                "scenario_id": scenario_id,
                "behavior_sha256": require_digest(
                    descriptor.get("behavior_sha256"), f"{scenario_id}.behavior_sha256"
                ),
                "case_id": require_text(descriptor.get("case_id"), f"{scenario_id}.case_id"),
                "seed": require_nonnegative_integer(descriptor.get("seed"), f"{scenario_id}.seed"),
                "inputs_sha256": require_digest(descriptor.get("inputs_sha256"), f"{scenario_id}.inputs_sha256"),
                "contract_sha256": require_digest(descriptor.get("contract_sha256"), f"{scenario_id}.contract_sha256"),
            }
        )

    request = {
        "label": f"{suite['label']} · {group['id']}",
        "lane": suite["lane"],
        "model": suite["subject"]["model"],
        "provider": suite["subject"]["provider"],
        "scenarios": group["scenarios"],
        "runs": group["runs"],
        "seed": suite["seed"],
        "rotating_seeds": [],
        "technical_retries": group["technical_retries"],
        "progress_interval_seconds": suite.get("progress_interval_seconds", 15),
        "run_contract": {
            "mode": {"environment": "demonstration", "decision": "observe_only"},
            # The declaration carries a selector, so the versions here are the
            # ones the engine installed, read back before the run starts.
            "target": {
                "application": APPLICATION,
                "version": (installed or {}).get(APPLICATION, DEFAULT_SELECTOR),
                "stack": {
                    "mode": "registry",
                    "stack_versions": installed or {},
                    "stack_lock_digest": canonical_sha256(installed or {}),
                },
            },
            # The plan is this contract: what Release Control froze and sent.
            "plan": {
                "id": contract["execution_id"],
                "revision": str(contract["attempt"]),
                "sha256": canonical_sha256(contract),
                "catalog_sha256": catalog_sha256,
            },
            "runner": runner,
            "attempt": contract["attempt"],
            "selected_cases": selected_cases,
            "correlation": {
                "system": "release-control",
                "deployment_id": contract["campaign_id"],
                "operation_id": contract["campaign_id"],
            },
        },
    }
    if suite.get("agent_profile") is not None:
        request["agent"] = suite["agent_profile"]
    # The runner keys admission on the fully materialized request, including the
    # cases and their fingerprints — those only exist after scenarios-list, so
    # the dispatch key cannot be reused. Deterministic for transport retries.
    request["idempotency_key"] = observation_idempotency_key(request)
    return request


def observation_idempotency_key(request: dict[str, Any]) -> str:
    intent = {
        key: request[key]
        for key in (
            "run_contract",
            "lane",
            "model",
            "provider",
            "scenarios",
            "runs",
            "seed",
            "rotating_seeds",
            "technical_retries",
        )
    }
    if request.get("agent") is not None:
        intent["agent"] = request["agent"]
    return f"rc:e2e:{canonical_sha256(intent).removeprefix('sha256:')}"


def assignments(values: list[str], label: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for value in values:
        key, separator, resolved = value.partition("=")
        if not separator or not key or key in result:
            raise ValueError(f"{label} values must be unique KEY=VALUE assignments")
        result[key] = resolved
    return result


def declared_workers(compose_path: Path) -> dict[str, str]:
    """The workers a compose project declares, with the selector each carries."""
    project = load_yaml(compose_path.read_text())
    if not isinstance(project, dict):
        raise ValueError("compose project must be an object")
    declared: dict[str, str] = {}
    for container in (project.get("containers") or {}).values():
        source = str(container.get("worker", ""))
        if source.startswith("package://"):
            # Compose rewrites a declaration with the registry host it resolved
            # against, so workers are compared by the name they are known by.
            declared[worker_name(source)] = str(container.get("version", DEFAULT_SELECTOR))
    return dict(sorted(declared.items()))


def project_roots(compose_path: Path) -> list[str]:
    return [f"{worker}@{selector}" for worker, selector in declared_workers(compose_path).items()]


def group_template(contract: dict[str, Any], group_id: str) -> str:
    group = next(group for group in contract["suite"]["groups"] if group["id"] == group_id)
    if "linkly_tutorial" not in group.get("scenarios", []):
        return contract["runtime"].get("template", {}).get("id", "")
    if (group["scenarios"] != ["linkly_tutorial"] or group["runs"] != 1
            or group["technical_retries"] != 0):
        raise ValueError("linkly_tutorial needs a fresh scaffold: one scenario, one run and no retries")
    return "linkly-agentic"


def project_engine_config(project: dict[str, Any], port: int) -> dict[str, Any]:
    workers = [{"name": "iii" + "-worker-manager", "config": {"host": "127.0.0.1", "port": port}}]
    workers.extend({"name": name, "config": config}
                   for name, config in project.get("engine", {}).get("workers", {}).items())
    return {"workers": workers}


def with_fixture(template: dict[str, Any], fixture: dict[str, Any]) -> dict[str, Any]:
    """Keep a scenario's required container names/configuration over the chosen base."""
    result = copy.deepcopy(template)
    containers = result.setdefault("containers", {})
    aliases = {"package://shell": "package://ide", "package://console": "package://ade"}
    for name, container in fixture.get("containers", {}).items():
        worker = aliases.get(container.get("worker"), container.get("worker"))
        for previous in list(containers):
            source = containers[previous].get("worker")
            if aliases.get(source, source) == worker:
                del containers[previous]
        containers[name] = copy.deepcopy(container)
    engine = result.setdefault("engine", {})
    engine_workers = engine.get("workers", {}) | fixture.get("engine", {}).get("workers", {})
    engine.update(fixture.get("engine", {}))
    if engine_workers:
        engine["workers"] = engine_workers
    return result


def merged(base: dict[str, Any], over: dict[str, Any]) -> dict[str, Any]:
    """`over` on top of `base`, mappings key by key, anything else replaced."""
    result = dict(base)
    for key, value in over.items():
        both = isinstance(value, dict) and isinstance(result.get(key), dict)
        result[key] = merged(result[key], value) if both else value
    return result


def built_versions(versions: dict[str, str], contract: dict[str, Any]) -> dict[str, str]:
    """`versions` with each worker built from a commit named `@<sha7>`: what
    it reports is its Cargo version, which a release series must not take
    for a release."""
    pins = ((contract.get("runtime") or {}).get("commits") or {}).values()
    return dict(sorted({**versions, **{pin["worker"]: f"@{pin['commit'][:7]}" for pin in pins}}.items()))


def scoped_config_name(namespace: str, name: str) -> str:
    """A configuration id both Compose and the engine accept.

    `<namespace>-<name>` when it fits their 64-character limit; otherwise the
    readable prefix with a stable digest, because Compose refuses to generate
    a truncated name and the engine refuses a longer id.
    """
    candidate = f"{namespace}-{name}"
    if len(candidate) <= 64:
        return candidate
    digest = hashlib.sha256(candidate.encode()).hexdigest()[:8]
    return f"{candidate[:55].rstrip('-')}-{digest}"

def project_scaffold(
    contract: dict[str, Any],
    namespace: str,
    data_dir: Path,
    env_file: str | None,
    environment: dict[str, str],
    template: dict[str, Any] | None = None,
    template_packages: dict[str, str] | None = None,
    profile_root: Path | None = None,
    group_id: str | None = None,
    assembling: bool = False,
) -> dict[str, Any]:
    """The Compose project one group starts, or with no group the stack the
    execution assembles once for all of them.

    Assembling, the model's provider and the Directory are not declared: a
    worker a declared one depends on (Harness brings both) arrives from its
    graph with the graph's pin, and a second declaration of it conflicts with
    that pin. The launcher asks for whichever no graph brought.

    Without a template it is the stack the contract carries — assembled once,
    its lock beside it — with only what is per group stamped on: namespace,
    runner data and configuration, credentials. Its workers keep the versions
    the lock was taken for. With a template, the template is the project and
    every package it declares takes the version the execution's lock
    resolved, or `latest`."""
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}[a-z0-9]", namespace):
        raise ValueError("project namespace must be lowercase kebab-case")
    if not data_dir.is_absolute():
        raise ValueError("data directory must be absolute")

    scenarios = {
        scenario for group in contract["suite"]["groups"] for scenario in group.get("scenarios", [])
    }
    harness_override = {}
    if "fanout_ladder" in scenarios:
        harness_override["max_children"] = 16
    if "depth_ladder" in scenarios:
        harness_override["max_depth"] = 6

    declared_environment: dict[str, dict[str, str]] = {}
    for key, value in environment.items():
        worker, separator, name = key.partition(".")
        if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", name):
            raise ValueError(f"invalid container environment assignment: {key}")
        declared_environment.setdefault(worker, {})[name] = value

    runtime = contract.get("runtime") or {}
    locked = {
        worker_name(entry["worker"]): entry["resolved"]["version"]
        for entry in ((runtime.get("lock") or {}).get("containers") or {}).values()
    }

    # The project is the template when there is one and the stack otherwise.
    # Add the runner and the campaign's provider to either project. Templates
    # only enable their own default providers; credentials alone cannot start
    # the provider selected by the campaign.
    if env_file and not Path(env_file).is_absolute():
        raise ValueError("env file must be absolute")
    manifest = copy.deepcopy(template if template is not None else (runtime.get("compose") or declared_base()))
    containers = manifest.setdefault("containers", {})
    # A worker the stack built from a commit is a path:// worker that is
    # still the package it was built from, known by its path.
    commits = runtime.get("commits") or {}
    assembled = (runtime.get("compose") or {}).get("containers") or {}
    built = {assembled[name]["worker"]: pin["worker"] for name, pin in commits.items() if name in assembled}

    def package_of(container: dict[str, Any]) -> str | None:
        source = str(container.get("worker", ""))
        return worker_name(source) if source.startswith("package://") else built.get(source)

    def declares(package: str) -> bool:
        return any(package_of(item) == package for item in containers.values())

    if not declares(RUNNER):
        containers.setdefault(RUNNER, {"worker": f"package://{RUNNER}", "version": DEFAULT_SELECTOR})
    group_scenarios = scenarios
    if group_id is not None:
        group = next((group for group in contract["suite"]["groups"] if group["id"] == group_id), None)
        if group is None:
            raise ValueError(f"unknown campaign group: {group_id}")
        group_scenarios = set(group["scenarios"])
    # Canvas only where a visual worker is built. The stack assembled for the
    # whole suite carries it when any group needs it; a group that does not
    # leaves it out again, with what only Canvas brought in.
    if {"form_flow_build", "state_machine_canvas_build"} & group_scenarios:
        if not declares("canvas"):
            if "canvas" in containers:
                raise ValueError("container canvas is already used by another worker")
            containers["canvas"] = {"worker": "package://canvas", "version": DEFAULT_SELECTOR}
    elif template is None and runtime.get("lock"):
        graphs = runtime["lock"].get("graphs") or {}
        others = {node for root, nodes in graphs.items() if root != "canvas" for node in nodes}
        for name in {"canvas", *graphs.get("canvas", [])} - others:
            containers.pop(name, None)
    provider = contract["suite"]["subject"]["provider"]
    if not re.fullmatch(r"[a-z][a-z0-9-]*", provider):
        raise ValueError("suite.subject.provider must be a provider package name")
    provider_package = f"provider-{provider}"
    if not assembling and not declares(provider_package):
        if provider_package in containers:
            raise ValueError(f"container {provider_package} is already used by another worker")
        containers[provider_package] = {"worker": f"package://{provider_package}", "version": DEFAULT_SELECTOR}
    package_names: dict[str, list[str]] = {}
    for name, container in containers.items():
        source = container.get("worker", "")
        # Only the executor's private env files may supply credentials.
        container.pop("env_file", None)
        if source.startswith("path://"):
            path = Path(source.removeprefix("path://"))
            if not path.is_absolute() and (not source.startswith("path://./") or ".." in path.parts):
                raise ValueError(f"declared worker {name} must stay inside its project")
            continue
        if not source.startswith("package://"):
            raise ValueError(f"declared worker {name} has an unsupported source: {source}")
        package = worker_name(source)
        renamed = (template_packages or {}).get(package, package)
        if renamed != package:
            container["worker"] = source[: -len(package)] + renamed
            package = renamed
        if template is not None:
            # A template's own pin is its author's, not ours: a fixture checked
            # out at an old commit would otherwise downgrade the very
            # application under test. It runs what the execution locked.
            container["version"] = locked.get(package, DEFAULT_SELECTOR)
        package_names.setdefault(package, []).append(name)
    if template is not None:
        # A worker the stack built from a commit runs that build, not the
        # template's package: its source, start and folder change, and the
        # build's default config and env go under the template's own, as
        # they do for any worker built from a commit. The dependencies are
        # declared beside it.
        for pinned, pin in commits.items():
            for name in package_names.pop(pin["worker"], []):
                container = containers[name]
                container.pop("version", None)
                container["worker"] = assembled[pinned]["worker"]
                container["scripts"] = {**(container.get("scripts") or {}), "run": assembled[pinned]["scripts"]["run"]}
                container["working_dir"] = assembled[pinned].get("working_dir", ".")
                if pin.get("config"):
                    container["config_override"] = merged(pin["config"], container.get("config_override") or {})
                if pin.get("env"):
                    container["environment"] = {**pin["env"], **(container.get("environment") or {})}
            for dependency in pin.get("dependencies") or []:
                if dependency in package_names:
                    continue
                if dependency in containers:
                    raise ValueError(f"container {dependency} is not the {dependency} worker {pin['worker']} depends on")
                containers[dependency] = copy.deepcopy(assembled[dependency])
                package_names[dependency] = [dependency]
        # Compose expands dependencies by container name, so a template that
        # renamed a role has to point at the name its own project uses.
        for container in containers.values():
            if "start_after" in container:
                container["start_after"] = [
                    dependency if dependency in containers else package_names.get(
                        (template_packages or {}).get(dependency, dependency), [dependency]
                    )[0]
                    for dependency in container["start_after"]
                ]
    for name, container in containers.items():
        worker = package_of(container)
        if worker in declared_environment:
            container.setdefault("environment", {}).update(sorted(declared_environment[worker].items()))
        if worker == RUNNER:
            container["config_name"] = scoped_config_name(namespace, "harness-e2e")
            container["config_override"] = {
                "data_dir": str(data_dir),
                "control_database": "primary",
                "control_namespace": namespace,
            }
        elif worker == APPLICATION and harness_override:
            container["config_name"] = scoped_config_name(namespace, "harness")
            container.setdefault("config_override", {}).update(harness_override)
        elif worker == "database":
            # The runner's `primary` database, the package's built-in default,
            # declared: iii 0.24.3+ injects the package's published default
            # (`{}`) as the live value, which hides the one the worker seeds.
            databases = container.setdefault("config_override", {}).setdefault("databases", {})
            databases.setdefault("primary", {"url": DATABASE_PRIMARY_URL})
        # Compose 0.24.2 derives `<namespace>-<container>` for the rest and
        # refuses to start when that exceeds 64 characters, which a long group
        # id reaches: only then name the container ourselves.
        if "config_name" not in container and len(f"{namespace}-{name}") > 64:
            container["config_name"] = scoped_config_name(namespace, name)
    if profile_root is not None and not assembling:
        if not profile_root.is_absolute():
            raise ValueError("profile root must be absolute")
        if "iii-directory" not in package_names and template is None and runtime.get("lock"):
            # A frozen start cannot add a worker its lock does not name.
            raise ValueError("the assembled stack brings no iii-directory; an agent profile needs one")
        if "iii-directory" not in package_names:
            containers["iii-directory"] = {
                "worker": "package://iii-directory",
                "version": locked.get("iii-directory", DEFAULT_SELECTOR),
            }
            package_names["iii-directory"] = ["iii-directory"]
        for name in package_names["iii-directory"]:
            containers[name]["config_name"] = scoped_config_name(namespace, "directory")
            containers[name].setdefault("config_override", {}).update({
                "auto_download": False,
                "skills_folder": str(profile_root / ".iii/registry-skills"),
                "local_skills_folder": str(profile_root / "skills"),
                "agents_folder": str(profile_root / "agents"),
                "global_agents_folder": str(profile_root / ".iii/empty/agents"),
                "agents_skills_folder": str(profile_root / ".agents/skills"),
                "global_agents_skills_folder": str(profile_root / ".iii/empty/skills"),
            })
    # One env file, every container. A worker reads the keys it knows and
    # ignores the rest; the template's own `./.env` reference is replaced by it.
    if env_file:
        for container in containers.values():
            container["env_file"] = [env_file]
    # iii telemetry stays off in every worker, whatever the Compose daemon
    # that starts it inherited (a developer's own daemon, say).
    for container in containers.values():
        container.setdefault("environment", {})["III_TELEMETRY_ENABLED"] = "false"
    manifest.update({
        "namespace": namespace,
        "startup_timeout": "5m",
        "stop_timeout": "30s",
        "containers": containers,
    })
    return manifest


def jwt_claims(token: str) -> dict[str, Any]:
    """The claims of a JWT, unverified; empty for anything else."""
    try:
        payload = token.split(".")[1]
        claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
    except (IndexError, ValueError):
        return {}
    return claims if isinstance(claims, dict) else {}


def subscription_login(
    contract: dict[str, Any], environ: dict[str, str], root: Path, now: float
) -> tuple[str, dict[str, Any]] | None:
    """The login a subscription provider starts with, written below `root`
    (mode 700, the file 600) from the access token the group received and
    nothing else: the provider's environment assignment pointing at it, and
    evidence of it without the token. None when the subject's provider takes
    an API key, or when its token was not given, which is said out loud: the
    provider starts signed out. A token that is already dead is an error."""
    provider = contract["suite"]["subject"]["provider"]
    if provider not in SUBSCRIPTION_LOGINS:
        return None
    variable, home, name = SUBSCRIPTION_LOGINS[provider]
    token = environ.get(variable, "")
    if not token:
        print(f"[WARN] {variable} is not set; provider-{provider} starts without a credential", file=sys.stderr)
        return None
    expires_at: float | None
    if provider == "openai-codex":
        claims = jwt_claims(token)
        expires_at = claims["exp"] if isinstance(claims.get("exp"), (int, float)) else None
        account = environ.get("CODEX_ACCOUNT_ID") or (claims.get(CODEX_AUTH_CLAIM) or {}).get("chatgpt_account_id")
        login: dict[str, Any] = {
            "auth_mode": "chatgpt",
            "tokens": {"access_token": token, **({"account_id": account} if account else {})},
        }
    else:
        milliseconds = environ.get("CLAUDE_CODE_EXPIRES_AT", "")
        if milliseconds and not milliseconds.isdigit():
            raise ValueError("CLAUDE_CODE_EXPIRES_AT must be epoch milliseconds")
        expires_at = int(milliseconds) / 1000 if milliseconds else None
        login = {"claudeAiOauth": {"accessToken": token, **({"expiresAt": int(milliseconds)} if milliseconds else {})}}
    if expires_at is not None and expires_at <= now + 60:
        raise ValueError(f"{variable} expired before the group started; provider-{provider} would start signed out")
    # A pasted token (GitHub) is not refreshed: said when it may not last the
    # group (its run timeout and fifteen minutes to start its stack).
    budget = int(environ.get("HARNESS_E2E_RUN_TIMEOUT_SECONDS") or 10800) + 900
    if expires_at is not None and expires_at < now + budget:
        at = datetime.fromtimestamp(expires_at, timezone.utc).isoformat()
        print(f"[WARN] {variable} expires at {at}, before the group's deadline; "
              f"provider-{provider} may be signed out before the group ends", file=sys.stderr)
    folder = root / provider
    folder.mkdir(mode=0o700, parents=True, exist_ok=True)
    folder.chmod(0o700)
    descriptor = os.open(folder / name, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    os.fchmod(descriptor, 0o600)
    with os.fdopen(descriptor, "w") as file:
        json.dump(login, file)
    evidence = {
        "provider": provider,
        "source": "env",
        "expires_at": None if expires_at is None else datetime.fromtimestamp(expires_at, timezone.utc).isoformat(),
    }
    return f"provider-{provider}.{home}={folder}", evidence


def compose_evidence(
    contract: dict[str, Any],
    compose_path: Path,
    namespace: str,
    lifecycle: dict[str, Any],
    workers_payload: dict[str, Any],
    processes: dict[str, Any],
) -> dict[str, Any]:
    worker_rows = workers_payload.get("workers")
    if not isinstance(worker_rows, list):
        raise ValueError("engine worker evidence must contain a workers array")

    # What the project was asked to run is the compose it was assembled from.
    requested = declared_workers(compose_path)
    observed = observed_versions(workers_payload, namespace)
    # The engine lists a worker by its container name, which a template picks
    # freely (Linkly runs `ade` as `console`). Compose already gated the start;
    # a container the engine does not report is drift to show, never a reason
    # to discard a finished run.
    containers = (load_yaml(compose_path.read_text()) or {}).get("containers") or {}
    missing = sorted(
        name for name, container in containers.items()
        if str(container.get("worker", "")).startswith("package://") and name not in observed
    )
    # The declaration carries a selector, so the versions that matter are the
    # ones the engine ended up installing. They are recorded, not compared.
    version_report_warnings: list[str] = []
    if missing:
        version_report_warnings.append(
            "containers the engine did not report: " + ", ".join(missing)
        )

    forbidden = "iii" + "-worker"
    for phase, rows in processes.items():
        if not isinstance(rows, list):
            raise ValueError(f"process inventory {phase} must be an array")
        for row in rows:
            if not isinstance(row, dict):
                raise ValueError(f"process inventory {phase} contains an invalid row")
            command = str(row.get("comm", ""))
            executable = str(row.get("args", "")).split(maxsplit=1)[0]
            if Path(command).name == forbidden or Path(executable).name == forbidden:
                raise ValueError(f"forbidden lifecycle executable observed during {phase}")

    return {
        "contract_sha256": canonical_sha256(contract),
        "compose_sha256": f"sha256:{hashlib.sha256(compose_path.read_bytes()).hexdigest()}",
        "namespace": namespace,
        "runtime": {
            "cli": contract["runtime"]["cli"],
            "requested_roots": dict(sorted(requested.items())),
            "observed_versions": dict(sorted(observed.items())),
            "version_report_warnings": version_report_warnings,
        },
        "lifecycle": lifecycle,
        "processes": processes,
        "forbidden_lifecycle_executable_absent": True,
    }


def validate_runtime_layout(artifact_root: Path, runtime_root: Path, allowed_root: Path) -> None:
    """Runtime state and provider secrets must never enter the uploaded tree."""
    artifact = artifact_root.resolve(strict=True)
    runtime = runtime_root.resolve(strict=True)
    allowed = allowed_root.resolve(strict=True)
    if artifact == allowed or not artifact.is_relative_to(allowed):
        raise ValueError("artifact root must remain below the canonical target directory")
    if artifact.is_relative_to(runtime) or runtime.is_relative_to(artifact):
        raise ValueError("runtime and artifact roots must not overlap")


def _package_files(root: Path) -> list[dict[str, Any]]:
    """Hash regular files without ever traversing or dereferencing symlinks."""
    if root.is_symlink():
        raise ValueError(f"artifact root must not be a symlink: {root}")
    resolved_root = root.resolve(strict=True)
    files: list[dict[str, Any]] = []
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if path.is_symlink():
            raise ValueError(f"artifact tree contains symlink: {relative}")
        if (
            path.name == ".env"
            or path.name.startswith(".env.")
            or path.name.endswith(".env")
            or path.name in {"secrets", ".aws", ".ssh", ".gnupg"}
        ):
            raise ValueError(f"artifact tree contains a reserved credential path: {relative}")
        if not path.is_file() or path.name == "bundle-manifest.json":
            continue
        resolved = path.resolve(strict=True)
        try:
            resolved.relative_to(resolved_root)
        except ValueError as error:
            raise ValueError(f"artifact escapes root: {relative}") from error
        payload = path.read_bytes()
        files.append(
            {
                "path": relative,
                "sha256": f"sha256:{hashlib.sha256(payload).hexdigest()}",
                "size_bytes": len(payload),
            }
        )
    return files


def package_bundle(
    root: Path,
    contract: dict[str, Any],
    workflow: dict[str, Any],
    credentials: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Check the tree and look for the credentials the phase received in it
    (see `redact_tree`), then hash it: the digests are of what is uploaded."""
    files = _package_files(root)
    redaction = redact_tree(root, [entry["path"] for entry in files], credentials or {})
    if redaction["files"]:
        files = _package_files(root)
    return {
        "schema": "e2e-observation-bundle",
        "campaign_id": contract["campaign_id"],
        "execution_id": contract["execution_id"],
        "attempt": contract["attempt"],
        "contract_sha256": canonical_sha256(contract),
        "workflow": workflow,
        "terminal_payload": "results.json" if (root / "results.json").is_file() else None,
        "failure_payload": "failure.json" if (root / "failure.json").is_file() else None,
        "files": files,
        "redaction": redaction,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("validate", "digest", "manifest", "groups"):
        command = commands.add_parser(name)
        command.add_argument("--contract", type=Path, required=True)
        if name == "manifest":
            command.add_argument("--output", type=Path, required=True)
    materialize = commands.add_parser("materialize")
    materialize.add_argument("--contract", type=Path, required=True)
    materialize.add_argument("--catalog", type=Path, required=True)
    materialize.add_argument("--workers", type=Path)
    materialize.add_argument("--namespace")
    materialize.add_argument("--output", type=Path, required=True)
    materialize.add_argument("--group-id")
    roots = commands.add_parser("roots")
    roots.add_argument("--compose", type=Path, required=True)
    template = commands.add_parser("group-template")
    template.add_argument("--contract", type=Path, required=True)
    template.add_argument("--group-id", required=True)
    project = commands.add_parser("project")
    project.add_argument("--contract", type=Path, required=True)
    project.add_argument("--group-id", help="omit to scaffold the stack the whole suite shares")
    project.add_argument("--assemble", action="store_true", help="the stack the execution assembles once")
    project.add_argument("--namespace", required=True)
    project.add_argument("--data-dir", type=Path, required=True)
    project.add_argument("--env-file", type=Path)
    project.add_argument("--environment", action="append", default=[])
    project.add_argument("--output", type=Path, required=True)
    project.add_argument("--template-compose", type=Path)
    project.add_argument("--fixture-compose", type=Path)
    project.add_argument("--profile-root", type=Path)
    project.add_argument("--template-package", action="append", default=[])
    project.add_argument("--engine-config", type=Path)
    project.add_argument("--engine-port", type=int, default=49134)
    evidence = commands.add_parser("compose-evidence")
    evidence.add_argument("--contract", type=Path, required=True)
    evidence.add_argument("--compose", type=Path, required=True)
    evidence.add_argument("--namespace", required=True)
    for name in ("add", "up", "status", "down", "workers", "process-before", "process-during", "process-after"):
        evidence.add_argument(f"--{name}", type=Path, required=True)
    evidence.add_argument("--output", type=Path, required=True)
    package = commands.add_parser("package")
    package.add_argument("--root", type=Path, required=True)
    package.add_argument("--contract", type=Path, required=True)
    package.add_argument("--workflow", required=True)
    package.add_argument("--output", type=Path, required=True)
    package.add_argument("--credentials", type=Path,
                         help="an env file of the credentials the phase received, redacted out of the tree")
    stack_env = commands.add_parser("credentials-env", help="the stack's .env: every credential the group received")
    stack_env.add_argument("--contract", type=Path, required=True)
    stack_env.add_argument("--output", type=Path, required=True)
    credentials_file = commands.add_parser(
        "credentials-file", help="a private env file of the catalog's credentials this environment sets")
    credentials_file.add_argument("--output", type=Path, required=True)
    login = commands.add_parser("subscription-login")
    login.add_argument("--contract", type=Path, required=True)
    login.add_argument("--root", type=Path, required=True)
    login.add_argument("--evidence", type=Path, required=True)
    layout = commands.add_parser("validate-layout")
    layout.add_argument("--artifact-root", type=Path, required=True)
    layout.add_argument("--runtime-root", type=Path, required=True)
    layout.add_argument("--allowed-root", type=Path, required=True)
    args = parser.parse_args()

    try:
        if args.command == "validate-layout":
            validate_runtime_layout(args.artifact_root, args.runtime_root, args.allowed_root)
            return 0
        if args.command == "roots":
            # The declaration answers this one; there is no contract to read.
            for root in project_roots(args.compose):
                print(root)
            return 0
        if args.command == "credentials-file":
            values = catalog_credentials(os.environ)
            write_private(args.output, values)
            print("provider credentials: " + (", ".join(values) or "none"))
            return 0
        contract = validate_contract(load_object(args.contract, "contract"))
        if args.command == "validate":
            print(canonical(contract))
        elif args.command == "digest":
            print(canonical_sha256(contract))
        elif args.command == "groups":
            for group in contract["suite"]["groups"]:
                print(group["id"])
        elif args.command == "manifest":
            args.output.write_text(json.dumps(campaign_manifest(contract), indent=2) + "\n")
        elif args.command == "materialize":
            installed = built_versions(observed_versions(
                load_object(args.workers, "engine workers"), args.namespace
            ), contract) if args.workers else {}
            request = materialize_request(
                contract, load_object(args.catalog, "scenario catalog"),
                group_id=args.group_id, installed=installed,
            )
            args.output.write_text(json.dumps(request, indent=2, sort_keys=True) + "\n")
        elif args.command == "project":
            try:
                import yaml
            except ImportError as error:  # pragma: no cover - CI installs PyYAML explicitly.
                raise ValueError("PyYAML is required to create the iii project scaffold") from error
            template = load_yaml(args.template_compose.read_text()) if args.template_compose else None
            if args.fixture_compose:
                if template is None:
                    raise ValueError("fixture-compose requires template-compose")
                # Selected local workers still belong to their own scaffold,
                # while the scenario keeps its canonical task directory.
                for container in template.get("containers", {}).values():
                    source = container.get("worker", "")
                    if source.startswith("path://./"):
                        container["worker"] = f"path://{(args.template_compose.parent / source.removeprefix('path://')).resolve()}"
                template = with_fixture(template, load_yaml(args.fixture_compose.read_text()))
            manifest = project_scaffold(
                contract,
                args.namespace,
                args.data_dir,
                str(args.env_file) if args.env_file else None,
                assignments(args.environment, "environment"),
                template,
                assignments(args.template_package, "template-package"),
                args.profile_root,
                args.group_id,
                args.assemble,
            )
            if "engine" in manifest:
                manifest["engine"]["url"] = f"ws://127.0.0.1:{args.engine_port}"
            args.output.write_text(yaml.safe_dump(manifest, sort_keys=False))
            lock = contract["runtime"].get("lock")
            if lock and template is None:
                # Beside the compose file, where `compose::up` reads it, and
                # only for what this group declares: frozen, Compose refuses a
                # lock entry the file no longer names.
                kept = manifest["containers"]
                lock = {
                    **lock,
                    "containers": {name: entry for name, entry in lock["containers"].items() if name in kept},
                    **({"graphs": {root: [node for node in nodes if node in kept]
                                   for root, nodes in lock["graphs"].items() if root in kept}}
                       if lock.get("graphs") else {}),
                }
                (args.output.parent / "worker-compose.lock").write_text(yaml.safe_dump(lock, sort_keys=False))
            if args.engine_config:
                args.engine_config.write_text(yaml.safe_dump(project_engine_config(manifest, args.engine_port), sort_keys=False))
        elif args.command == "subscription-login":
            # The token comes from the environment, never the command line.
            delivered = subscription_login(contract, dict(os.environ), args.root, time.time())
            if delivered:
                assignment, evidence = delivered
                args.evidence.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")
                print(assignment)
        elif args.command == "group-template":
            print(group_template(contract, args.group_id))
        elif args.command == "credentials-env":
            values = received_credentials(os.environ)
            write_private(args.output, values)
            # Said out loud; the group still runs.
            provider = contract["suite"]["subject"]["provider"]
            key = credential_catalog()[0].get(provider)
            if key and key not in values:
                print(f"[WARN] {key} is not set; provider-{provider} starts without a credential", file=sys.stderr)
        elif args.command == "compose-evidence":
            manifest = compose_evidence(
                contract,
                args.compose,
                args.namespace,
                {name: load_object(getattr(args, name), f"compose {name}") for name in ("add", "up", "status", "down")},
                load_object(args.workers, "engine workers"),
                {
                    "before": json.loads(args.process_before.read_text()),
                    "during": json.loads(args.process_during.read_text()),
                    "after": json.loads(args.process_after.read_text()),
                },
            )
            args.output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        else:
            workflow = json.loads(args.workflow)
            if not isinstance(workflow, dict):
                raise ValueError("workflow must be a JSON object")
            credentials = received_credentials(os.environ, args.credentials)
            manifest = package_bundle(args.root, contract, workflow, credentials)
            for name in manifest["redaction"]["too_short"]:
                print(f"::warning::{name} is shorter than {REDACTION_MIN_LENGTH} characters: "
                      "the evidence is not checked for it")
            args.output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        return 0
    except (ValueError, json.JSONDecodeError, OSError) as error:
        print(f"error: {error}")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
