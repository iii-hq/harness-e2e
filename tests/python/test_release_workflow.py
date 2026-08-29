import json
import pathlib
import re
import tempfile
import unittest

import yaml

from scripts.release_compiler import read_pin


ROOT = pathlib.Path(__file__).parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
INDEX_WORKFLOW = ROOT / ".github" / "workflows" / "release-descriptor-index.yml"


def workflow():
    return yaml.load(WORKFLOW.read_text(encoding="utf-8"), Loader=yaml.BaseLoader)


class ReleaseWorkflowTests(unittest.TestCase):
    def test_release_control_is_the_only_workflow_entrypoint(self):
        parsed = workflow()
        self.assertEqual(set(parsed["on"]), {"workflow_dispatch"})
        inputs = parsed["on"]["workflow_dispatch"]["inputs"]
        self.assertLessEqual(len(inputs), 10)
        self.assertTrue(
            {
                "identity",
                "source_sha",
                "descriptor_sha256",
                "descriptor_run_id",
                "descriptor_artifact",
            }
            <= set(inputs)
        )
        self.assertFalse(
            (ROOT / ".github" / "workflows" / "promote-latest.yml").exists()
        )

    def test_actions_are_immutable_and_mac_builds_use_the_warm_pool(self):
        text = "\n".join(
            path.read_text(encoding="utf-8")
            for path in (WORKFLOW, INDEX_WORKFLOW)
        )
        references = re.findall(
            r"^\s*-?\s*uses:\s*([^\s#]+)", text, flags=re.MULTILINE
        )
        self.assertTrue(references)
        for reference in references:
            if reference.startswith("./"):
                continue
            self.assertRegex(reference, r"^[^@]+@[0-9a-f]{40}$")

        matrix = workflow()["jobs"]["binaries"]["strategy"]["matrix"]["include"]
        self.assertEqual(len(matrix), 9)
        mac = [entry for entry in matrix if entry["target"].endswith("apple-darwin")]
        self.assertEqual(len(mac), 2)
        self.assertTrue(
            all("workers-release-macos-12core" in entry["os"] for entry in mac)
        )

    def test_result_and_registry_publication_use_current_registry_contract(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        self.assertNotIn("iii.worker.yaml", text)
        self.assertIn("validate-descriptor", text)
        self.assertIn("frontend-metadata", text)
        self.assertIn("build-frontends", text)
        self.assertNotIn("pnpm --dir dashboard", text)
        self.assertIn("--phase harness_release", text)
        self.assertIn(
            "release-result-${{ fromJSON(inputs.identity).candidate_id }}-${{ fromJSON(inputs.identity).step_id }}-attempt-${{ github.run_attempt }}",
            text,
        )
        self.assertIn(
            "--descriptor release-descriptor-input/release-descriptor.json", text
        )
        self.assertIn("scripts/publish_registry.py", text)
        self.assertNotIn("package_descriptor", text)
        self.assertEqual(workflow()["jobs"]["report"]["if"], "${{ always() }}")

    def test_descriptor_index_is_compiled_once_by_pinned_workers_compiler(self):
        text = INDEX_WORKFLOW.read_text(encoding="utf-8")
        self.assertNotIn("iii compose descriptor-index", text)
        self.assertIn('python3 "$compiler" compile-index', text)
        self.assertIn("--compiler-commit \"$COMPILER_COMMIT\"", text)
        self.assertIn("--release-spec .release/workers.yaml", text)
        self.assertIn(
            "release-descriptor-index-${{ steps.source.outputs.sha }}", text
        )

    def test_compiler_pin_is_full_and_fails_closed(self):
        pin = read_pin(ROOT / ".github" / "release-compiler.json")
        self.assertEqual(pin["repository"], "iii-hq/workers")
        self.assertRegex(pin["commit"], r"^[0-9a-f]{40}$")
        self.assertRegex(pin["digest"], r"^[0-9a-f]{64}$")

        with tempfile.TemporaryDirectory() as directory:
            invalid = pathlib.Path(directory) / "pin.json"
            invalid.write_text(
                json.dumps({**pin, "commit": "0" * 40}), encoding="utf-8"
            )
            with self.assertRaises(SystemExit):
                read_pin(invalid)
