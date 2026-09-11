import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ASSETS = Path(__file__).resolve().parents[2] / "tests/fixtures/registry-version-comparison"
spec = importlib.util.spec_from_file_location("registry_validate", ASSETS / "validate.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ValidationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "evidence.json").write_text("{}")

    def test_ratio_retains_factual_counts(self):
        evidence = ["evidence.json"]
        measured = module.ratio("verification.outcome_accuracy", 2, 4, evidence)
        self.assertEqual(measured["status"], "measured")
        self.assertEqual((measured["numerator"], measured["denominator"]), (2, 4))
        na = module.ratio("verification.outcome_accuracy", 0, 0, evidence)
        self.assertEqual(na["status"], "not_applicable")
        self.assertEqual(na["denominator"], 0)
        self.assertEqual(module.binary("implementation.same_version", True, [] )["status"], "unavailable")

    def test_controller_timeout_is_factual_result(self):
        state = {"container": "not-a-real-container"}
        with patch.object(module.subprocess, "run", side_effect=module.subprocess.TimeoutExpired("docker", 1)):
            result = module.controller_command(state, "true", timeout=1)
        self.assertTrue(result["timeout"])
        self.assertIsNone(result["exit_code"])

    def test_feature_probe_pipes_shared_detail_helper_into_web_runtime(self):
        task = self.root / "probe-runtime"
        task.mkdir()
        captured = {}

        def run(command, **kwargs):
            captured["command"] = command
            captured["input"] = kwargs["input"].decode()
            return module.subprocess.CompletedProcess(command, 0, stdout=b"", stderr=b"")

        with patch.object(module.subprocess, "run", side_effect=run), \
             patch.object(module, "controller_command", return_value={
                 "exit_code": 1, "stdout": "", "stderr": "copy deliberately stopped",
             }):
            feature, error = module.feature_probe(task, ASSETS, {"container": "outer"})
        self.assertIsNone(feature)
        self.assertEqual(error, "feature_probe_copy_failed")
        self.assertIn("registryDetailProbeSource", captured["input"])
        self.assertIn("globalThis.registryDetailProbe", captured["input"])
        self.assertIn("docker compose", " ".join(captured["command"]))

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

    def feature(self, failed=(), unavailable=()):
        return {"observations": [
            {"id": metric, "status": "unavailable", "reason": "blocked"}
            if metric in unavailable else
            {"id": metric, "status": "measured", "value": 0 if metric in failed else 1}
            for metric in module.PUBLIC_VERIFICATION_IDS
        ], "cases": {}}

    def record_command(self, task, command_id, command, exit_code=0, stdout="observed\n"):
        (task / "commands" / f"{command_id}.json").write_text(json.dumps({
            "command_id": command_id, "command": command, "exit_code": exit_code,
            "stdout": stdout, "stderr": "",
        }))

    def write_check(self, task, check):
        (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": [check]}))

    def test_verification_scores_correct_passes_and_failures_over_all_required_ids(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        command = "curl -fsS http://api/check"
        (task / "workspace" / "output" / "evidence.json").write_text(
            f"{check_id}: observed failure\n"
        )
        (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": [
            {"id": check_id, "status": "fail", "command_id": "one",
             "evidence_record": "output/check-evidence.json", "evidence": ["output/evidence.json"]},
            {"id": "invented.check", "status": "fail", "command": command, "evidence": ["output/evidence.json"]},
        ]}))
        self.record_command(task, "one", command, exit_code=1)
        command_path = task / "commands" / "one.json"
        command_record = json.loads(command_path.read_text())
        command_record["output_artifacts"] = {"output/evidence.json": {
            "sha256": module.hashlib.sha256((task / "workspace/output/evidence.json").read_bytes()).hexdigest(),
            "size": (task / "workspace/output/evidence.json").stat().st_size,
        }}
        command_path.write_text(json.dumps(command_record))
        (task / "workspace/output/check-evidence.json").write_text(json.dumps({
            "check_id": check_id, "status": "fail", "command_id": "one",
            "expected": "orders::get removal is detected",
            "observed": "orders::get removal was not detected",
            "artifacts": ["output/evidence.json"],
        }))
        with patch.object(module, "feature_probe", return_value=(self.feature(failed={check_id}), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
        self.assertEqual((found["verification.outcome_accuracy"]["numerator"], found["verification.outcome_accuracy"]["denominator"]), (1, 24))
        self.assertEqual((found["verification.execution_coverage"]["numerator"], found["verification.execution_coverage"]["denominator"]), (1, 24))
        self.assertEqual((found["verification.evidence_coverage"]["numerator"], found["verification.evidence_coverage"]["denominator"]), (1, 24))

    def test_missing_subject_checks_earn_zero_outcome_accuracy(self):
        task, state = self.verification_task()
        with patch.object(module, "feature_probe", return_value=(self.feature(), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
        self.assertEqual((found["verification.outcome_accuracy"]["numerator"], found["verification.outcome_accuracy"]["denominator"]), (0, 24))
        self.assertEqual((found["verification.execution_coverage"]["numerator"], found["verification.execution_coverage"]["denominator"]), (0, 24))
        self.assertEqual((found["verification.evidence_coverage"]["numerator"], found["verification.evidence_coverage"]["denominator"]), (0, 24))

    def test_all_correctly_reported_passing_checks_have_full_outcome_accuracy(self):
        task, state = self.verification_task()
        command = "curl -fsS http://api/check"
        evidence = task / "workspace" / "output" / "evidence.json"
        evidence.write_text("all required checks observed pass\n" + "\n".join(module.PUBLIC_VERIFICATION_IDS))
        (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": [
            {"id": metric, "status": "pass", "command_id": "one", "evidence": ["output/evidence.json"]}
            for metric in module.PUBLIC_VERIFICATION_IDS
        ]}))
        self.record_command(task, "one", command)
        with patch.object(module, "feature_probe", return_value=(self.feature(), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
        self.assertEqual((found["verification.outcome_accuracy"]["numerator"], found["verification.outcome_accuracy"]["denominator"]), (24, 24))

    def test_incorrect_reported_outcome_does_not_earn_accuracy(self):
        task, state = self.verification_task()
        command = "curl -fsS http://api/check"
        (task / "workspace" / "output" / "evidence.json").write_text("implementation.function_removal observed\n")
        self.record_command(task, "one", command)
        case_id = "implementation.function_removal"
        for name, actual_failures, reported_status in (
            ("false positive", set(), "fail"),
            ("missed failure", {case_id}, "pass"),
        ):
            with self.subTest(name):
                self.write_check(task, {"id": case_id, "status": reported_status,
                                        "command_id": "one", "evidence": ["output/evidence.json"]})
                with patch.object(module, "feature_probe", return_value=(self.feature(failed=actual_failures), None)):
                    found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
                self.assertEqual(found["verification.outcome_accuracy"]["numerator"], 0)

    def test_blocked_independent_probe_keeps_truth_unavailable(self):
        task, state = self.verification_task()
        blocked = {"implementation.function_removal"}
        with patch.object(module, "feature_probe", return_value=(self.feature(unavailable=blocked), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
        self.assertEqual(found["verification.outcome_accuracy"]["status"], "unavailable")

    def test_command_id_and_outer_whitespace_legacy_commands_match_recorded_execution(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        command = "sh -lc 'printf \\\"a b\\\"'"
        evidence = task / "workspace" / "output" / "evidence.txt"
        evidence.write_text(f"{check_id}: pass\n")
        self.record_command(task, "stable-id", command)
        with patch.object(module, "feature_probe", return_value=(self.feature(), None)):
            for check in (
                {"id": check_id, "status": "pass", "command_id": "stable-id", "evidence": ["output/evidence.txt"]},
                {"id": check_id, "status": "pass", "command": command + "  \n", "evidence": ["output/evidence.txt"]},
            ):
                self.write_check(task, check)
                found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
                self.assertEqual(found["verification.execution_coverage"]["numerator"], 1)

    def test_legacy_command_matching_preserves_quoted_content(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        evidence = task / "workspace" / "output" / "evidence.txt"
        evidence.write_text(f"{check_id}: pass\n")
        self.record_command(task, "stable-id", "printf 'expected value'")
        self.write_check(task, {"id": check_id, "status": "pass",
                                "command": "printf 'altered value'",
                                "evidence": ["output/evidence.txt"]})
        with patch.object(module, "feature_probe", return_value=(self.feature(), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
        self.assertEqual(found["verification.execution_coverage"]["numerator"], 0)

    def test_empty_or_unrelated_evidence_does_not_receive_coverage(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        command = "printf unrelated > output/result.txt"
        self.record_command(task, "stable-id", command)
        for contents in ("", "unrelated successful command output\n"):
            with self.subTest(contents=contents):
                (task / "workspace" / "output" / "result.txt").write_text(contents)
                self.write_check(task, {"id": check_id, "status": "pass", "command_id": "stable-id",
                                        "evidence": ["output/result.txt"]})
                with patch.object(module, "feature_probe", return_value=(self.feature(), None)):
                    found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
                self.assertEqual(found["verification.evidence_coverage"]["numerator"], 0)

    def test_check_id_text_without_execution_provenance_does_not_receive_coverage(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        (task / "workspace" / "output" / "result.txt").write_text(f"{check_id}: pass\n")
        self.record_command(task, "stable-id", "printf no-artifact")
        self.write_check(task, {"id": check_id, "status": "pass", "command_id": "stable-id",
                                "evidence": ["output/result.txt"]})
        with patch.object(module, "feature_probe", return_value=(self.feature(), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
        self.assertEqual(found["verification.evidence_coverage"]["numerator"], 0)

    def test_structured_evidence_record_links_check_to_recorded_artifact_digest(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        artifact = task / "workspace" / "output" / "result.json"
        artifact.write_text('{"changes":[{"kind":"removed"}]}\n')
        self.record_command(task, "stable-id", "curl http://api/check > output/result.json")
        command_path = task / "commands" / "stable-id.json"
        command_record = json.loads(command_path.read_text())
        command_record["output_artifacts"] = {
            "output/result.json": {
                "sha256": module.hashlib.sha256(artifact.read_bytes()).hexdigest(),
                "size": artifact.stat().st_size,
            }
        }
        command_path.write_text(json.dumps(command_record))
        evidence_record = task / "workspace" / "output" / "function-removal-evidence.json"
        evidence_record.write_text(json.dumps({
            "check_id": check_id, "status": "pass", "command_id": "stable-id",
            "expected": "orders::get is reported as removed",
            "observed": "response contained a removed orders::get change",
            "artifacts": ["output/result.json"],
        }))
        self.write_check(task, {"id": check_id, "status": "pass", "command_id": "stable-id",
                                "evidence_record": "output/function-removal-evidence.json",
                                "evidence": ["output/result.json"]})
        with patch.object(module, "feature_probe", return_value=(self.feature(), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
        self.assertEqual(found["verification.evidence_coverage"]["numerator"], 1)

        artifact.write_text('{"changes":[]}\n')
        with patch.object(module, "feature_probe", return_value=(self.feature(), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
        self.assertEqual(found["verification.evidence_coverage"]["numerator"], 0)

    def structured_evidence_result(self, evidence_record):
        sequence = getattr(self, "structured_sequence", 0) + 1
        self.structured_sequence = sequence
        task = self.root / f"structured-{sequence}"
        (task / "workspace" / "output").mkdir(parents=True)
        (task / "validation").mkdir()
        (task / "commands").mkdir()
        (task / "validation" / "feature.json").write_text("{}")
        (task / "source.patch").write_bytes(b"")
        state = {"initial_patch_sha256": module.hashlib.sha256(b"").hexdigest()}
        check_id = "implementation.function_removal"
        artifact = task / "workspace" / "output" / "result.json"
        artifact.write_text('{"status":200}\n')
        self.record_command(task, "stable-id", "curl http://api/check > output/result.json")
        command_path = task / "commands" / "stable-id.json"
        command_record = json.loads(command_path.read_text())
        command_record["output_artifacts"] = {"output/result.json": {
            "sha256": module.hashlib.sha256(artifact.read_bytes()).hexdigest(),
            "size": artifact.stat().st_size,
        }}
        command_path.write_text(json.dumps(command_record))
        record_path = task / "workspace" / "output" / "evidence-record.json"
        if isinstance(evidence_record, bytes):
            record_path.write_bytes(evidence_record)
        else:
            record_path.write_text(json.dumps(evidence_record))
        self.write_check(task, {"id": check_id, "status": "pass", "command_id": "stable-id",
                                "evidence_record": "output/evidence-record.json",
                                "evidence": ["output/result.json"]})
        with patch.object(module, "feature_probe", return_value=(self.feature(), None)):
            return {item["id"]: item for item in module.verification_observations(
                task, ASSETS, state
            )}

    def test_non_object_or_invalid_text_evidence_record_receives_no_credit(self):
        for record in ([], None, "scalar", 42, b"\xff\xfe"):
            with self.subTest(record=record):
                found = self.structured_evidence_result(record)
                self.assertEqual(found["verification.evidence_coverage"]["numerator"], 0)

    def test_short_factual_evidence_text_is_accepted_but_blank_text_is_not(self):
        base = {
            "check_id": "implementation.function_removal", "status": "pass",
            "command_id": "stable-id", "expected": "200 OK", "observed": "HTTP200",
            "artifacts": ["output/result.json"],
        }
        found = self.structured_evidence_result(base)
        self.assertEqual(found["verification.evidence_coverage"]["numerator"], 1)
        for field in ("expected", "observed"):
            with self.subTest(field=field):
                invalid = {**base, field: "   "}
                found = self.structured_evidence_result(invalid)
                self.assertEqual(found["verification.evidence_coverage"]["numerator"], 0)

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
            if "config --format json" in command:
                stdout = json.dumps({"services": {"web": {"ports": [
                    {"published": "62000", "target": 3000},
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
        with patch.object(module, "controller_command", side_effect=execute), \
             patch.object(module, "allocate_validator_ports", side_effect=[(62000, 62001), (62002, 62003)]):
            module.environment_observations(task, ASSETS, state)
        self.assertTrue(any("docker compose -f /workspace/registry/compose.yaml restart" in command for command in commands))
        self.assertTrue(any("--filter label=com.docker.compose.project=validator-123456789012" in command for command in commands))
        self.assertTrue(any("WEB_PORT=62002 API_PORT=62003" in command for command in commands))
        self.assertTrue(all("WEB_PORT=65000 API_PORT=65001" not in command for command in commands))
        browser = next(command for command in commands if "environment-web" not in command and "E2E_APP_URL=" in command)
        self.assertIn("compose -f /workspace/registry/compose.yaml exec -T", browser)
        self.assertIn("E2E_APP_URL=http://127.0.0.1:3000", browser)
        self.assertNotIn("docker run", browser)

    def test_validator_port_allocation_rejects_subject_ports(self):
        state = {"container": "outer", "web_port": 65000, "api_port": 65001}
        allocated = {"exit_code": 0, "stdout": "[65000, 62001]\n", "stderr": ""}
        with patch.object(module, "controller_command", return_value=allocated):
            with self.assertRaises(ValueError):
                module.allocate_validator_ports(state)

    def test_failed_validator_preparation_leaves_dependent_observations_unavailable(self):
        task = self.root / "test-3-preparation"
        (task / "workspace" / "output").mkdir(parents=True)
        (task / "state.json").write_text("{}")
        (task / "workspace" / "output" / "environment.json").write_text(json.dumps({
            "compose_file": "registry/compose.yaml", "startup_command": "docker compose up -d",
            "teardown_command": "docker compose down -v", "migration_command": "docker compose run migrate",
            "db_service": "db", "db_user": "fixture", "db_name": "registry_fixture",
        }))
        def execute(_state, command, timeout=120):
            if "docker compose up -d" in command:
                return {"exit_code": 1, "stdout": "", "stderr": "address already in use"}
            return {"exit_code": 0, "stdout": "", "stderr": ""}
        with patch.object(module, "controller_command", side_effect=execute), \
             patch.object(module, "allocate_validator_ports", side_effect=[(62000, 62001), (62002, 62003)]):
            found = {item["id"]: item for item in module.environment_observations(
                task, ASSETS, {"container": "outer", "web_port": 65000, "api_port": 65001}
            )}
        for metric in ("environment.migration", "environment.api_readiness",
                       "environment.frontend_reachability", "environment.runtime_identity"):
            self.assertEqual(found[metric]["status"], "unavailable")
            self.assertEqual(found[metric]["reason"], "validator_preparation_failed")

    def replay_failure_observations(self, failure):
        task = self.root / f"test-3-replay-{failure}"
        (task / "workspace" / "output").mkdir(parents=True)
        (task / "state.json").write_text("{}")
        (task / "workspace" / "output" / "environment.json").write_text(json.dumps({
            "compose_file": "registry/compose.yaml", "startup_command": "docker compose up -d",
            "teardown_command": "docker compose down -v", "migration_command": "docker compose run migrate",
            "db_service": "db", "db_user": "fixture", "db_name": "registry_fixture",
        }))

        def execute(_state, command, timeout=120):
            replay = "validator-123456789012-replay" in command
            if replay and "docker compose up -d" in command and failure == "startup":
                return {"exit_code": 1, "stdout": "", "stderr": "port is already allocated"}
            if replay and "ps -q api" in command and failure in ("startup", "identity"):
                return {"exit_code": 1, "stdout": "", "stderr": "instance identity unavailable"}
            if replay and "to_regclass" in command:
                return {"exit_code": 1, "stdout": "", "stderr": "service db is not running"}
            if "127.0.0.1:62003/health" in command and failure in ("startup", "identity"):
                return {"exit_code": 1, "stdout": "", "stderr": "connection refused"}
            stdout = ""
            if "config --format json" in command:
                stdout = json.dumps({"services": {"web": {"ports": [
                    {"published": "62000", "target": 3000},
                ]}}})
            elif "select version" in command:
                stdout = "0.9.0\n1.0.0\n1.1.0\n2.0.0\n"
            elif "select (select count" in command or "to_regclass" in command:
                stdout = "t\n"
            elif "validator_restart_sentinel" in command and "select count" in command:
                stdout = "1\n"
            elif "rev-parse HEAD" in command:
                stdout = module.REGISTRY_SHA + "\n"
            elif "--filter label=com.docker.compose.project=validator-123456789012" in command and not replay:
                stdout = "container\nvolume\nnetwork\n"
            return {"exit_code": 0, "stdout": stdout, "stderr": ""}

        with patch.object(module, "controller_command", side_effect=execute), \
             patch.object(module, "allocate_validator_ports", side_effect=[(62000, 62001), (62002, 62003)]):
            return {item["id"]: item for item in module.environment_observations(
                task, ASSETS, {"container": "outer-123456789012", "web_port": 65000, "api_port": 65001}
            )}

    def test_replay_startup_failure_keeps_two_instance_checks_unavailable(self):
        found = self.replay_failure_observations("startup")
        for metric in ("environment.clean_reproduction", "environment.parallel_isolation",
                       "environment.cleanup_scope"):
            self.assertEqual(found[metric]["status"], "unavailable")
            self.assertEqual(found[metric]["reason"], "validator_replay_preparation_failed")
        self.assertIn(found["environment.cleanup_completeness"]["status"], ("measured", "unavailable"))

    def test_replay_identity_failure_keeps_two_instance_checks_unavailable(self):
        found = self.replay_failure_observations("identity")
        for metric in ("environment.clean_reproduction", "environment.parallel_isolation",
                       "environment.cleanup_scope"):
            self.assertEqual(found[metric]["status"], "unavailable")
            self.assertEqual(found[metric]["reason"], "validator_replay_preparation_failed")


if __name__ == "__main__":
    unittest.main()
