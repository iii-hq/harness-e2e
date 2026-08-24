import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


class WorkflowBoundaryTests(unittest.TestCase):
    def test_cross_repository_shadow_checks_out_the_e2e_repository(self):
        workflow = (ROOT / ".github/workflows/shadow.yml").read_text(encoding="utf-8")
        self.assertEqual(workflow.count("repository: iii-hq/harness-e2e"), 2)
        self.assertNotIn("sudo ", workflow)

    def test_weekly_stress_delegates_privileged_actions_to_protected_launchers(self):
        workflow = (ROOT / ".github/workflows/weekly-stress.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("environment: harness-e2e-trusted", workflow)
        self.assertIn("/opt/iii-harness-e2e/run-weekly-stress", workflow)
        self.assertIn("e2e::archive", (ROOT / "docs/fault-injection.md").read_text())
        self.assertNotIn("sudo ", workflow)
        self.assertNotIn("docker ", workflow)

    def test_canonical_gate_pins_and_authorizes_the_e2e_revision(self):
        workflow = (ROOT / ".github/workflows/canonical-gate.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("repository: iii-hq/harness-e2e", workflow)
        self.assertIn("ref: ${{ inputs.e2e_revision }}", workflow)
        self.assertIn("compare/$E2E_REVISION...$default_sha", workflow)
        self.assertIn("/opt/iii-harness-e2e/resolve-cutover-evidence", workflow)

    def test_daily_campaign_uses_a_protected_disposable_engineering_fixture(self):
        workflow = (ROOT / ".github/workflows/run-campaign.yml").read_text(
            encoding="utf-8"
        )
        launcher = "/opt/iii-harness-e2e/engineering-ticket-fixture"
        self.assertGreaterEqual(workflow.count(launcher), 3)
        self.assertIn("HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH", workflow)
        self.assertIn("prepare", workflow)
        self.assertIn("cleanup --lease-id", workflow)
        self.assertNotIn("git commit", workflow)

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
