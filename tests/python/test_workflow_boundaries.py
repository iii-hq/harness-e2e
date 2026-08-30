import json
import re
import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


class WorkflowBoundaryTests(unittest.TestCase):
    def test_external_actions_are_pinned_to_immutable_commits(self):
        action_ref = re.compile(r"^\s*uses:\s*([^\s#]+)", re.MULTILINE)
        immutable = re.compile(r"^[^\s]+@[0-9a-f]{40}$")
        for path in (ROOT / ".github/workflows").glob("*.yml"):
            for action in action_ref.findall(path.read_text(encoding="utf-8")):
                self.assertRegex(action, immutable, f"{path.name}: {action}")

    def test_migrated_operational_paths_are_compose_only_and_unversioned(self):
        paths = [
            ROOT / "scripts/exact_stack_campaign.py",
            ROOT / "scripts/run_exact_stack_group.sh",
            ROOT / "scripts/run_exact_stack_fault.sh",
            ROOT / "supervisor/run-weekly-stress",
            ROOT / "supervisor/install.sh",
            ROOT / ".github/workflows/exact-stack-e2e.yml",
            ROOT / "src/worker.rs",
            ROOT / "src/main.rs",
        ]
        forbidden = [
            "iii " + "worker",
            "iii-" + "worker",
            "iii." + "lock",
            "HARNESS_E2E_" + "DATA_DIR",
            "contract_" + "schema_version",
            "schema" + "_version",
        ]
        for path in paths:
            content = path.read_text()
            for token in forbidden:
                self.assertNotIn(token, content, f"{path.relative_to(ROOT)} contains {token}")

    def test_exact_stack_campaign_execution_is_owned_here(self):
        workflow = (
            ROOT / ".github/workflows/exact-stack-e2e.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("strategy:\n      fail-fast: false", workflow)
        self.assertIn("scripts/run_exact_stack_group.sh", workflow)
        self.assertIn("scripts/run_exact_stack_fault.sh", workflow)
        self.assertIn("scripts/exact_stack_campaign.py", workflow)
        self.assertIn("runs-on: ${{ matrix.runs_on }}", workflow)
        self.assertIn("environment: harness-e2e-trusted", workflow)
        self.assertIn("ref: ${{ inputs.runner_sha }}", workflow)
        self.assertNotIn("matrix.requires_", workflow)
        launcher = (ROOT / "scripts/run_exact_stack_group.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn("engineering-ticket.bundle", launcher)
        self.assertIn("shared-fixture.bundle", launcher)
        self.assertIn("HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH", launcher)
        self.assertIn("HARNESS_E2E_FIXTURE_PATH", launcher)
        self.assertIn("cleanup --lease-id", launcher)
        self.assertNotIn("iii-hq/workers", workflow)

    def test_exact_stack_is_the_only_release_control_executor(self):
        workflows = {path.name for path in (ROOT / ".github/workflows").glob("*.yml")}
        self.assertIn("exact-stack-e2e.yml", workflows)
        self.assertNotIn("shadow.yml", workflows)
        self.assertNotIn("release.yml", workflows)

    def test_weekly_stress_delegates_privileged_actions_to_protected_launchers(self):
        self.assertIn("e2e::archive", (ROOT / "docs/fault-injection.md").read_text())
        self.assertFalse(
            (ROOT / "config/profiles/weekly-l5-recovery.json").exists()
        )
        self.assertFalse(
            (ROOT / "config/profiles/weekly-l5-cancellation.json").exists()
        )
        supervisor = (ROOT / "supervisor/run-weekly-stress").read_text()
        installer = (ROOT / "supervisor/install.sh").read_text()
        for operation in ("validate", "up", "status", "down"):
            self.assertIn(f"compose::{operation}", supervisor)
        self.assertIn("III_COMPOSE_STATE_DIR", supervisor)
        self.assertIn("--namespace \"$project_namespace\"", supervisor)
        self.assertIn("0.23.0-rc.4", installer)
        self.assertIn("d9ab056f17daefc2f04ed892092a3df2fe76ffde5587335918606048047cf40a", installer)
        self.assertNotIn("iii " + "worker", supervisor + installer)
        self.assertNotIn("iii-" + "worker", supervisor + installer)

    def test_release_control_is_the_only_operational_campaign_dispatch(self):
        for name in (
            "daily.yml",
            "post-deploy.yml",
            "weekly.yml",
            "weekly-stress.yml",
            "run-campaign.yml",
        ):
            self.assertFalse((ROOT / ".github/workflows" / name).exists())
        workflow = (ROOT / ".github/workflows/exact-stack-e2e.yml").read_text()
        self.assertIn("exact-stack", workflow)
        contract_tool = (ROOT / "scripts/exact_stack_campaign.py").read_text()
        self.assertIn('definition.get("failurePolicy") != "advisory"', contract_tool)

        expected_adaptive = {"post-deploy.json": 1, "weekly.json": 2}
        for name, expected_count in expected_adaptive.items():
            manifest = json.loads(
                (ROOT / "config/campaigns" / name).read_text(encoding="utf-8")
            )
            adaptive = [
                group
                for group in manifest["groups"]
                if group["execution_kind"] == "adaptive_flow"
            ]
            self.assertEqual(len(adaptive), expected_count)
            self.assertTrue(all(group["runs"] == 1 for group in adaptive))
            self.assertTrue(all(group["technical_retries"] == 0 for group in adaptive))

    def test_canonical_gate_pins_and_authorizes_the_e2e_revision(self):
        workflow = (ROOT / ".github/workflows/canonical-gate.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("repository: iii-hq/harness-e2e", workflow)
        self.assertIn("ref: ${{ inputs.e2e_revision }}", workflow)
        self.assertIn("compare/$E2E_REVISION...$default_sha", workflow)
        self.assertIn("/opt/iii-harness-e2e/resolve-cutover-evidence", workflow)

    def test_compose_campaigns_use_disposable_code_fixtures(self):
        launcher = (ROOT / "scripts/run_exact_stack_group.sh").read_text()
        self.assertIn("HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH", launcher)
        self.assertIn("HARNESS_E2E_FIXTURE_PATH", launcher)
        self.assertIn("engineering_fixture_revision=", launcher)
        self.assertIn("shared_fixture_revision=", launcher)
        self.assertIn("prepare --execution-id", launcher)
        self.assertIn("cleanup --lease-id", launcher)
        self.assertNotIn("git commit", launcher)

    def test_endurance_keeps_github_authority_in_the_post_run_publisher(self):
        workflow = (ROOT / ".github/workflows/engineering-endurance.yml").read_text(
            encoding="utf-8"
        )
        scenario = (
            ROOT / "src/scenarios/engineering_endurance_ladder.rs"
        ).read_text(encoding="utf-8")
        publisher = (
            ROOT / "scripts/publish_engineering_endurance.py"
        ).read_text(encoding="utf-8")
        self.assertIn("timeout-minutes: 240", workflow)
        self.assertIn("E2E_FIXTURE_GITHUB_TOKEN", workflow)
        self.assertIn("Publish sanitized GitHub handoff", workflow)
        self.assertIn('"github::*"', scenario)
        self.assertIn('ALLOWED_REPOSITORY = "iii-hq/e2e-fixture"', publisher)
        self.assertNotIn("E2E_FIXTURE_GITHUB_TOKEN", scenario)
        self.assertNotIn("hidden_output", publisher.split("def public_projection", 1)[1].split("def create_blob", 1)[0])


if __name__ == "__main__":
    unittest.main()
