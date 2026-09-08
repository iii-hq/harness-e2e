#!/usr/bin/env python3
"""Trusted, evidence-only validator for one completed Registry task.

All product commands are sent through the attempt's private DIND runner.  This
script never invokes a product command directly on the controller host and it
never infers a pass from a subject report alone.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import shutil
import subprocess

REGISTRY_SHA = "662eb87c1bdbb395f36264d5d26bf823e2ace783"
IMPLEMENTATION_METRICS = (
    "implementation.same_version", "implementation.function_removal", "implementation.required_impact",
    "implementation.object_order", "implementation.required_order", "implementation.enum_order",
    "implementation.config_array_order", "implementation.missing_metadata", "implementation.worker_lookup",
    "implementation.reverse_kinds", "implementation.reverse_values", "implementation.reverse_impact",
    "implementation.shared_url", "implementation.stale_results", "implementation.patch_application",
    "implementation.invalid_version", "implementation.missing_worker", "implementation.missing_version",
    "implementation.history", "implementation.expanded_detail", "implementation.keyboard_selectors",
    "implementation.versions_regression", "implementation.readme_regression",
    "implementation.api_reference_regression", "implementation.download_regression",
)
PUBLIC_VERIFICATION_IDS = frozenset(metric for metric in IMPLEMENTATION_METRICS
                                    if metric != "implementation.patch_application")
ENVIRONMENT_METRICS = (
    "environment.build", "environment.migration", "environment.seed", "environment.api_readiness",
    "environment.frontend_reachability", "environment.artifact_integrity", "environment.clean_reproduction",
    "environment.restart_persistence", "environment.parallel_isolation", "environment.cleanup_completeness",
    "environment.cleanup_scope", "environment.registry_base", "environment.runtime_identity",
    "environment.database_readiness",
)
VERIFICATION_METRICS = (
    "verification.recall", "verification.precision", "verification.execution_coverage",
    "verification.evidence_coverage", "verification.source_preservation",
)


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def relative_evidence(run_root, path):
    path = path.resolve()
    root = run_root.resolve()
    if not path.is_file() or not path.is_relative_to(root):
        return None
    return str(path.relative_to(root))


def unavailable(metric_id, reason):
    return {"id": metric_id, "status": "unavailable", "reason": reason}


def binary(metric_id, value, evidence):
    if not evidence:
        return unavailable(metric_id, "no_attributable_evidence")
    return {"id": metric_id, "status": "measured", "value": int(bool(value)), "evidence": evidence}


def infrastructure_failure(result):
    text = (result.get("stderr", "") + result.get("stdout", "")).lower()
    return result.get("timeout") or any(token in text for token in (
        "cannot connect to the docker daemon", "no such container", "is the docker daemon running",
        "permission denied while trying to connect"))


def command_binary(metric_id, result, evidence):
    if infrastructure_failure(result):
        return unavailable(metric_id, "validator_infrastructure_unavailable")
    return binary(metric_id, result.get("exit_code") == 0, evidence)


def ratio(metric_id, numerator, denominator, evidence, reason=None, empty_value=None):
    if not evidence:
        return unavailable(metric_id, "no_attributable_evidence")
    if denominator == 0:
        if empty_value in (0, 1):
            return {"id": metric_id, "status": "measured", "numerator": 0,
                    "denominator": 0, "value": empty_value,
                    "reason": reason or "zero_denominator", "evidence": evidence}
        return {"id": metric_id, "status": "not_applicable", "numerator": 0,
                "denominator": 0, "reason": reason or "zero_denominator", "evidence": evidence}
    return {"id": metric_id, "status": "measured", "numerator": numerator,
            "denominator": denominator, "evidence": evidence}


def controller_command(state, command, timeout=120):
    """Run command only inside the private outer DIND container."""
    try:
        result = subprocess.run(
            ["docker", "exec", "-i", "-w", "/workspace", state["container"],
             "timeout", "--signal=KILL", str(timeout), "sh", "-lc", command],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout + 15, check=False,
        )
        return {"exit_code": result.returncode,
                "stdout": result.stdout.decode(errors="replace"),
                "stderr": result.stderr.decode(errors="replace")}
    except subprocess.TimeoutExpired as error:
        return {"exit_code": None, "stdout": "", "stderr": str(error), "timeout": True}


def save_command(root, name, result):
    path = root / "validation" / "commands" / f"{name}.json"
    write_json(path, result)
    return relative_evidence(root.parent, path)


def actual_evidence(run_root, task_root, references):
    evidence = []
    for reference in references or []:
        candidate = Path(reference)
        if candidate.is_absolute():
            # Feature probe paths are inside /workspace, whose host counterpart
            # is this task's workspace; any other absolute location is rejected.
            try:
                candidate = task_root / "workspace" / candidate.relative_to("/workspace")
            except ValueError:
                continue
        else:
            candidate = task_root / "workspace" / candidate
        item = relative_evidence(run_root, candidate)
        if item:
            evidence.append(item)
    return sorted(set(evidence))


def feature_probe(task_root, assets, state):
    """Run the independent probe in the real web container and collect its JSON."""
    script = assets / "validate-feature.cjs"
    if not script.is_file():
        return None, "feature_probe_not_supplied"
    command = ["docker", "exec", "-i", state["container"], "docker", "compose", "-f",
               "/fixture/compose.yaml", "exec", "-T", "web", "node", "-"]
    try:
        result = subprocess.run(command, input=script.read_bytes(), stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=180, check=False)
    except subprocess.TimeoutExpired:
        return None, "feature_probe_timeout"
    log = task_root / "validation" / "feature-probe.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_bytes(result.stdout + result.stderr)
    if result.returncode:
        return None, "feature_probe_failed"
    copied = controller_command(state, "mkdir -p /workspace/output/validator && docker compose -f /fixture/compose.yaml cp web:/tmp/registry-validation/. /workspace/output/validator/", 120)
    save_command(task_root, "feature-probe-copy", copied)
    if copied.get("exit_code") != 0:
        return None, "feature_probe_copy_failed"
    source = task_root / "workspace" / "output" / "validator" / "feature.json"
    if not source.is_file():
        return None, "feature_probe_output_missing"
    destination = task_root / "validation" / "feature.json"
    shutil.copyfile(source, destination)
    try:
        return json.loads(destination.read_text()), None
    except json.JSONDecodeError:
        return None, "feature_probe_output_invalid"


def checks_by_id(feature):
    checks = feature.get("observations", []) if isinstance(feature, dict) else []
    return {item.get("id"): item for item in checks if isinstance(item, dict) and isinstance(item.get("id"), str)}


def recorded_commands(task_root):
    commands = set()
    for path in (task_root / "commands").glob("*.json"):
        try:
            item = json.loads(path.read_text())
        except (json.JSONDecodeError, OSError):
            continue
        if isinstance(item.get("command"), str):
            commands.add(item["command"])
    return commands


def implementation_observations(task_root, assets, state, test):
    run_root = task_root.parent
    feature, error = feature_probe(task_root, assets, state)
    metric_ids = IMPLEMENTATION_METRICS
    checks = checks_by_id(feature or {})
    feature_evidence = relative_evidence(run_root, task_root / "validation" / "feature.json")
    observations = []
    for metric in metric_ids:
        if metric == "implementation.patch_application":
            patch = task_root / "source.patch"
            if not patch.is_file():
                observations.append(unavailable(metric, "patch_missing"))
                continue
            # A fresh detached base is made inside the DIND workspace.  The
            # temporary index/object paths avoid changing the subject checkout.
            shutil.copyfile(patch, task_root / "workspace" / "output" / "validator.patch")
            checkout = controller_command(state,
                "set -eu; rm -rf -- /tmp/registry-patch-validation; "
                "git config --global --add safe.directory /workspace/registry/.git; "
                "git clone --quiet --no-checkout "
                "/workspace/registry /tmp/registry-patch-validation; "
                f"git -C /tmp/registry-patch-validation checkout --quiet --detach {REGISTRY_SHA}", 120)
            evidence = [relative_evidence(run_root, patch),
                        save_command(task_root, "patch-checkout", checkout)]
            if checkout.get("exit_code") != 0:
                observations.append(unavailable(metric, "patch_replay_checkout_failed"))
                continue
            result = controller_command(state,
                "git -C /tmp/registry-patch-validation apply --check /workspace/output/validator.patch", 120)
            evidence.append(save_command(task_root, "patch-application", result))
            observations.append(command_binary(metric, result, [item for item in evidence if item]))
            continue
        if error:
            observations.append(unavailable(metric, error))
            continue
        check = checks.get(metric)
        if not check:
            observations.append(unavailable(metric, "independent_case_missing"))
            continue
        if check.get("status") == "measured" and check.get("value") in (0, 1):
            observations.append(binary(metric, check["value"] == 1, [feature_evidence] if feature_evidence else []))
        elif check.get("status") in ("unavailable", "not_applicable"):
            observations.append(unavailable(metric, check.get("reason", "independent_case_unavailable")))
        else:
            observations.append(unavailable(metric, "independent_case_invalid"))
    return observations


def verification_observations(task_root, assets, state):
    run_root = task_root.parent
    feature, _error = feature_probe(task_root, assets, state)
    independent = checks_by_id(feature or {})
    truth_complete = all(independent.get(case_id, {}).get("status") == "measured"
                         and independent[case_id].get("value") in (0, 1)
                         for case_id in PUBLIC_VERIFICATION_IDS)
    known_failed = {case_id for case_id in PUBLIC_VERIFICATION_IDS
                    if independent.get(case_id, {}).get("status") == "measured"
                    and independent[case_id].get("value") == 0}
    feature_path = task_root / "validation" / "feature.json"
    feature_evidence = relative_evidence(run_root, feature_path)
    checks_path = task_root / "workspace" / "output" / "checks.json"
    if not checks_path.is_file():
        return empty_verification(task_root, state, run_root, truth_complete, known_failed,
                                  [item for item in [feature_evidence] if item])
    try:
        document = json.loads(checks_path.read_text())
        reported = document.get("checks", [])
        if not isinstance(reported, list):
            raise ValueError("checks must be an array")
    except (json.JSONDecodeError, ValueError, AttributeError):
        invalid_evidence = [relative_evidence(run_root, checks_path), feature_evidence]
        return empty_verification(task_root, state, run_root, truth_complete, known_failed,
                                  [item for item in invalid_evidence if item])
    report = {case["id"]: case for case in reported if isinstance(case, dict)
              and isinstance(case.get("id"), str)}
    report_fail = {case_id for case_id, case in report.items() if case.get("status") == "fail"}
    commands = recorded_commands(task_root)
    evidenced = {case_id for case_id, case in report.items()
                 if case.get("status") in ("pass", "fail", "blocked")
                 and actual_evidence(run_root, task_root, case.get("evidence"))}
    executed = {case_id for case_id, case in report.items()
                if case_id in PUBLIC_VERIFICATION_IDS and case.get("status") in ("pass", "fail")
                and isinstance(case.get("command"), str) and case["command"] in commands
                and case_id in evidenced}
    report_evidence = relative_evidence(run_root, checks_path)
    evidence = [item for item in (report_evidence, feature_evidence) if item]
    observations = []
    if not truth_complete:
        observations.append(unavailable("verification.recall", "independent_truth_incomplete"))
    elif not known_failed:
        observations.append(ratio("verification.recall", 0, 0, evidence,
                                  "no_independently_failing_checks", empty_value=1))
    else:
        observations.append(ratio("verification.recall", len(known_failed & report_fail), len(known_failed), evidence))
    if any(independent.get(case_id, {}).get("status") != "measured"
           for case_id in report_fail & PUBLIC_VERIFICATION_IDS):
        observations.append(unavailable("verification.precision", "reported_failure_truth_unavailable"))
    elif not report_fail:
        observations.append(ratio("verification.precision", 0, 0, evidence,
                                  "no_reported_failures", empty_value=1))
    else:
        observations.append(ratio("verification.precision", len(known_failed & report_fail), len(report_fail), evidence))
    observations.append(ratio("verification.execution_coverage", len(executed), len(PUBLIC_VERIFICATION_IDS), evidence))
    observations.append(ratio("verification.evidence_coverage", len(evidenced), len(report), evidence,
                              "no_reported_outcomes", empty_value=0))
    observations.extend(source_preservation(task_root, state, run_root))
    return observations


def empty_verification(task_root, state, run_root, truth_complete, known_failed, evidence):
    observations = []
    if truth_complete:
        observations.append(ratio("verification.recall", 0, len(known_failed), evidence)
                            if known_failed else ratio("verification.recall", 0, 0, evidence,
                                                      "no_independently_failing_checks", empty_value=1))
    else:
        observations.append(unavailable("verification.recall", "independent_truth_incomplete"))
    observations.append(ratio("verification.precision", 0, 0, evidence,
                              "no_reported_failures", empty_value=1))
    observations.append(ratio("verification.execution_coverage", 0,
                              len(PUBLIC_VERIFICATION_IDS), evidence))
    observations.append(ratio("verification.evidence_coverage", 0, 0, evidence,
                              "no_reported_outcomes", empty_value=0))
    observations.extend(source_preservation(task_root, state, run_root))
    return observations


def source_preservation(task_root, state, run_root):
    source_patch = task_root / "source.patch"
    state_path = task_root / "state.json"
    evidence = [relative_evidence(run_root, source_patch), relative_evidence(run_root, state_path)]
    expected_patch = state.get("initial_patch_sha256")
    if isinstance(expected_patch, str) and source_patch.is_file():
        return [binary("verification.source_preservation", hashlib.sha256(source_patch.read_bytes()).hexdigest() == expected_patch, [item for item in evidence if item])]
    else:
        return [unavailable("verification.source_preservation", "delivery_or_final_patch_missing")]


def environment_observations(task_root, assets, state):
    """Validate the submitted environment through its explicit runtime contract."""
    metric_ids = ENVIRONMENT_METRICS
    run_root = task_root.parent
    contract = task_root / "workspace" / "output" / "environment.json"
    if not contract.is_file():
        evidence = [item for item in [relative_evidence(run_root, task_root / "state.json")] if item]
        return [binary(metric, False, evidence) for metric in metric_ids]
    try:
        environment = json.loads(contract.read_text())
        compose = environment["compose_file"]
        startup = environment["startup_command"]
        teardown = environment["teardown_command"]
        migration = environment["migration_command"]
        db_service = environment["db_service"]
        db_user = environment["db_user"]
        db_name = environment["db_name"]
        if not all(isinstance(value, str) and value for value in (compose, startup, teardown, migration, db_service, db_user, db_name)):
            raise ValueError
    except (json.JSONDecodeError, KeyError, ValueError):
        evidence = [item for item in [relative_evidence(run_root, contract)] if item]
        return [binary(metric, False, evidence) for metric in metric_ids]
    compose_path = Path(compose)
    if compose_path.is_absolute() or ".." in compose_path.parts or not compose.startswith("registry/"):
        evidence = [item for item in [relative_evidence(run_root, contract)] if item]
        return [binary(metric, False, evidence) for metric in metric_ids]
    compose_arg = shlex.quote(f"/workspace/{compose}")
    db_service = shlex.quote(db_service)
    project = f"validator-{state['container'][-12:]}"

    def scoped(command, selected_project=project, web_port=state["web_port"], api_port=state["api_port"]):
        variables = f"COMPOSE_PROJECT_NAME={shlex.quote(selected_project)} WEB_PORT={web_port} API_PORT={api_port}"
        return f"{variables} sh -lc {shlex.quote(command)}"

    def checked(name, command, timeout=180):
        result = controller_command(state, command, timeout)
        return result, [save_command(task_root, name, result)]

    build, build_evidence = checked("environment-build", scoped(f"docker compose -f {compose_arg} build"), 600)
    start, start_evidence = checked("environment-start", scoped(startup), 600)
    migrate, migrate_evidence = checked("environment-migrate", scoped(migration), 300)
    db, db_evidence = checked("environment-db-ready", scoped(f"docker compose -f {compose_arg} exec -T {db_service} pg_isready"), 90)
    health_assertion = "import json,sys; d=json.load(sys.stdin); assert any(x.get('name') == 'database' and x.get('status') == 'ok' for x in d.get('results', []))"
    api, api_evidence = checked("environment-api-health", f"curl -fsS http://127.0.0.1:{state['api_port']}/health | python3 -c {shlex.quote(health_assertion)}", 60)
    browser_script = "const{chromium}=require('@playwright/test');(async()=>{const b=await chromium.launch({headless:true});try{const p=await b.newPage();await p.goto(process.env.E2E_APP_URL+'/workers/orders-worker',{waitUntil:'networkidle'});if(!(await p.locator('body').innerText()).includes('orders-worker'))process.exitCode=1}finally{await b.close()}})().catch(()=>process.exitCode=1)"
    web, web_evidence = checked("environment-web-browser", scoped(f"docker run --rm --network host -e E2E_APP_URL=http://127.0.0.1:{state['web_port']} --entrypoint node $(docker compose -f {compose_arg} images -q web | head -n 1) -e {shlex.quote(browser_script)}"), 120)
    psql = f"psql -U {shlex.quote(db_user)} -d {shlex.quote(db_name)}"
    seed_query = (f"docker compose -f {compose_arg} exec -T {db_service} {psql} -Atc "
                  "\"select version from worker_version where worker_id='10000000-0000-4000-8000-000000000001' order by version\"")
    seed, seed_evidence = checked("environment-seed", scoped(seed_query), 90)
    expected_versions = {"0.9.0", "1.0.0", "1.1.0", "2.0.0"}
    canonical_seed_query = (f"docker compose -f {compose_arg} exec -T {db_service} {psql} -Atc "
        "\"select (select count(*) from worker_version where worker_id='10000000-0000-4000-8000-000000000001')=4 "
        "and (select config->>'timeoutMs' from worker_version where worker_id='10000000-0000-4000-8000-000000000001' and version='1.0.0')='3000' "
        "and (select config->>'route/name' from worker_version where worker_id='10000000-0000-4000-8000-000000000001' and version='1.0.0')='orders' "
        "and (select config->>'timeoutMs' from worker_version where worker_id='10000000-0000-4000-8000-000000000001' and version='2.0.0')='5000' "
        "and (select jsonb_array_length(functions) from worker_version where worker_id='10000000-0000-4000-8000-000000000001' and version='1.0.0')=2 "
        "and (select jsonb_array_length(functions) from worker_version where worker_id='10000000-0000-4000-8000-000000000001' and version='1.1.0')=3\"")
    canonical, canonical_evidence = checked("environment-canonical-seed", scoped(canonical_seed_query), 90)
    seeded = (expected_versions == set(seed.get("stdout", "").split())
              and canonical.get("stdout", "").strip() == "t")
    artifacts, artifact_evidence = checked("environment-artifacts", "set -eu; cd /workspace/inputs/artifacts; for f in *; do test \"$(sha256sum \"$f\" | cut -d' ' -f1)\" = \"$(curl -fsS http://127.0.0.1:%s/fixture-artifacts/$f | sha256sum | cut -d' ' -f1)\"; done" % state["web_port"], 120)
    # Restart and replay use the explicit startup contract under a different
    # project and ports, so success cannot be borrowed from the first stack.
    sentinel = f"docker compose -f {compose_arg} exec -T {db_service} {psql} -v ON_ERROR_STOP=1 -Atc \"create table if not exists validator_restart_sentinel(id integer); truncate validator_restart_sentinel; insert into validator_restart_sentinel values (1)\""
    sentinel_result, sentinel_evidence = checked("environment-restart-sentinel", scoped(sentinel), 90)
    restart, restart_evidence = checked("environment-restart", scoped(f"docker compose -f {compose_arg} restart"), 300)
    persistence_query = f"docker compose -f {compose_arg} exec -T {db_service} {psql} -Atc \"select count(*) from validator_restart_sentinel\""
    persisted, persisted_evidence = checked("environment-persistence", scoped(persistence_query), 90)
    alternate_project = f"{project}-replay"
    alternate_web_port, alternate_api_port = 41000, 41001
    replay, replay_evidence = checked("environment-clean-replay", scoped(startup, alternate_project, alternate_web_port, alternate_api_port), 600)
    replay_health, replay_health_evidence = checked("environment-clean-replay-health", f"curl -fsS http://127.0.0.1:{alternate_api_port}/health | python3 -c {shlex.quote(health_assertion)}", 60)
    sentinel_create = (f"docker compose -f {compose_arg} exec -T {db_service} {psql} -v ON_ERROR_STOP=1 -Atc "
                       "\"create table validator_isolation_sentinel(id integer); insert into validator_isolation_sentinel values (1);\"")
    sentinel_absent = (f"docker compose -f {compose_arg} exec -T {db_service} {psql} -Atc "
                       "\"select to_regclass('validator_isolation_sentinel') is null\"")
    isolation_command = scoped(sentinel_create) + " >/dev/null && " + scoped(sentinel_absent, alternate_project, alternate_web_port, alternate_api_port)
    isolation, isolation_evidence = checked("environment-isolation", isolation_command, 180)
    resources, resources_evidence = checked("environment-resources", f"docker ps -a --filter label=com.docker.compose.project={project} --format '{{{{.ID}}}}'; docker volume ls --filter label=com.docker.compose.project={project} --format '{{{{.Name}}}}'; docker network ls --filter label=com.docker.compose.project={project} --format '{{{{.ID}}}}'", 90)
    runtime_command = scoped(f"docker compose -f {compose_arg} images --format json && docker compose -f {compose_arg} exec -T web sh -lc 'set -eu; node --version; pnpm --version; sha256sum $(command -v node) $(command -v pnpm)' && docker compose -f {compose_arg} exec -T api sh -lc 'set -eu; iii --version; sha256sum $(command -v iii)'")
    runtime, runtime_evidence = checked("environment-runtime-identity", runtime_command, 120)
    # Tear down only the primary stack while the alternate replay remains.  Its
    # continued presence proves cleanup did not remove another attempt's state.
    primary_absent = f"test -z \"$(docker ps -aq --filter label=com.docker.compose.project={project})\" && test -z \"$(docker volume ls -q --filter label=com.docker.compose.project={project})\" && test -z \"$(docker network ls -q --filter label=com.docker.compose.project={project})\""
    alternate_inventory = f"docker ps -aq --filter label=com.docker.compose.project={alternate_project}; docker volume ls -q --filter label=com.docker.compose.project={alternate_project}; docker network ls -q --filter label=com.docker.compose.project={alternate_project}"
    scope_before, scope_before_evidence = checked("environment-scope-before", alternate_inventory, 60)
    cleanup, cleanup_evidence = checked("environment-cleanup", scoped(teardown) + " && " + primary_absent, 180)
    scope_after, scope_after_evidence = checked("environment-scope-after", alternate_inventory, 60)
    alternate_cleanup, alternate_cleanup_evidence = checked("environment-clean-replay-teardown", scoped(teardown, alternate_project, alternate_web_port, alternate_api_port), 180)
    base, base_evidence = checked("environment-registry-base", "git -C /workspace/registry rev-parse HEAD", 30)

    values = {
        "environment.build": (build, build_evidence),
        "environment.migration": (migrate, migrate_evidence),
        "environment.seed": (seeded and seed.get("exit_code") == 0 and canonical.get("exit_code") == 0, seed_evidence + canonical_evidence),
        "environment.api_readiness": (api, api_evidence),
        "environment.frontend_reachability": (web, web_evidence),
        "environment.artifact_integrity": (artifacts, artifact_evidence),
        "environment.clean_reproduction": (replay.get("exit_code") == 0 and replay_health.get("exit_code") == 0, replay_evidence + replay_health_evidence),
        "environment.restart_persistence": (sentinel_result.get("exit_code") == 0 and restart.get("exit_code") == 0 and persisted.get("stdout", "").strip() == "1", sentinel_evidence + restart_evidence + persisted_evidence),
        "environment.parallel_isolation": (isolation.get("exit_code") == 0 and isolation.get("stdout", "").strip() == "t" and resources.get("exit_code") == 0, isolation_evidence + resources_evidence),
        "environment.cleanup_completeness": (cleanup, cleanup_evidence),
        "environment.cleanup_scope": (scope_before.get("exit_code") == 0 and scope_after.get("exit_code") == 0 and bool(scope_before.get("stdout", "").strip()) and scope_before.get("stdout", "").split() == scope_after.get("stdout", "").split(), scope_before_evidence + cleanup_evidence + scope_after_evidence),
        "environment.registry_base": (base.get("exit_code") == 0 and base.get("stdout", "").strip() == REGISTRY_SHA, base_evidence),
        "environment.runtime_identity": (runtime, runtime_evidence),
        "environment.database_readiness": (db, db_evidence),
    }
    combined_results = {
        "environment.clean_reproduction": [replay, replay_health],
        "environment.seed": [seed, canonical],
        "environment.restart_persistence": [sentinel_result, restart, persisted],
        "environment.parallel_isolation": [isolation, resources],
        "environment.cleanup_scope": [scope_before, cleanup, scope_after],
    }
    observations = []
    for metric in metric_ids:
        value, evidence = values[metric]
        if any(infrastructure_failure(result) for result in combined_results.get(metric, [])):
            observations.append(unavailable(metric, "validator_infrastructure_unavailable"))
            continue
        if isinstance(value, dict):
            observations.append(command_binary(metric, value, [item for item in evidence if item]))
        else:
            observations.append(binary(metric, value, [item for item in evidence if item]))
    return observations


def validate(task_root, assets):
    state = json.loads((task_root / "state.json").read_text())
    test = state["test"]
    if test == 2:
        observations = implementation_observations(task_root, assets, state, test)
    elif test == 4:
        observations = verification_observations(task_root, assets, state)
    elif test == 3:
        observations = environment_observations(task_root, assets, state)
    else:
        observations = []
    result = {"test": test, "observations": observations}
    write_json(task_root / "validation" / "observations.json", result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--assets", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(validate(args.root.resolve(), args.assets.resolve())))


if __name__ == "__main__":
    main()
