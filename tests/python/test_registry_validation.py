import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ASSETS = Path(__file__).resolve().parents[2] / "repository-tasks/registry-version-comparison"
spec = importlib.util.spec_from_file_location("registry_validate", ASSETS / "validate.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


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

    def feature(self, failed=(), unavailable=()):
        return {"observations": [
            {"id": metric, "status": "unavailable", "reason": "blocked"}
            if metric in unavailable else
            {"id": metric, "status": "measured", "value": 0 if metric in failed else 1}
            for metric in module.PUBLIC_VERIFICATION_IDS
        ], "cases": {}}

    def test_verification_uses_probe_observations_and_bounded_ids(self):
        task, state = self.verification_task()
        check_id = "implementation.function_removal"
        command = "curl -fsS http://api/check"
        (task / "workspace" / "output" / "evidence.json").write_text("{}")
        (task / "workspace" / "output" / "checks.json").write_text(json.dumps({"checks": [
            {"id": check_id, "status": "fail", "command": command, "evidence": ["output/evidence.json"]},
            {"id": "invented.check", "status": "fail", "command": command, "evidence": ["output/evidence.json"]},
        ]}))
        (task / "commands" / "one.json").write_text(json.dumps({"command": command, "exit_code": 0}))
        with patch.object(module, "feature_probe", return_value=(self.feature(failed={check_id}), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
        self.assertEqual((found["verification.recall"]["numerator"], found["verification.recall"]["denominator"]), (1, 1))
        self.assertEqual((found["verification.precision"]["numerator"], found["verification.precision"]["denominator"]), (1, 2))
        self.assertEqual((found["verification.execution_coverage"]["numerator"], found["verification.execution_coverage"]["denominator"]), (1, 24))
        self.assertEqual(found["verification.evidence_coverage"]["denominator"], 2)

    def test_missing_subject_checks_is_zero_where_objectively_measurable(self):
        task, state = self.verification_task()
        failed = {"implementation.function_removal"}
        with patch.object(module, "feature_probe", return_value=(self.feature(failed=failed), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
        self.assertEqual((found["verification.recall"]["numerator"], found["verification.recall"]["denominator"]), (0, 1))
        self.assertEqual((found["verification.execution_coverage"]["numerator"], found["verification.execution_coverage"]["denominator"]), (0, 24))
        self.assertEqual(found["verification.precision"]["status"], "not_applicable")

    def test_blocked_independent_probe_keeps_truth_unavailable(self):
        task, state = self.verification_task()
        blocked = {"implementation.function_removal"}
        with patch.object(module, "feature_probe", return_value=(self.feature(unavailable=blocked), None)):
            found = {item["id"]: item for item in module.verification_observations(task, ASSETS, state)}
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
            if "select version" in command:
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
        self.assertTrue(any("WEB_PORT=41000 API_PORT=41001" in command for command in commands))


if __name__ == "__main__":
    unittest.main()
