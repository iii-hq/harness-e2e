import json
import pathlib
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
FIXTURE = ROOT / 'tests/fixtures/results/diagnostics-real-contract-discovery.json'


class RealDiscoveryEvidenceTests(unittest.TestCase):
    def test_live_green_runs_expose_the_baseline_repetition_and_clear_after_the_harness_fix(self):
        fixture = json.loads(FIXTURE.read_text())
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for variant, expected in (('baseline', 2), ('candidate', 0)):
                with self.subTest(variant=variant):
                    artifacts, output = root / variant, root / (variant + '-report')
                    artifacts.mkdir()
                    source = json.dumps(fixture[variant]['report'])
                    (artifacts / 'results.json').write_text(source)
                    completed = subprocess.run([
                        sys.executable, str(ROOT / 'scripts/render_e2e_report.py'),
                        '--artifacts', str(artifacts), '--output', str(output),
                    ], capture_output=True, text=True)
                    self.assertEqual(completed.returncode, 0, completed.stderr)
                    self.assertEqual((artifacts / 'results.json').read_text(), source)
                    diagnostics = json.loads((output / 'diagnostics.json').read_text())
                    self.assertEqual(diagnostics['runs_collected'], 2)
                    self.assertEqual(diagnostics['affected_runs'], expected)
                    self.assertEqual(diagnostics['occurrences'], expected)
                    self.assertEqual(diagnostics['identity']['subject']['model'], 'deepseek-flash')
                    summary = (output / 'summary.md').read_text()
                    self.assertIn(f'Runs requiring attention: {expected}', summary)
                    self.assertEqual(len(list((output / 'failures').glob('*.md'))), expected)
                    for scenario in fixture[variant]['report']['scenarios']:
                        for run in scenario['runs']:
                            self.assertEqual((run['technical'], run['completion'], run['score']),
                                             ('valid', 'completed', 100))
                    if expected:
                        self.assertEqual(len(diagnostics['cohorts']), 1)
                        self.assertEqual(diagnostics['cohorts'][0]['affected_runs'], 2)
                        for run in diagnostics['runs']:
                            occurrence = run['attempts'][0]['occurrences'][0]
                            self.assertEqual(occurrence['rule_id'], 'repeated_contract_discovery')
                            self.assertEqual(occurrence['cause'], 'harness_notice_correlated')
                            for function in occurrence['functions']:
                                self.assertLess(function['source_result']['transcript_index'],
                                                function['registry_changed_notice']['transcript_index'])
                                self.assertLess(function['registry_changed_notice']['transcript_index'],
                                                occurrence['repeated_call']['transcript_index'])
                    else:
                        self.assertEqual(diagnostics['cohorts'], [])
                        self.assertEqual(diagnostics['runs'], [])


if __name__ == '__main__':
    unittest.main()
