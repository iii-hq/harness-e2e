import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/extract_kanban_reports.py"


class ExtractKanbanReportsTests(unittest.TestCase):
    def test_missing_reports_is_a_successful_noop(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = subprocess.run([sys.executable, str(SCRIPT), "--native-dir", str(root), "--output-dir", str(root / "out")], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)["reports"], 0)

    def test_preserves_allowlisted_kanban_evaluation_and_diagnostic(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            delivery = root / "native/deliverables/run-1/attempt-1"
            diagnostic = root / "native/evidence/run-1/attempt-1"
            delivery.mkdir(parents=True)
            diagnostic.mkdir(parents=True)
            (delivery / "kanban_evaluation.json").write_bytes(b'{"status":"passed"}\n')
            (delivery / "unrelated.json").write_text('{"secret":"not a Kanban report"}')
            (diagnostic / "kanban-controller.json").write_bytes(b'{"fatal":false}\n')
            result = subprocess.run([sys.executable, str(SCRIPT), "--native-dir", str(root / "native"), "--output-dir", str(root / "out")], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / "out/kanban/deliverables/run-1/attempt-1/kanban_evaluation.json").read_bytes(), b'{"status":"passed"}\n')
            self.assertEqual((root / "out/kanban/evidence/run-1/attempt-1/kanban-controller.json").read_bytes(), b'{"fatal":false}\n')
            self.assertFalse((root / "out/kanban/deliverables/run-1/attempt-1/unrelated.json").exists())
            self.assertEqual(json.loads(result.stdout)["reports"], 2)

    def test_refuses_kanban_evidence_reached_through_a_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outside = root / "private"
            outside.mkdir()
            (outside / "kanban_evaluation.json").write_text("private")
            (root / "native/deliverables/run").mkdir(parents=True)
            (root / "native/deliverables/run/attempt").symlink_to(outside, target_is_directory=True)
            result = subprocess.run([sys.executable, str(SCRIPT), "--native-dir", str(root / "native"), "--output-dir", str(root / "out")], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((root / "out/kanban").exists())

    def test_refuses_an_output_symlink_even_when_its_bytes_match(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "native/deliverables/run/attempt/kanban_evaluation.json"
            source.parent.mkdir(parents=True)
            source.write_text('{"status":"passed"}')
            target = root / "out/kanban/deliverables/run/attempt/kanban_evaluation.json"
            target.parent.mkdir(parents=True)
            target.symlink_to(source)
            result = subprocess.run([sys.executable, str(SCRIPT), "--native-dir", str(root / "native"), "--output-dir", str(root / "out")], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
