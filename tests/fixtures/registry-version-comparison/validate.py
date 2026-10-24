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
import uuid

REGISTRY_SHA = "662eb87c1bdbb395f36264d5d26bf823e2ace783"
IMPLEMENTATION_METRICS = (
    "implementation.same_version", "implementation.function_removal", "implementation.required_impact",
    "implementation.required_order", "implementation.enum_order",
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
FAULT_PATHS = {
    "navigation": "/w/orders-worker/compare/1.1.0...2.0.0",
    "interaction": "/w/orders-worker/compare/1.0.0...1.1.0",
}
FAULT_COMMAND = "docker compose -f /fixture/compose.yaml exec -T fault node /fixture/fault-gateway.cjs"


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
        if not isinstance(reference, str):
            continue
        candidate = Path(reference)
        if candidate.is_absolute():
            # Feature probe paths are inside /workspace, whose host counterpart
            # is this task's workspace; any other absolute location is rejected.
            try:
                candidate = task_root / "workspace" / candidate.relative_to("/workspace")
            except ValueError:
                continue
        elif candidate.parts[:1] == ("execution-evidence",):
            candidate = task_root / candidate
        else:
            candidate = task_root / "workspace" / candidate
        item = relative_evidence(run_root, candidate)
        if item and candidate.stat().st_size:
            evidence.append({"path": item, "reference": reference,
                             "size_bytes": candidate.stat().st_size,
                             "sha256": hashlib.sha256(candidate.read_bytes()).hexdigest()})
    return sorted(evidence, key=lambda item: item["path"])


def fault_receipts_valid(config, status):
    if not isinstance(config, dict) or not isinstance(status, dict):
        return False
    probe_id = config.get("probe_id")
    try:
        uuid.UUID(probe_id)
    except (TypeError, ValueError, AttributeError):
        return False
    if (config.get("paths") != FAULT_PATHS or status.get("probe_id") != probe_id
            or status.get("paths") != FAULT_PATHS or not isinstance(status.get("receipts"), list)):
        return False
    paths = set()
    for receipt in status["receipts"]:
        if not isinstance(receipt, dict):
            continue
        count = receipt.get("count")
        first = receipt.get("first_sequence")
        last = receipt.get("last_sequence")
        if (receipt.get("probe_id") == probe_id and receipt.get("method") == "GET"
                and receipt.get("path") in FAULT_PATHS.values() and receipt.get("status") == 503
                and type(count) is int and count > 0 and type(first) is int
                and type(last) is int and 0 < first <= last):
            paths.add(receipt["path"])
    return paths == set(FAULT_PATHS.values())


def feature_probe(task_root, assets, state):
    """Run the independent probe in the real web container and collect its JSON."""
    script = assets / "validate-feature.cjs"
    if not script.is_file():
        return None, "feature_probe_not_supplied"
    validation = task_root / "validation"
    validation.mkdir(parents=True, exist_ok=True)
    config = None
    feature = None
    error = None
    status = None
    restart_ready = False
    cleared = False
    try:
        configure = controller_command(state, f"{FAULT_COMMAND} configure", 30)
        save_command(task_root, "fault-gateway-configure", configure)
        if configure.get("exit_code") == 0:
            try:
                config = json.loads(configure.get("stdout", ""))
            except json.JSONDecodeError:
                pass
        restart = controller_command(
            state,
            "docker compose -f /fixture/compose.yaml restart web && "
            "docker compose -f /fixture/compose.yaml up -d --no-deps --wait --wait-timeout 90 web",
            120,
        )
        save_command(task_root, "feature-probe-web-restart", restart)
        restart_ready = restart.get("exit_code") == 0
        command = ["docker", "exec", "-i", state["container"], "docker", "compose", "-f",
                   "/fixture/compose.yaml", "exec", "-T", "web", "node", "-"]
        try:
            result = subprocess.run(command, input=script.read_bytes(), stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, timeout=180, check=False)
        except subprocess.TimeoutExpired as exception:
            result = exception
            error = "feature_probe_timeout"
        output = (getattr(result, "stdout", None) or b"") + (getattr(result, "stderr", None) or b"")
        (validation / "feature-probe.log").write_bytes(output)
        if error is None and result.returncode:
            error = "feature_probe_failed"
        if error is None:
            copied = controller_command(state, "mkdir -p /workspace/output/validator && docker compose -f /fixture/compose.yaml cp web:/tmp/registry-validation/. /workspace/output/validator/", 120)
            save_command(task_root, "feature-probe-copy", copied)
            if copied.get("exit_code") != 0:
                error = "feature_probe_copy_failed"
            else:
                source = task_root / "workspace" / "output" / "validator" / "feature.json"
                if not source.is_file():
                    error = "feature_probe_output_missing"
                else:
                    try:
                        feature = json.loads(source.read_text())
                    except json.JSONDecodeError:
                        error = "feature_probe_output_invalid"
    finally:
        try:
            status_result = controller_command(state, f"{FAULT_COMMAND} status", 30)
            save_command(task_root, "fault-gateway-status", status_result)
            if status_result.get("exit_code") == 0:
                try:
                    status = json.loads(status_result.get("stdout", ""))
                except json.JSONDecodeError:
                    pass
        finally:
            clear = controller_command(state, f"{FAULT_COMMAND} clear", 30)
            save_command(task_root, "fault-gateway-clear", clear)
            cleared = clear.get("exit_code") == 0

    if feature is not None:
        receipts_valid = fault_receipts_valid(config, status)
        try:
            valid_probe_id = str(uuid.UUID(config.get("probe_id"))) == config.get("probe_id")
        except (AttributeError, TypeError, ValueError):
            valid_probe_id = False
        configured = (isinstance(config, dict) and valid_probe_id
                      and config.get("paths") == FAULT_PATHS)
        feature["fault_gateway"] = {
            "trusted": True,
            "configured": configured,
            "restart_ready": restart_ready,
            "receipts_valid": receipts_valid,
            "cleared": cleared,
            "status": status,
        }
        reason = None
        if not cleared:
            reason = "fault_cleanup_failed"
        elif not configured:
            reason = "fault_gateway_prerequisite_missing"
        elif not restart_ready:
            reason = "web_restart_failed"
        elif not receipts_valid:
            reason = "fault_not_exercised"
        if reason:
            observations = feature.get("observations")
            if isinstance(observations, list):
                replacement = unavailable("implementation.stale_results", reason)
                stale = next((item for item in observations
                              if isinstance(item, dict)
                              and item.get("id") == "implementation.stale_results"), None)
                if stale is None:
                    observations.append(replacement)
                else:
                    stale.clear()
                    stale.update(replacement)
        write_json(validation / "feature.json", feature)
    return feature, error


def checks_by_id(feature):
    checks = feature.get("observations", []) if isinstance(feature, dict) else []
    return {item.get("id"): item for item in checks if isinstance(item, dict) and isinstance(item.get("id"), str)}


def recorded_commands(task_root):
    commands = {}
    for path in (task_root / "commands").glob("*.json"):
        try:
            item = json.loads(path.read_text())
        except (json.JSONDecodeError, OSError):
            continue
        if (isinstance(item.get("command"), str) and item.get("exit_code") not in (124, 137)
                and not item.get("timeout") and isinstance(item.get("artifacts"), list)):
            receipt = {"path": relative_evidence(task_root.parent, path),
                       "artifacts": item["artifacts"]}
            commands.setdefault(item["command"], []).append(receipt)
    return commands


def verification_evidence(task_root, report):
    run_root = task_root.parent
    attributable = {}
    claimed = {}
    for case_id, case in report.items():
        for item in actual_evidence(run_root, task_root, case.get("evidence")):
            claimed.setdefault(item["path"], set()).add(case_id)
            path = run_root / item["path"]
            if "/execution-evidence/" not in item["path"] or path.suffix != ".json":
                continue
            try:
                proof = json.loads(path.read_text())
            except (json.JSONDecodeError, OSError):
                continue
            if not isinstance(proof, dict):
                continue
            assertions = proof.get("assertions")
            if (proof.get("check_id") != case_id or proof.get("status") != case.get("status")
                    or not isinstance(assertions, list) or not assertions):
                continue
            assertion_ids = [assertion.get("id") for assertion in assertions
                             if isinstance(assertion, dict)]
            if (len(assertion_ids) != len(assertions) or len(set(assertion_ids)) != len(assertions)
                    or not all(isinstance(assertion_id, str) and assertion_id.strip()
                               for assertion_id in assertion_ids)):
                continue
            def json_equal(left, right):
                try:
                    options = {"sort_keys": True, "separators": (",", ":"), "allow_nan": False}
                    return json.dumps(left, **options) == json.dumps(right, **options)
                except (TypeError, ValueError):
                    return False
            if not all(isinstance(assertion.get("passed"), bool)
                       and "expected" in assertion and "observed" in assertion
                       and (json_equal(assertion["expected"], assertion["observed"])
                            == assertion["passed"])
                       for assertion in assertions):
                continue
            passed = [assertion["passed"] for assertion in assertions]
            if ((case.get("status") == "pass" and not all(passed))
                    or (case.get("status") == "fail" and all(passed))):
                continue
            attributable.setdefault(case_id, []).append(item)
    for case_id, items in attributable.items():
        attributable[case_id] = [item for item in items
                                 if claimed.get(item["path"]) == {case_id}]
    return attributable


def receipt_proves(receipts, evidence):
    return any(
        artifact.get("path") == evidence["reference"]
        and artifact.get("sha256", "").removeprefix("sha256:") == evidence["sha256"]
        and artifact.get("size_bytes") == evidence["size_bytes"]
        for receipt in receipts or []
        for artifact in receipt.get("artifacts", [])
        if isinstance(artifact, dict)
    )


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
    if not reported:
        empty_evidence = [relative_evidence(run_root, checks_path), feature_evidence]
        return empty_verification(task_root, state, run_root, truth_complete, known_failed,
                                  [item for item in empty_evidence if item])
    report = {case["id"]: case for case in reported if isinstance(case, dict)
              and isinstance(case.get("id"), str)}
    duplicate_ids = {case_id for case_id in report
                     if sum(case.get("id") == case_id for case in reported
                            if isinstance(case, dict)) > 1}
    report_fail = {case_id for case_id, case in report.items() if case.get("status") == "fail"}
    commands = recorded_commands(task_root)
    evidence_by_id = verification_evidence(task_root, report)
    evidenced = {case_id for case_id, case in report.items()
                 if case_id in PUBLIC_VERIFICATION_IDS and case_id not in duplicate_ids
                 and case.get("status") in ("pass", "fail", "blocked")
                 and isinstance(case.get("steps"), str) and case["steps"].strip()
                 and evidence_by_id.get(case_id)}
    executed = {case_id for case_id, case in report.items()
                if case_id in PUBLIC_VERIFICATION_IDS and case.get("status") in ("pass", "fail")
                and isinstance(case.get("command"), str) and case["command"] in commands
                and case_id in evidenced
                and any(receipt_proves(commands[case["command"]], item)
                        for item in evidence_by_id[case_id])}
    receipt_path = task_root / "validation" / "verification-evidence.json"
    write_json(receipt_path, {"checks": [{"id": case_id,
                                          "command_receipts": commands.get(case.get("command"), []),
                                          "evidence": evidence_by_id.get(case_id, [])}
                                         for case_id, case in sorted(report.items())]})
    report_evidence = relative_evidence(run_root, checks_path)
    validation_evidence = relative_evidence(run_root, receipt_path)
    evidence = [item for item in (report_evidence, feature_evidence, validation_evidence) if item]
    # A report that executed nothing only claims: its empty failure sets and
    # its untouched source hold because nothing was attempted.
    empty_value = 1 if executed else 0
    observations = []
    if not truth_complete:
        observations.append(unavailable("verification.recall", "independent_truth_incomplete"))
    elif not known_failed:
        observations.append(ratio("verification.recall", 0, 0, evidence,
                                  "no_independently_failing_checks", empty_value=empty_value))
    else:
        observations.append(ratio("verification.recall", len(known_failed & report_fail), len(known_failed), evidence))
    if any(independent.get(case_id, {}).get("status") != "measured"
           for case_id in report_fail & PUBLIC_VERIFICATION_IDS):
        observations.append(unavailable("verification.precision", "reported_failure_truth_unavailable"))
    elif not report_fail and not known_failed:
        observations.append(ratio("verification.precision", 0, 0, evidence,
                                  "no_failures_to_reproduce", empty_value=empty_value))
    elif not report_fail:
        observations.append(ratio("verification.precision", 0, 0, evidence,
                                  "no_reported_failures"))
    else:
        observations.append(ratio("verification.precision", len(known_failed & report_fail), len(report_fail), evidence))
    observations.append(ratio("verification.execution_coverage", len(executed), len(PUBLIC_VERIFICATION_IDS), evidence))
    observations.append(ratio("verification.evidence_coverage", len(evidenced),
                              len(PUBLIC_VERIFICATION_IDS), evidence))
    observations.extend(source_preservation(task_root, state, run_root, reported=bool(executed)))
    return observations


def empty_verification(task_root, state, run_root, truth_complete, known_failed, evidence):
    observations = []
    if truth_complete:
        observations.append(ratio("verification.recall", 0, len(known_failed), evidence)
                            if known_failed else ratio("verification.recall", 0, 0, evidence,
                                                      "no_subject_report", empty_value=0))
    else:
        observations.append(unavailable("verification.recall", "independent_truth_incomplete"))
    observations.append(ratio("verification.precision", 0, 0, evidence,
                              "no_reported_failures"))
    observations.append(ratio("verification.execution_coverage", 0,
                              len(PUBLIC_VERIFICATION_IDS), evidence))
    observations.append(ratio("verification.evidence_coverage", 0, 0, evidence,
                              "no_reported_outcomes", empty_value=0))
    observations.extend(source_preservation(task_root, state, run_root, reported=False))
    return observations


def source_preservation(task_root, state, run_root, reported=True):
    """An unchanged source counts only beside a report with an executed check:
    a tester that ran nothing left the source alone by doing nothing."""
    source_patch = task_root / "source.patch"
    state_path = task_root / "state.json"
    evidence = [relative_evidence(run_root, source_patch), relative_evidence(run_root, state_path)]
    expected_patch = state.get("initial_patch_sha256")
    if isinstance(expected_patch, str) and source_patch.is_file():
        unchanged = hashlib.sha256(source_patch.read_bytes()).hexdigest() == expected_patch
        return [binary("verification.source_preservation", reported and unchanged, [item for item in evidence if item])]
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
    web_config, web_evidence = checked("environment-web-config", scoped(f"docker compose -f {compose_arg} config --format json"), 60)
    web, web_error = None, None
    if web_config.get("exit_code") != 0:
        web_error = "compose_config_unavailable"
    else:
        try:
            config = json.loads(web_config.get("stdout", ""))
            mappings = [item["target"] for item in config["services"]["web"]["ports"]
                        if str(item.get("published")) == str(state["web_port"])]
            if len(mappings) != 1 or not 0 < int(mappings[0]) < 65536:
                raise ValueError
            target_port = int(mappings[0])
        except (json.JSONDecodeError, AttributeError, KeyError, TypeError, ValueError):
            web_error = "web_port_mapping_unavailable"
        else:
            web, browser_evidence = checked("environment-web-browser", scoped(
                f"docker compose -f {compose_arg} exec -T -e E2E_APP_URL=http://127.0.0.1:{target_port} "
                f"web node -e {shlex.quote(browser_script)}"), 120)
            web_evidence += browser_evidence
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
    port_allocation, port_evidence = checked(
        "environment-replay-ports",
        "python3 -c 'import socket; s=[socket.socket(),socket.socket()]; "
        "[x.bind((\"127.0.0.1\",0)) for x in s]; print(*(x.getsockname()[1] for x in s))'",
        30,
    )
    try:
        alternate_web_port, alternate_api_port = map(int, port_allocation.get("stdout", "").split())
        if (port_allocation.get("exit_code") != 0 or alternate_web_port == alternate_api_port
                or not all(0 < port < 65536 for port in (alternate_web_port, alternate_api_port))):
            raise ValueError
    except (TypeError, ValueError):
        alternate_web_port = alternate_api_port = None
    skipped_replay = {"exit_code": None, "stdout": "", "stderr": "replay ports unavailable"}
    if alternate_web_port is None:
        replay = replay_health = skipped_replay
        replay_evidence = replay_health_evidence = port_evidence
    else:
        replay, replay_evidence = checked("environment-clean-replay", scoped(startup, alternate_project, alternate_web_port, alternate_api_port), 600)
        replay_health, replay_health_evidence = checked("environment-clean-replay-health", f"curl -fsS http://127.0.0.1:{alternate_api_port}/health | python3 -c {shlex.quote(health_assertion)}", 60)
    sentinel_create = (f"docker compose -f {compose_arg} exec -T {db_service} {psql} -v ON_ERROR_STOP=1 -Atc "
                       "\"create table validator_isolation_sentinel(id integer); insert into validator_isolation_sentinel values (1);\"")
    sentinel_absent = (f"docker compose -f {compose_arg} exec -T {db_service} {psql} -Atc "
                       "\"select to_regclass('validator_isolation_sentinel') is null\"")
    if alternate_web_port is None:
        isolation, isolation_evidence = skipped_replay, port_evidence
    else:
        isolation_command = scoped(sentinel_create) + " >/dev/null && " + scoped(sentinel_absent, alternate_project, alternate_web_port, alternate_api_port)
        isolation, isolation_evidence = checked("environment-isolation", isolation_command, 180)
    resources, resources_evidence = checked("environment-resources", f"docker ps -a --filter label=com.docker.compose.project={project} --format '{{{{.ID}}}}'; docker volume ls --filter label=com.docker.compose.project={project} --format '{{{{.Name}}}}'; docker network ls --filter label=com.docker.compose.project={project} --format '{{{{.ID}}}}'", 90)
    runtime_command = scoped(f"docker compose -f {compose_arg} images --format json && docker compose -f {compose_arg} exec -T web sh -lc 'set -eu; node --version; pnpm --version; sha256sum $(command -v node) $(command -v pnpm)' && docker compose -f {compose_arg} exec -T api sh -lc 'set -eu; iii --version; sha256sum $(command -v iii)'")
    runtime, runtime_evidence = checked("environment-runtime-identity", runtime_command, 120)
    # Tear down only the primary stack while the alternate replay remains.  Its
    # continued presence proves cleanup did not remove another attempt's state.
    primary_absent = f"test -z \"$(docker ps -aq --filter label=com.docker.compose.project={project})\" && test -z \"$(docker volume ls -q --filter label=com.docker.compose.project={project})\" && test -z \"$(docker network ls -q --filter label=com.docker.compose.project={project})\""
    alternate_inventory = f"docker ps -aq --filter label=com.docker.compose.project={alternate_project}; docker volume ls -q --filter label=com.docker.compose.project={alternate_project}; docker network ls -q --filter label=com.docker.compose.project={alternate_project}"
    if alternate_web_port is None:
        scope_before, scope_before_evidence = skipped_replay, port_evidence
    else:
        scope_before, scope_before_evidence = checked("environment-scope-before", alternate_inventory, 60)
    cleanup, cleanup_evidence = checked("environment-cleanup", scoped(teardown) + " && " + primary_absent, 180)
    if alternate_web_port is None:
        scope_after, scope_after_evidence = skipped_replay, port_evidence
    else:
        scope_after, scope_after_evidence = checked("environment-scope-after", alternate_inventory, 60)
        checked("environment-clean-replay-teardown", scoped(teardown, alternate_project, alternate_web_port, alternate_api_port), 180)
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
        if alternate_web_port is None and metric in (
                "environment.clean_reproduction", "environment.parallel_isolation", "environment.cleanup_scope"):
            observations.append(unavailable(metric, "validator_port_allocation_unavailable"))
            continue
        if metric == "environment.frontend_reachability" and web_error:
            observations.append(unavailable(metric, web_error))
            continue
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
