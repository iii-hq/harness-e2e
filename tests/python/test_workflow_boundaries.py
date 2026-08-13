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


if __name__ == "__main__":
    unittest.main()
