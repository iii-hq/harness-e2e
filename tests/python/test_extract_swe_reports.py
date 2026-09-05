import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/extract_swe_reports.py"


class ExtractSweReportsTests(unittest.TestCase):
    def test_preserves_native_bytes_and_ignores_other_deliverables(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            native = root / "native/deliverables/attempt"
            native.mkdir(parents=True)
            content = b'{ "schema": "swe-service-report/v1", "mode": "journey" }\n'
            (native / "swe_service_report.json").write_bytes(content)
            (native / "unrelated.json").write_text('{"secret":"not a SWE report"}')
            result = subprocess.run([sys.executable, str(SCRIPT), "--native-dir", str(root / "native"), "--output-dir", str(root / "out")], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / "out/attempt/swe_service_report.json").read_bytes(), content)
            self.assertFalse((root / "out/attempt/unrelated.json").exists())
            self.assertEqual(json.loads(result.stdout)["reports"], 1)

    def test_missing_swe_reports_is_a_successful_noop(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = subprocess.run([sys.executable, str(SCRIPT), "--native-dir", str(root), "--output-dir", str(root / "out")], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)["reports"], 0)

    def test_refuses_a_report_reached_through_a_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outside = root / "private"
            outside.mkdir()
            (outside / "swe_service_report.json").write_text("private")
            (root / "native/deliverables").mkdir(parents=True)
            (root / "native/deliverables/attempt").symlink_to(outside, target_is_directory=True)
            result = subprocess.run([sys.executable, str(SCRIPT), "--native-dir", str(root / "native"), "--output-dir", str(root / "out")], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((root / "out/attempt/swe_service_report.json").exists())
