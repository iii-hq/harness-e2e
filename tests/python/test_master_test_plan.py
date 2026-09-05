import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import run_test_plan
from run_e2e_campaign import CampaignError, _canonical_sha256, parse_campaign, execute_campaign
from exact_stack_campaign import campaign_manifest, validate_suite

DEFINITION = json.loads((ROOT / 'config/test-plan.json').read_text())
CATALOG = json.loads((ROOT / 'config/test-plan-profiles.json').read_text())


def snapshot(profile_id):
    exported = next(p for p in CATALOG['profiles'] if p['id'] == profile_id)
    profile = next(p for p in DEFINITION['profiles'] if p['id'] == profile_id)
    return {
        'schema': 'harness-e2e-profile-snapshot/v1', 'plan_id': 'harness',
        'definition_sha256': CATALOG['definition_sha256'], 'profile': profile,
        'profile_sha256': exported['profile_sha256'], 'campaigns': exported['campaigns'],
        'cases': [{'judge_required': False}], 'budget': exported['budget'],
        'protected_supervisor_required': exported['protected_supervisor_required'],
    }


class MasterTestPlanTest(unittest.TestCase):
    def test_generated_compatibility_campaigns_preserve_existing_semantics(self):
        for ident, expected in DEFINITION['compatibility_campaigns'].items():
            actual = json.loads((ROOT / 'config/campaigns' / f'{ident}.json').read_text())
            self.assertEqual(actual, expected)
            parse_campaign(actual)

    def test_every_profile_round_is_admitted_by_the_campaign_executor(self):
        expected_slots = {'smoke': 5, 'regression': 12, 'capability': 47, 'evolution': 90, 'resilience': 13, 'endurance': 5}
        for profile in CATALOG['profiles']:
            slots = 0
            for raw in profile['campaigns']:
                campaign = parse_campaign(raw)
                self.assertEqual(campaign.failure_policy, 'advisory')
                slots += sum(g.runs * (len(g.scenarios) or 1) for g in campaign.groups)
                for group in campaign.groups:
                    if group.execution_kind != 'harness_turn':
                        self.assertEqual(group.technical_retries, 0)
                    if group.execution_kind == 'adaptive_flow':
                        self.assertEqual(group.runs, 1)
                        self.assertEqual(len(group.scenarios), 1)
            self.assertEqual(slots, expected_slots[profile['id']])

    def test_tampering_is_rejected_even_when_attacker_rehashes_the_manifest(self):
        altered = copy.deepcopy(snapshot('regression')['campaigns'][0])
        altered['groups'][0]['technical_retries'] = 0
        altered['test_plan']['campaign_sha256'] = _canonical_sha256({k: v for k, v in altered.items() if k != 'test_plan'})
        with self.assertRaisesRegex(CampaignError, 'reviewed profile composition'):
            parse_campaign(altered)
        altered = copy.deepcopy(snapshot('smoke')['campaigns'][0])
        altered['test_plan']['definition_sha256'] = 'sha256:' + 'a' * 64
        with self.assertRaisesRegex(CampaignError, 'pinned runner catalog'):
            parse_campaign(altered)

    def test_missing_repetitions_and_wrong_source_are_rejected(self):
        missing = copy.deepcopy(snapshot('evolution'))
        missing['campaigns'].pop()
        with self.assertRaisesRegex(CampaignError, 'missing campaign repetitions'):
            run_test_plan.validate_snapshot(missing, DEFINITION)
        changed = copy.deepcopy(DEFINITION)
        changed['version'] += 1
        with self.assertRaisesRegex(CampaignError, 'definition differ'):
            run_test_plan.validate_snapshot(snapshot('smoke'), changed)

    def test_release_control_roundtrip_keeps_canonical_seeds_and_plan_identity(self):
        identity = {'provider': 'test', 'model': 'test'}
        for profile in CATALOG['profiles']:
            for raw in profile['campaigns']:
                suite = run_test_plan.release_control_suite(raw, profile, identity, identity)
                self.assertIsNone(suite['seed'])
                self.assertEqual(campaign_manifest({'suite': suite}), raw)
                self.assertEqual(validate_suite(suite), suite)
                suite['seed'] = 4404
                with self.assertRaisesRegex(ValueError, 'scenario-owned'):
                    validate_suite(suite)

    def test_export_is_immutable_and_does_not_start_a_model(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / 'snapshot'
            with mock.patch('run_test_plan.subprocess.run', side_effect=AssertionError('must not execute')):
                paths = run_test_plan.write_export(root, snapshot('evolution'), DEFINITION, None, None)
            self.assertEqual(len(paths), 5)
            for path in paths:
                parse_campaign(json.loads(path.read_text()))
            before = (root / 'profile.json').read_bytes()
            with self.assertRaises(FileExistsError):
                run_test_plan.write_export(root, snapshot('smoke'), DEFINITION, None, None)
            self.assertEqual((root / 'profile.json').read_bytes(), before)

    def test_protected_fault_profile_is_rejected_before_any_execution_or_write(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / 'output'
            with mock.patch('run_test_plan.native_json', side_effect=[snapshot('resilience'), DEFINITION]), mock.patch('run_test_plan.subprocess.run', side_effect=AssertionError('must not execute')):
                code = run_test_plan.main(['--profile', 'resilience', '--e2e-bin', sys.executable, '--execute', '--output-root', str(output)])
            self.assertEqual(code, 2)
            self.assertFalse(output.exists())

    def test_direct_campaign_execution_cannot_override_profile_seeds(self):
        campaign = parse_campaign(snapshot('smoke')['campaigns'][0])
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / 'output'
            with self.assertRaisesRegex(CampaignError, 'scenario-owned seeds'):
                execute_campaign(campaign, e2e_bin=Path(sys.executable), output_root=output,
                    execution_id='seed-test', dry_run=True, advisory=True,
                    environ={'HARNESS_E2E_SEED': '123'})
            self.assertFalse(output.exists())

    def test_malformed_dashboard_export_is_rejected_before_materialization(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'profile.json'
            for invalid in [None, [], {'schema': 'harness-e2e-profile-campaigns/v1', 'profile': []}]:
                path.write_text(json.dumps(invalid))
                with mock.patch('run_test_plan.native_json', side_effect=AssertionError('must not materialize')):
                    self.assertEqual(run_test_plan.main(['--import-profile', str(path), '--e2e-bin', sys.executable]), 2)

    def test_execution_retains_every_round_and_marks_missing_results_as_coverage_gap(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            def execute(command):
                summary = Path(command[command.index('--summary') + 1])
                summary.parent.mkdir(parents=True)
                summary.write_text(json.dumps({'objective_passed': False, 'groups': [
                    {'group_id': 'missing', 'output': str(summary.parent / 'missing')},
                ]}))
                return mock.Mock(returncode=0)
            with mock.patch.dict('os.environ', {}, clear=True), mock.patch('run_test_plan.native_json', side_effect=[snapshot('evolution'), DEFINITION]), mock.patch('run_test_plan.subprocess.run', side_effect=execute) as runner:
                code = run_test_plan.main(['--profile', 'evolution', '--e2e-bin', sys.executable,
                    '--execute', '--output-root', str(output), '--execution-id', 'sample',
                    '--model', 'test', '--provider', 'test'])
            self.assertEqual(code, 0)
            self.assertEqual(runner.call_count, 5)
            receipt = json.loads((output / 'evolution/sample/execution.json').read_text())
            self.assertEqual(receipt['state'], 'complete')
            self.assertFalse(receipt['objective_passed'])
            self.assertEqual(receipt['results_artifacts'], 0)
            self.assertEqual(len(receipt['missing_results']), 5)

    def test_interrupted_execution_keeps_partial_receipt_and_never_starts_next_round(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            with mock.patch.dict('os.environ', {}, clear=True), mock.patch('run_test_plan.native_json', side_effect=[snapshot('evolution'), DEFINITION]), mock.patch('run_test_plan.subprocess.run', return_value=mock.Mock(returncode=1)) as runner:
                code = run_test_plan.main(['--profile', 'evolution', '--e2e-bin', sys.executable,
                    '--execute', '--output-root', str(output), '--execution-id', 'sample',
                    '--model', 'test', '--provider', 'test'])
            self.assertEqual(code, 2)
            self.assertEqual(runner.call_count, 1)
            receipt = json.loads((output / 'evolution/sample/execution.json').read_text())
            self.assertEqual(receipt['state'], 'partial')
            self.assertEqual(receipt['campaigns'][0]['exit_code'], 1)


if __name__ == '__main__':
    unittest.main()
