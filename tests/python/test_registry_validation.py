import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import uuid
from unittest.mock import patch

ASSETS = Path(__file__).resolve().parents[2] / "tests/fixtures/registry-version-comparison"
spec = importlib.util.spec_from_file_location("registry_validate", ASSETS / "validate.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


# A synthetic planted truth: two checks the probe measures as failing.
PLANTED = frozenset(sorted(module.PUBLIC_VERIFICATION_IDS)[:2])


class ValidationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "evidence.json").write_text("{}")

    def test_truth_counts_and_zero_denominators(self):
        evidence = ["evidence.json"]
        measured = module.ratio("verification.recall", 2, 4, evidence)
        self.assertEqual(measured["status"], "measured")
        self.assertEqual((measured["numerator"], measured["denominator"]), (2, 4))
        na = module.ratio("verification.precision", 0, 0, evidence)
        self.assertEqual(na["status"], "not_applicable")
        self.assertEqual(na["denominator"], 0)
        empty = module.ratio("verification.precision", 0, 0, evidence, empty_value=1)
        self.assertEqual((empty["status"], empty["numerator"], empty["denominator"], empty["value"]),
                         ("measured", 0, 0, 1))
        self.assertEqual(module.binary("implementation.same_version", True, [] )["status"], "unavailable")

    def test_controller_timeout_is_factual_result(self):
        state = {"container": "not-a-real-container"}
        with patch.object(module.subprocess, "run", side_effect=module.subprocess.TimeoutExpired("docker", 1)):
            result = module.controller_command(state, "true", timeout=1)
        self.assertTrue(result["timeout"])
        self.assertIsNone(result["exit_code"])

    def test_evidence_must_remain_inside_run_root(self):
        nested = self.root / "nested"
        nested.mkdir()
        self.assertEqual(module.relative_evidence(self.root, self.root / "evidence.json"), "evidence.json")
        self.assertIsNone(module.relative_evidence(self.root, Path("/etc/hosts")))

    def test_partial_environment_contract_cannot_receive_credit(self):
        task = self.root / "test-3"
        (task / "workspace" / "output").mkdir(parents=True)
        (task / "state.json").write_text("{}")
        (task / "workspace" / "output" / "environment.json").write_text(json.dumps({"compose_file": "registry/compose.yaml"}))
        observations = module.environment_observations(task, ASSETS, {"container": "unused", "web_port": 1, "api_port": 2})
        self.assertTrue(observations)
        self.assertTrue(all(item["status"] == "measured" and item["value"] == 0 for item in observations))

    def verification_task(self):
        task = self.root / "test-4"
        (task / "workspace" / "output").mkdir(parents=True)
        (task / "validation").mkdir()
        (task / "commands").mkdir()
        (task / "validation" / "feature.json").write_text("{}")
        (task / "source.patch").write_bytes(b"")
        return task, {"initial_patch_sha256": module.hashlib.sha256(b"").hexdigest()}

    def command_receipt(self, task, name, command, evidence=(), exit_code=0):
        artifacts = []
        for reference in evidence:
            path = task / reference if reference.startswith("execution-evidence/") else task / "workspace" / reference
            contents = path.read_bytes()
            artifacts.append({"path": reference,
                              "sha256": module.hashlib.sha256(contents).hexdigest(),
                              "size_bytes": len(contents)})
        (task / "commands" / f"{name}.json").write_text(json.dumps({
            "command": command, "exit_code": exit_code, "artifacts": artifacts,
        }))

    def receipted_report(self, task, statuses):
        """checks.json with one receipted proof per check, {id: status}."""
        checks = []
        for check_id, status in sorted(statuses.items()):
            name = f"{status}-{check_id}"
            evidence = self.evidence_proof(task, check_id, status, name)
            command = f"python check.py {name}"
            self.command_receipt(task, name, command, [evidence])
            checks.append({"id": check_id, "status": status, "command": command,
                           "steps": "expected and observed result", "evidence": [evidence]})
        (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": checks}))

    def evidence_proof(self, task, check_id, status, name="proof"):
        reference = f"execution-evidence/{name}/proof.json"
        path = task / reference
        path.parent.mkdir(parents=True, exist_ok=True)
        passed = status == "pass"
        path.write_text(json.dumps({
            "check_id": check_id,
            "status": status,
            "assertions": [{
                "id": "observed_result",
                "passed": passed,
                "expected": {"result": "pass"},
                "observed": {"result": "pass" if passed else "fail"},
            }],
        }))
        return reference

    def feature(self, failed=(), unavailable=()):
        return {"observations": [
            {"id": metric, "status": "unavailable", "reason": "blocked"}
            if metric in unavailable else
            {"id": metric, "status": "measured", "value": 0 if metric in failed else 1}
            for metric in module.PUBLIC_VERIFICATION_IDS
        ], "cases": {}}

    def verify(self, task, state, failed=(), unavailable=()):
        """Validates against a probe that fails exactly `failed`, the planted count."""
        with patch.object(module, "feature_probe",
                          return_value=(self.feature(failed=failed, unavailable=unavailable), None)):
            return {item["id"]: item for item in module.verification_observations(
                task, ASSETS, state, len(failed))}

    def fault_config(self, receipts=True, probe_id=None):
        probe_id = probe_id or str(uuid.uuid4())
        config = {"probe_id": probe_id, "paths": module.FAULT_PATHS}
        status = {**config, "receipts": []}
        if receipts:
            status["receipts"] = [
                {"probe_id": probe_id, "method": "GET", "path": path, "status": 503,
                 "count": 1, "first_sequence": index, "last_sequence": index}
                for index, path in enumerate(module.FAULT_PATHS.values(), 1)
            ]
        return config, status

    def run_feature_probe(self, feature, config=None, status=None, run_error=None,
                          copy_error=False, restart_error=False, clear_error=False):
        task = self.root / "test-2"
        source = task / "workspace/output/validator/feature.json"
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_text(json.dumps(feature))
        commands = []

        def execute(_state, command, timeout=120):
            commands.append(command)
            if command.endswith(" configure"):
                return {"exit_code": 0 if config is not None else 1,
                        "stdout": json.dumps(config) if config is not None else "",
                        "stderr": "" if config is not None else "configure failed"}
            if command.endswith(" status"):
                return {"exit_code": 0 if status is not None else 1,
                        "stdout": json.dumps(status) if status is not None else "",
                        "stderr": "" if status is not None else "status failed"}
            if restart_error and "restart web" in command:
                return {"exit_code": 1, "stdout": "", "stderr": "restart failed"}
            if copy_error and " cp web:" in command:
                return {"exit_code": 1, "stdout": "", "stderr": "copy failed"}
            if clear_error and command.endswith(" clear"):
                return {"exit_code": 1, "stdout": "", "stderr": "clear failed"}
            return {"exit_code": 0, "stdout": "", "stderr": ""}

        completed = module.subprocess.CompletedProcess([], 0, b"probe output", b"")
        with patch.object(module, "controller_command", side_effect=execute):
            with patch.object(module.subprocess, "run", side_effect=run_error or [completed]):
                result = module.feature_probe(task, ASSETS, {"container": "outer"})
        return result, commands, task

    def test_feature_probe_orders_gateway_and_cleans_up_failures(self):
        config, status = self.fault_config()
        (feature, error), commands, task = self.run_feature_probe(self.feature(), config, status)
        self.assertIsNone(error)
        self.assertTrue(feature["fault_gateway"]["receipts_valid"])
        self.assertTrue(feature["fault_gateway"]["restart_ready"])
        self.assertTrue(feature["fault_gateway"]["cleared"])
        self.assertIn(" configure", commands[0])
        self.assertIn("restart web", commands[1])
        self.assertIn("up -d --no-deps --wait --wait-timeout 90 web", commands[1])
        self.assertIn(" cp web:", commands[2])
        self.assertEqual([command.rsplit(" ", 1)[-1] for command in commands[-2:]],
                         ["status", "clear"])
        saved = json.loads((task / "validation/feature.json").read_text())
        self.assertEqual(saved["fault_gateway"]["status"], status)

        for expected, kwargs in (
            ("feature_probe_copy_failed", {"copy_error": True}),
            ("feature_probe_timeout", {
                "run_error": module.subprocess.TimeoutExpired("docker", 180)}),
        ):
            with self.subTest(expected):
                (found, error), commands, _ = self.run_feature_probe(
                    self.feature(), config, status, **kwargs)
                self.assertIsNone(found)
                self.assertEqual(error, expected)
                self.assertEqual([command.rsplit(" ", 1)[-1] for command in commands[-2:]],
                                 ["status", "clear"])

    def test_feature_probe_requires_correlated_receipts(self):
        config, missing = self.fault_config(receipts=False)
        for name, status in (
            ("missing", missing),
            ("wrong probe", self.fault_config(probe_id=str(uuid.uuid4()))[1]),
        ):
            with self.subTest(name):
                (feature, error), _commands, _task = self.run_feature_probe(
                    self.feature(failed={"implementation.stale_results"}), config, status)
                self.assertIsNone(error)
                stale = module.checks_by_id(feature)["implementation.stale_results"]
                self.assertEqual((stale["status"], stale["reason"]),
                                 ("unavailable", "fault_not_exercised"))

    def test_fault_receipts_reject_100_bypass_controls(self):
        for kind in range(3):
            for index in range(100):
                config, status = self.fault_config()
                if kind == 0:
                    status["receipts"].pop()
                elif kind == 1:
                    status["receipts"][index % 2]["probe_id"] = str(uuid.uuid4())
                else:
                    status["receipts"][index % 2]["path"] = f"/untrusted/{index}"
                with self.subTest(index=index, kind=kind):
                    self.assertFalse(module.fault_receipts_valid(config, status))

    def test_feature_probe_preserves_measured_failure_with_receipts(self):
        config, status = self.fault_config()
        feature = self.feature(failed={"implementation.stale_results"})
        (feature, error), _commands, _task = self.run_feature_probe(feature, config, status)
        self.assertIsNone(error)
        self.assertEqual(module.checks_by_id(feature)["implementation.stale_results"], {
            "id": "implementation.stale_results", "status": "measured", "value": 0,
        })

    def test_feature_probe_configure_failure_only_blocks_stale_result(self):
        feature = self.feature(failed={"implementation.function_removal"})
        (feature, error), commands, _task = self.run_feature_probe(feature)
        self.assertIsNone(error)
        checks = module.checks_by_id(feature)
        self.assertEqual(checks["implementation.function_removal"]["value"], 0)
        self.assertEqual(checks["implementation.stale_results"]["reason"],
                         "fault_gateway_prerequisite_missing")
        self.assertEqual([command.rsplit(" ", 1)[-1] for command in commands[-2:]],
                         ["status", "clear"])

    def test_feature_probe_requires_cold_restart_and_cleanup(self):
        config, status = self.fault_config()
        for expected, kwargs in (
            ("web_restart_failed", {"restart_error": True}),
            ("fault_cleanup_failed", {"clear_error": True}),
        ):
            with self.subTest(expected):
                (feature, error), _commands, _task = self.run_feature_probe(
                    self.feature(), config, status, **kwargs)
                self.assertIsNone(error)
                gateway = feature["fault_gateway"]
                self.assertEqual(gateway["restart_ready"], not kwargs.get("restart_error", False))
                self.assertEqual(gateway["cleared"], not kwargs.get("clear_error", False))
                checks = module.checks_by_id(feature)
                self.assertEqual(checks["implementation.function_removal"]["value"], 1)
                stale = checks["implementation.stale_results"]
                self.assertEqual((stale["status"], stale["reason"]),
                                 ("unavailable", expected))

    def test_verification_uses_probe_observations_and_bounded_ids(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        command = "python verify.py"
        real_evidence = self.evidence_proof(task, check_id, "fail", "real")
        invented_evidence = self.evidence_proof(task, "invented.check", "fail", "invented")
        (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": [
            {"id": check_id, "status": "fail", "command": command, "steps": "expected failure; observed failure", "evidence": [real_evidence]},
            {"id": "invented.check", "status": "fail", "command": command, "steps": "expected failure; observed failure", "evidence": [invented_evidence]},
        ]}))
        self.command_receipt(task, "one", command, [real_evidence, invented_evidence])
        found = self.verify(task, state, failed={check_id})
        self.assertEqual((found["verification.recall"]["numerator"], found["verification.recall"]["denominator"]), (22, 22))
        self.assertEqual((found["verification.precision"]["numerator"], found["verification.precision"]["denominator"]), (1, 2))
        self.assertEqual((found["verification.execution_coverage"]["numerator"], found["verification.execution_coverage"]["denominator"]), (1, 23))
        self.assertEqual(found["verification.evidence_coverage"]["denominator"], 23)

    def test_missing_or_invalid_subject_checks_leave_precision_unassessed(self):
        task, state = self.verification_task()
        failed = {"implementation.function_removal"}
        for name, contents in (("missing", None), ("invalid", "{")):
            with self.subTest(name):
                checks = task / "workspace" / "output" / "checks.json"
                if contents is None:
                    checks.unlink(missing_ok=True)
                else:
                    checks.write_text(contents)
                found = self.verify(task, state, failed=failed)
                self.assertEqual((found["verification.recall"]["numerator"], found["verification.recall"]["denominator"]), (0, 22))
                self.assertEqual((found["verification.execution_coverage"]["numerator"], found["verification.execution_coverage"]["denominator"]), (0, 23))
                self.assertEqual((found["verification.precision"]["status"], found["verification.precision"]["numerator"], found["verification.precision"]["denominator"]), ("not_applicable", 0, 0))
                self.assertEqual((found["verification.evidence_coverage"]["numerator"], found["verification.evidence_coverage"]["denominator"], found["verification.evidence_coverage"]["value"]), (0, 0, 0))
                self.assertEqual(found["verification.source_preservation"]["value"], 0)

    def test_no_report_earns_nothing(self):
        task, state = self.verification_task()
        (task / "workspace" / "output" / "checks.json").unlink(missing_ok=True)
        found = self.verify(task, state, failed=PLANTED)
        self.assertEqual(len(found), 5)
        self.assertEqual(found.pop("verification.precision")["status"], "not_applicable")
        for metric, item in found.items():
            self.assertEqual(item["status"], "measured", metric)
            self.assertEqual(item.get("value", item.get("numerator")), 0, metric)

    def test_empty_checks_array_earns_nothing(self):
        task, state = self.verification_task()
        (task / "workspace" / "output" / "checks.json").write_text('{"checks": []}')
        found = self.verify(task, state, failed=PLANTED)
        self.assertEqual(found.pop("verification.precision")["status"], "not_applicable")
        self.assertTrue(all(item.get("value", item.get("numerator")) == 0
                            for item in found.values()))

    def test_a_report_that_only_claims_results_earns_nothing(self):
        task, state = self.verification_task()
        claims = {"checks": [{"id": metric, "status": "pass", "steps": "claimed"}
                             for metric in module.PUBLIC_VERIFICATION_IDS]}
        failures = {"checks": [{"id": metric, "status": "fail", "steps": "claimed"}
                               for metric in module.PUBLIC_VERIFICATION_IDS]}
        for name, report in (
            ("junk entry", {"checks": [{"id": "x"}]}),
            ("every check claimed", claims),
            ("every check claimed failing", failures),
        ):
            with self.subTest(name):
                (task / "workspace" / "output" / "checks.json").write_text(json.dumps(report))
                found = self.verify(task, state, failed=PLANTED)
                for metric, item in found.items():
                    if item["status"] == "measured":
                        self.assertEqual(item.get("value", item.get("numerator")), 0, metric)

    def test_a_reported_failure_counts_only_once_its_check_was_executed(self):
        task, state = self.verification_task()
        executed_id, claimed_id = sorted(module.PUBLIC_VERIFICATION_IDS)[:2]
        evidence = self.evidence_proof(task, executed_id, "fail", "executed")
        command = "python executed.py"
        self.command_receipt(task, "executed", command, [evidence], exit_code=1)
        (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": [
            {"id": executed_id, "status": "fail", "command": command,
             "steps": "expected pass; observed failure", "evidence": [evidence]},
            {"id": claimed_id, "status": "fail", "steps": "claimed without running"},
        ]}))
        found = self.verify(task, state, failed={executed_id, claimed_id})
        self.assertEqual((found["verification.recall"]["numerator"], found["verification.recall"]["denominator"]), (21, 42))
        self.assertEqual((found["verification.precision"]["numerator"], found["verification.precision"]["denominator"]), (1, 2))

    def test_the_planted_failures_found_score_every_verification_metric(self):
        task, state = self.verification_task()
        self.receipted_report(task, {metric: "fail" if metric in PLANTED else "pass"
                                     for metric in module.PUBLIC_VERIFICATION_IDS})
        found = self.verify(task, state, failed=PLANTED)
        self.assertEqual((found["verification.recall"]["numerator"], found["verification.recall"]["denominator"]), (42, 42))
        self.assertEqual((found["verification.precision"]["status"], found["verification.precision"]["numerator"], found["verification.precision"]["denominator"]), ("measured", 2, 2))
        self.assertEqual((found["verification.execution_coverage"]["numerator"], found["verification.evidence_coverage"]["numerator"]), (23, 23))
        self.assertEqual(found["verification.source_preservation"]["value"], 1)
        (task / "source.patch").write_text("changed")
        changed = self.verify(task, state, failed=PLANTED)
        self.assertEqual(changed["verification.source_preservation"]["value"], 0)

    def test_empty_reused_evidence_and_unrelated_command_do_not_count(self):
        task, state = self.verification_task()
        evidence = task / "workspace" / "output" / "empty.txt"
        evidence.write_bytes(b"")
        command = "true"
        self.command_receipt(task, "one", command)
        (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": [
            {"id": metric, "status": "pass", "command": command,
             "steps": "claims success without an observation", "evidence": ["output/empty.txt"]}
            for metric in module.PUBLIC_VERIFICATION_IDS
        ]}))
        found = self.verify(task, state, failed=PLANTED)
        self.assertEqual(found["verification.execution_coverage"]["numerator"], 0)
        self.assertEqual(found["verification.evidence_coverage"]["numerator"], 0)
        receipt = json.loads((task / "validation" / "verification-evidence.json").read_text())
        self.assertTrue(receipt["checks"])
        self.assertTrue(all(not check["evidence"] for check in receipt["checks"]))

    def test_proofs_written_straight_to_output_are_not_execution_evidence(self):
        task, state = self.verification_task()
        checks = []
        for index, metric in enumerate(sorted(module.PUBLIC_VERIFICATION_IDS)):
            reference = f"output/execution-evidence/{index}.json"
            path = task / "workspace" / reference
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps({"check_id": metric, "status": "pass", "assertions": [{
                "id": "observed_result", "passed": True,
                "expected": {"result": "pass"}, "observed": {"result": "pass"},
            }]}))
            command = f"python forge.py {index}"
            self.command_receipt(task, str(index), command, [reference])
            checks.append({"id": metric, "status": "pass", "command": command,
                           "steps": "expected pass; observed pass", "evidence": [reference]})
        (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": checks}))
        found = self.verify(task, state, failed=PLANTED)
        coverage = found["verification.evidence_coverage"]
        self.assertEqual((coverage["numerator"], coverage["denominator"]), (0, 23))

    def test_receipt_provenance_rejects_stale_files_and_accepts_internal_script_output(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        reference = self.evidence_proof(task, check_id, "pass", "receipt")
        evidence = task / reference
        stale_command = "true output/evidence.json"
        self.command_receipt(task, "stale", stale_command)

        def observe(command):
            (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": [{
                "id": check_id, "status": "pass", "command": command,
                "steps": "expected pass; observed pass", "evidence": [reference],
            }]}))
            return self.verify(task, state, failed=PLANTED)

        self.assertEqual(observe(stale_command)["verification.execution_coverage"]["numerator"], 0)

        script_command = "python verify.py"
        self.command_receipt(task, "script", script_command, [reference])
        self.assertEqual(observe(script_command)["verification.execution_coverage"]["numerator"], 1)
        evidence.write_text(json.dumps({"check_id": check_id, "status": "pass", "assertions": [{
            "id": "observed_result", "passed": True,
            "expected": {"result": "pass"}, "observed": {"result": "pass"},
        }], "tampered": True}))
        self.assertEqual(observe(script_command)["verification.execution_coverage"]["numerator"], 0)

    def test_evidence_requires_unique_consistent_assertions(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        reference = "execution-evidence/schema/proof.json"
        path = task / reference
        path.parent.mkdir(parents=True)
        invalid = {
            "empty": {"check_id": check_id, "status": "pass", "assertions": []},
            "duplicate": {"check_id": check_id, "status": "pass", "assertions": [
                {"id": "same", "passed": True, "expected": {"ok": True}, "observed": {"ok": True}},
                {"id": "same", "passed": True, "expected": {"ok": True}, "observed": {"ok": True}},
            ]},
            "missing-expected": {"check_id": check_id, "status": "pass", "assertions": [
                {"id": "claim", "passed": True, "observed": []},
            ]},
            "missing-observed": {"check_id": check_id, "status": "pass", "assertions": [
                {"id": "claim", "passed": True, "expected": []},
            ]},
            "typed-mismatch": {"check_id": check_id, "status": "pass", "assertions": [
                {"id": "claim", "passed": True, "expected": True, "observed": 1},
            ]},
            "inconsistent": {"check_id": check_id, "status": "fail", "assertions": [
                {"id": "claim", "passed": False, "expected": {"ok": True}, "observed": {"ok": True}},
            ]},
        }
        for name, proof in invalid.items():
            with self.subTest(name):
                path.write_text(json.dumps(proof))
                command = f"python {name}.py"
                self.command_receipt(task, name, command, [reference])
                (task / "workspace/output/checks.json").write_text(json.dumps({"checks": [{
                    "id": check_id, "status": proof["status"], "command": command,
                    "steps": "claimed result", "evidence": [reference],
                }]}))
                found = self.verify(task, state, failed=PLANTED)
                self.assertEqual(found["verification.evidence_coverage"]["numerator"], 0)
                self.assertEqual(found["verification.execution_coverage"]["numerator"], 0)

        self_reference = "output/checks.json"
        command = "python circular.py"
        (task / "workspace/output/checks.json").write_text(json.dumps({"checks": [{
            "id": check_id, "status": "pass", "command": command,
            "steps": "self-reported result", "evidence": [self_reference],
        }]}))
        self.command_receipt(task, "circular", command, [self_reference])
        found = self.verify(task, state, failed=PLANTED)
        self.assertEqual(found["verification.evidence_coverage"]["numerator"], 0)

    def test_evidence_accepts_equal_empty_null_and_string_json_values(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        reference = "execution-evidence/json-values/proof.json"
        path = task / reference
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps({
            "check_id": check_id,
            "status": "pass",
            "assertions": [
                {"id": "empty-array", "passed": True, "expected": [], "observed": []},
                {"id": "null", "passed": True, "expected": None, "observed": None},
                {"id": "string", "passed": True, "expected": "ok", "observed": "ok"},
            ],
        }))
        command = "python json-values.py"
        self.command_receipt(task, "json-values", command, [reference])
        (task / "workspace/output/checks.json").write_text(json.dumps({"checks": [{
            "id": check_id, "status": "pass", "command": command,
            "steps": "compared exact JSON values", "evidence": [reference],
        }]}))
        found = self.verify(task, state, failed=PLANTED)
        self.assertEqual(found["verification.evidence_coverage"]["numerator"], 1)
        self.assertEqual(found["verification.execution_coverage"]["numerator"], 1)

    def test_false_positive_and_missed_failure_are_scored_independently(self):
        task, state = self.verification_task()
        case_id = "implementation.function_removal"
        for name, actual_failures, reported_status, expected_recall, expected_precision in (
            ("false positive", PLANTED, "fail", (0, 42, None), (0, 1, None)),
            ("missed failure", {case_id, min(PLANTED)}, "pass", (0, 42, None), (0, 0, None)),
        ):
            with self.subTest(name):
                evidence = self.evidence_proof(task, case_id, reported_status,
                                               name.replace(" ", "-"))
                command = f"python {name.replace(' ', '-')}.py"
                self.command_receipt(task, name.replace(" ", "-"), command, [evidence],
                                     exit_code=1 if reported_status == "fail" else 0)
                (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": [
                    {"id": case_id, "status": reported_status, "command": command,
                     "steps": "expected and observed result",
                     "evidence": [evidence]},
                ]}))
                found = self.verify(task, state, failed=actual_failures)
                recall = found["verification.recall"]
                precision = found["verification.precision"]
                self.assertEqual((recall["numerator"], recall["denominator"], recall.get("value")), expected_recall)
                self.assertEqual((precision["numerator"], precision["denominator"], precision.get("value")), expected_precision)
                if reported_status != "fail":
                    self.assertEqual(precision["status"], "not_applicable")

    def test_a_truth_that_is_not_the_planted_delivery_s_is_not_scored(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        evidence = self.evidence_proof(task, check_id, "fail")
        command = "python verify.py"
        self.command_receipt(task, "one", command, [evidence], exit_code=1)
        report = json.dumps({"checks": [{"id": check_id, "status": "fail", "command": command,
                                         "steps": "expected pass; observed failure",
                                         "evidence": [evidence]}]})
        checks = task / "workspace" / "output" / "checks.json"
        for name, contents in (("readable report", report), ("no report", None)):
            with self.subTest(name):
                if contents is None:
                    checks.unlink(missing_ok=True)
                else:
                    checks.write_text(contents)
                with patch.object(module, "feature_probe",
                                  return_value=(self.feature(failed={check_id}), None)):
                    found = {item["id"]: item for item in module.verification_observations(
                        task, ASSETS, state, 2)}
                recall = found["verification.recall"]
                self.assertEqual((recall["status"], recall.get("reason")),
                                 ("unavailable", "fixed_delivery_truth_mismatch"))

    def test_a_probe_that_disagrees_is_run_once_more(self):
        task, state = self.verification_task()
        right = (self.feature(failed=PLANTED), None)
        for name, answers in (("wrong, then right", [(self.feature(failed={min(PLANTED)}), None), right]),
                              ("right at once", [right])):
            with self.subTest(name):
                with patch.object(module, "feature_probe", side_effect=answers) as probe:
                    found = {item["id"]: item for item in module.verification_observations(
                        task, ASSETS, state, 2)}
                self.assertEqual(found["verification.recall"]["status"], "measured")
                self.assertEqual(probe.call_count, len(answers))

    def test_a_check_is_executed_only_when_the_probe_saw_the_same_outcome(self):
        task, state = self.verification_task()
        for status, executed in (("fail", 2), ("pass", 21)):
            with self.subTest(status):
                self.receipted_report(task, dict.fromkeys(module.PUBLIC_VERIFICATION_IDS, status))
                coverage = self.verify(task, state, failed=PLANTED)["verification.execution_coverage"]
                self.assertEqual((coverage["numerator"], coverage["denominator"]), (executed, 23))

    def test_recall_is_net_of_false_alarms(self):
        task, state = self.verification_task()
        ids = sorted(module.PUBLIC_VERIFICATION_IDS)
        truth = set(ids[:4])
        for name, failures, recall in (
            ("all fail", ids, 0),
            ("two found, one false alarm", ids[:2] + ids[4:5], 34),
            ("all found", ids[:4], 76),
        ):
            with self.subTest(name):
                self.receipted_report(task, dict.fromkeys(failures, "fail"))
                found = self.verify(task, state, failed=truth)["verification.recall"]
                self.assertEqual((found["numerator"], found["denominator"]), (recall, 76))
        (task / "workspace" / "output" / "checks.json").unlink()
        found = self.verify(task, state, failed=truth)
        self.assertEqual((found["verification.recall"]["numerator"],
                          found["verification.recall"]["denominator"]), (0, 76))
        self.assertEqual(found["verification.precision"]["status"], "not_applicable")

    def test_verification_requires_the_planted_count(self):
        task, _state = self.verification_task()
        (task / "state.json").write_text(json.dumps({"test": 4}))
        with self.assertRaisesRegex(ValueError, "planted"):
            module.validate(task, ASSETS)

    def test_blocked_independent_probe_keeps_truth_unavailable(self):
        task, state = self.verification_task()
        blocked = {"implementation.function_removal"}
        found = self.verify(task, state, failed=PLANTED, unavailable=blocked)
        self.assertEqual(found["verification.recall"]["status"], "unavailable")

    def test_implementation_consumes_real_probe_observation_shape(self):
        task = self.root / "test-2"
        (task / "validation").mkdir(parents=True)
        (task / "validation" / "feature.json").write_text("{}")
        feature = {"observations": [
            {"id": metric, "status": "measured", "value": int(metric == "implementation.same_version")}
            for metric in module.IMPLEMENTATION_METRICS if metric != "implementation.patch_application"
        ], "cases": {}}
        with patch.object(module, "feature_probe", return_value=(feature, None)):
            found = {item["id"]: item for item in module.implementation_observations(task, ASSETS, {}, 2)}
        self.assertEqual(found["implementation.same_version"]["value"], 1)
        self.assertEqual(found["implementation.function_removal"]["value"], 0)

    def test_patch_checkout_failure_is_not_scored_as_a_bad_patch(self):
        task = self.root / "test-2"
        (task / "workspace" / "output").mkdir(parents=True)
        (task / "validation").mkdir()
        (task / "source.patch").write_text("patch")
        (task / "validation" / "feature.json").write_text("{}")
        state = {"container": "unused"}
        with patch.object(module, "feature_probe", return_value=(self.feature(), None)):
            with patch.object(module, "controller_command", return_value={"exit_code": 128, "stdout": "", "stderr": "dubious ownership"}) as execute:
                found = {item["id"]: item for item in module.implementation_observations(task, ASSETS, state, 2)}
            self.assertEqual(found["implementation.patch_application"], {
                "id": "implementation.patch_application", "status": "unavailable",
                "reason": "patch_replay_checkout_failed",
            })
            self.assertEqual(execute.call_count, 1)
            self.assertIn("safe.directory /workspace/registry/.git", execute.call_args.args[1])

            with patch.object(module, "controller_command", side_effect=[
                {"exit_code": 0, "stdout": "", "stderr": ""},
                {"exit_code": 1, "stdout": "", "stderr": "patch does not apply"},
            ]):
                found = {item["id"]: item for item in module.implementation_observations(task, ASSETS, state, 2)}
            self.assertEqual(found["implementation.patch_application"]["status"], "measured")
            self.assertEqual(found["implementation.patch_application"]["value"], 0)

    def test_environment_uses_restart_and_exact_project_cleanup(self):
        task = self.root / "test-3"
        (task / "workspace" / "output").mkdir(parents=True)
        (task / "state.json").write_text("{}")
        (task / "workspace" / "output" / "environment.json").write_text(json.dumps({
            "compose_file": "registry/compose.yaml", "startup_command": "docker compose up -d",
            "teardown_command": "docker compose down -v", "migration_command": "docker compose run migrate",
            "db_service": "db", "db_user": "fixture", "db_name": "registry_fixture",
        }))
        commands = []
        def execute(_state, command, timeout=120):
            commands.append(command)
            stdout = ""
            if "socket.socket()" in command:
                stdout = "52340 52341\n"
            elif "config --format json" in command:
                stdout = json.dumps({"services": {"web": {"ports": [
                    {"published": "65000", "target": 3000},
                ]}}})
            elif "select version" in command:
                stdout = "0.9.0\n1.0.0\n1.1.0\n2.0.0\n"
            elif "select (select count" in command or "to_regclass" in command:
                stdout = "t\n"
            elif "validator_restart_sentinel" in command and "select count" in command:
                stdout = "1\n"
            elif "rev-parse HEAD" in command:
                stdout = module.REGISTRY_SHA + "\n"
            elif "validator-" in command and "--filter label=com.docker.compose.project" in command:
                stdout = "container\nvolume\nnetwork\n"
            return {"exit_code": 0, "stdout": stdout, "stderr": ""}
        state = {"container": "outer-123456789012", "web_port": 65000, "api_port": 65001}
        with patch.object(module, "controller_command", side_effect=execute):
            module.environment_observations(task, ASSETS, state)
        self.assertTrue(any("docker compose -f /workspace/registry/compose.yaml restart" in command for command in commands))
        self.assertTrue(any("--filter label=com.docker.compose.project=validator-123456789012" in command for command in commands))
        self.assertTrue(any("WEB_PORT=52340 API_PORT=52341" in command for command in commands))
        self.assertFalse(any("41000" in command or "41001" in command for command in commands))
        browser = next(command for command in commands if "environment-web" not in command and "E2E_APP_URL=" in command)
        self.assertIn("compose -f /workspace/registry/compose.yaml exec -T", browser)
        self.assertIn("E2E_APP_URL=http://127.0.0.1:3000", browser)
        self.assertNotIn("docker run", browser)

        def execute_without_ports(state, command, timeout=120):
            if "socket.socket()" in command:
                return {"exit_code": 1, "stdout": "", "stderr": "cannot allocate ports"}
            return execute(state, command, timeout)
        with patch.object(module, "controller_command", side_effect=execute_without_ports):
            found = {item["id"]: item for item in module.environment_observations(task, ASSETS, state)}
        for metric in ("environment.clean_reproduction", "environment.parallel_isolation", "environment.cleanup_scope"):
            self.assertEqual(found[metric]["status"], "unavailable")
            self.assertEqual(found[metric]["reason"], "validator_port_allocation_unavailable")
        self.assertEqual(found["environment.cleanup_completeness"]["status"], "measured")


if __name__ == "__main__":
    unittest.main()
