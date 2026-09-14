"""Real rollout and rollback qualification against a separately pinned product fixture."""
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
BUNDLE = ROOT / 'tests/fixtures/campaign/swe-service.bundle'
PROBES = ROOT / 'src/scenarios/swe_service/probes.py'
ISOLATION = ROOT / 'src/scenarios/swe_service/isolation.py'


class SoftwareCompanyOperations(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        temporary = tempfile.TemporaryDirectory(prefix='lifecycle-operations-fixture-')
        cls.addClassCleanup(temporary.cleanup)
        clone = Path(temporary.name) / 'fixture'
        subprocess.run(['git', '-c', 'advice.detachedHead=false', 'clone', '--quiet', str(BUNDLE), str(clone)],
                       check=True, capture_output=True, text=True, timeout=30)
        cls.snapshots = clone / 'swe-service/snapshots'

    def probe(self, workspace, through, previous=None, canary=False):
        command = [sys.executable, '-I', str(PROBES), '--workspace', str(workspace),
                   '--through', str(through), '--lifecycle']
        if previous:
            command += ['--previous-workspace', str(previous)]
        if canary:
            command.append('--canary')
        result = subprocess.run(command, capture_output=True, text=True, timeout=90)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return json.loads(result.stdout)

    def test_rollout_and_recovery_preserve_http_and_sqlite_state(self):
        rollout = self.probe(self.snapshots / '05', 5)
        check = next(item for item in rollout['checks'] if item['id'] == 'lifecycle.rollout')
        self.assertTrue(check['passed'], rollout)
        self.assertEqual(check['evidence']['starts'], 2)
        self.assertEqual(check['evidence']['acknowledgement_rows'], [['alpha', 'rollout-seed']])
        self.assertNotIn('v1', check['evidence']['start_0'])
        self.assertNotIn('legacy', check['evidence']['start_0'])

        canary = self.probe(self.snapshots / '05', 5, canary=True)
        check = next(item for item in canary['checks'] if item['id'] == 'lifecycle.rollout')
        self.assertTrue(check['passed'], canary)
        self.assertEqual(check['evidence']['start_0']['legacy']['score'], 5)

        release = self.probe(self.snapshots / '08', 8, self.snapshots / '05', canary=True)
        check = next(item for item in release['checks'] if item['id'] == 'lifecycle.recovery')
        self.assertTrue(release['passed'], release)
        self.assertEqual(check['evidence']['killed_returncode'], -9)
        self.assertEqual(check['evidence']['persisted_profile'],
                         [['alpha', 'release', 'Release Customer', 17, 4]])
        self.assertEqual(len(check['evidence']['acknowledgement_rows']), 4)
        self.assertEqual([stage['stage'] for stage in check['evidence']['stages']], [
            'previous_seed', 'candidate_upgrade', 'candidate_update', 'candidate_restart',
            'previous_rollback', 'previous_update', 'candidate_reupgrade', 'candidate_final'])
        comparison = next(item for item in release['checks'] if item['id'] == 'lifecycle.performance_repair')
        self.assertTrue(comparison['passed'], release)
        published = comparison['evidence']['published']
        repaired = comparison['evidence']['repaired']
        self.assertEqual(published['profile_score'], repaired['profile_score'])
        self.assertEqual(published['acknowledged_events'], repaired['acknowledged_events'])
        self.assertAlmostEqual(comparison['evidence']['published_to_repaired_work_ratio'],
                               published['sqlite_vm_instructions_sampled'] /
                               repaired['sqlite_vm_instructions_sampled'])

    def test_already_optimized_previous_release_is_valid(self):
        with tempfile.TemporaryDirectory(prefix='lifecycle-optimized-previous-') as directory:
            previous = Path(directory) / 'previous'
            shutil.copytree(self.snapshots / '08', previous)
            result = self.probe(self.snapshots / '08', 8, previous)
            comparison = next(item for item in result['checks'] if item['id'] == 'lifecycle.performance_repair')
            self.assertTrue(result['passed'], result)
            self.assertTrue(comparison['passed'], result)
            self.assertEqual(comparison['evidence']['published_to_repaired_work_ratio'], 1.0)

    def test_missing_previous_release_and_unacknowledged_events_are_rejected(self):
        missing = self.probe(self.snapshots / '08', 8)
        check = next(item for item in missing['checks'] if item['id'] == 'lifecycle.recovery')
        self.assertFalse(missing['passed'])
        self.assertFalse(check['passed'])
        self.assertIn('previous release export is required', check['reason'])

        with tempfile.TemporaryDirectory(prefix='lifecycle-operations-mutant-') as directory:
            workspace = Path(directory) / 'candidate'
            shutil.copytree(self.snapshots / '08', workspace)
            path = workspace / 'src/profile_service/store.py'
            source = path.read_text()
            marker = '    def mark_event(self, event):\n'
            self.assertIn(marker, source)
            path.write_text(source.replace(marker, marker + '        return\n', 1))
            result = self.probe(workspace, 8, self.snapshots / '05')
            check = next(item for item in result['checks'] if item['id'] == 'lifecycle.recovery')
            self.assertFalse(result['passed'])
            self.assertFalse(check['passed'], result)

    def test_previous_export_is_only_mounted_read_only_when_requested(self):
        spec = importlib.util.spec_from_file_location('swe_isolation', ISOLATION)
        isolation = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(isolation)
        previous = self.snapshots / '05'
        current = self.snapshots / '08'
        bwrap = isolation._bwrap_command('bwrap', current, PROBES, previous)
        docker = isolation._docker_command('docker', 'sha256:trusted', current, PROBES, 'test', previous)
        self.assertIn(['--ro-bind', str(previous), '/previous'],
                      [bwrap[index:index + 3] for index in range(len(bwrap))])
        self.assertIn(f'type=bind,src={previous},dst=/previous,readonly', docker)
        self.assertNotIn(str(previous), isolation._bwrap_command('bwrap', current, PROBES))
        self.assertFalse(any(str(previous) in item for item in
                             isolation._docker_command('docker', 'sha256:trusted', current, PROBES, 'test')))


if __name__ == '__main__':
    unittest.main()
