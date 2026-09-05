"""Behavioral qualification for the separately pinned SWE fixture and trusted probes."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
BUNDLE = ROOT / 'tests/fixtures/campaign/swe-service.bundle'
PROBES = ROOT / 'src/scenarios/swe_service/probes.py'


class FixtureContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        override = os.environ.get('SWE_FIXTURE_ROOT')
        if override:
            cls.fixture = Path(override).resolve() / 'swe-service'
        else:
            if not BUNDLE.is_file():
                raise AssertionError(f'packaged SWE fixture bundle is missing: {BUNDLE}')
            temporary = tempfile.TemporaryDirectory(prefix='swe-fixture-qualification-')
            cls.addClassCleanup(temporary.cleanup)
            clone = Path(temporary.name) / 'fixture'
            subprocess.run(['git', '-c', 'advice.detachedHead=false', 'clone', '--quiet', str(BUNDLE), str(clone)],
                           check=True, capture_output=True, text=True, timeout=30)
            cls.fixture = clone / 'swe-service'
        if not (cls.fixture / 'snapshots/00').is_dir():
            raise AssertionError(f'runnable SWE fixture is missing: {cls.fixture}')

    def probe(self, workspace, through, canary=False):
        args = [sys.executable, '-I', str(PROBES), '--workspace', str(workspace), '--through', str(through)]
        if canary:
            args.append('--canary')
        result = subprocess.run(args, capture_output=True, text=True, timeout=90)
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        return json.loads(result.stdout)

    def test_all_reference_prefixes_pass_and_next_ticket_fails(self):
        self.assertTrue((self.fixture / 'snapshots/00').is_dir(), 'stage-zero runnable fixture must exist')
        for stage in range(9):
            with self.subTest(stage=stage):
                workspace = self.fixture / 'snapshots' / f'{stage:02}'
                result = self.probe(workspace, stage, stage >= 5)
                self.assertTrue(result['passed'], result)
                if stage < 8:
                    result = self.probe(workspace, stage + 1, stage + 1 >= 5)
                    self.assertFalse(result['passed'], f'ticket {stage + 1} is already solved')
                    self.assertTrue(any(not item['passed'] and item['id'].startswith(f'ticket{stage + 1}') for item in result['checks']), result)

    def test_regressions_are_rejected_and_canary_is_delayed(self):
        self.assertTrue((self.fixture / 'snapshots/08').is_dir(), 'complete reference fixture must exist')
        regressions = [(1, 'config.py'), (2, 'service.py'), (3, 'replay.py'), (4, 'replay.py'), (5, 'service.py'), (6, 'store.py'), (7, 'replay.py'), (8, 'config.py')]
        for ticket, module in regressions:
            with self.subTest(ticket=ticket), tempfile.TemporaryDirectory() as td:
                workspace = Path(td) / 'subject'
                shutil.copytree(self.fixture / 'snapshots/08', workspace)
                shutil.copyfile(self.fixture / f'snapshots/{ticket - 1:02}/src/profile_service/{module}', workspace / f'src/profile_service/{module}')
                result = self.probe(workspace, ticket, ticket >= 5)
                self.assertFalse(result['passed'], f'regression {ticket} escaped')
                self.assertTrue(any(not check['passed'] and check['id'].startswith(f'ticket{ticket}') for check in result['checks']), result)
        with tempfile.TemporaryDirectory() as td:
            workspace = Path(td) / 'subject'
            shutil.copytree(self.fixture / 'snapshots/08', workspace)
            path = workspace / 'src/profile_service/client.py'
            path.write_text(path.read_text().replace('return payload["name"]', 'return payload["display_name"]'))
            self.assertTrue(self.probe(workspace, 5)['passed'])
            result = self.probe(workspace, 5, True)
            self.assertFalse(result['passed'], result)
            self.assertTrue(any(x['id'] == 'ticket5.legacy_canary' and not x['passed'] for x in result['checks']))

    def test_cli_cursor_and_handoff_regressions_are_rejected(self):
        mutations = [
            (1, 'src/profile_service/config.py', '        self.values.update(self.cli_overrides)\n', ''),
            (3, 'src/profile_service/replay.py', 'islice(source, start_cursor)', 'islice(source, 0)'),
            (3, 'src/profile_service/http.py', "start_cursor=data.get('start_cursor', 0)", 'start_cursor=0'),
            (3, 'src/profile_service/replay.py', 'isinstance(batch_size, bool) or ', ''),
            (8, 'src/profile_service/config.py',
             '    def set_cli_overrides(self, overrides):\n        self.cli_overrides = copy.deepcopy(overrides or {})',
             '    def set_cli_overrides(self, overrides):\n        self.cli_overrides.update(copy.deepcopy(overrides or {}))'),
            (8, 'docs/delivery.md', None, None),
        ]
        for ticket, relative, old, new in mutations:
            with self.subTest(ticket=ticket, mutation=relative, old=old), tempfile.TemporaryDirectory() as td:
                workspace = Path(td) / 'subject'
                shutil.copytree(self.fixture / 'snapshots/08', workspace)
                path = workspace / relative
                if old is None:
                    path.unlink()
                else:
                    source = path.read_text()
                    self.assertIn(old, source, 'mutation must modify a real production operation')
                    path.write_text(source.replace(old, new))
                result = self.probe(workspace, ticket, ticket >= 5)
                self.assertTrue(any(not check['passed'] and check['id'].startswith(f'ticket{ticket}')
                                    for check in result['checks']), result)

    def test_cache_hit_bypass_is_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            workspace = Path(td) / 'subject'
            shutil.copytree(self.fixture / 'snapshots/08', workspace)
            path = workspace / 'src/profile_service/service.py'
            source = path.read_text()
            enforcement = ("        settings = self.settings()\n"
                           "        capacity = max(0, settings['cache_size']) if settings['cache_enabled'] else 0\n"
                           "        while len(self.cache) > capacity:\n"
                           "            self.cache.popitem(last=False)\n")
            self.assertIn(enforcement, source)
            source = source.replace(enforcement, '', 1)
            source = source.replace('        payload = self.format_profile(row, version)\n',
                                    '        payload = self.format_profile(row, version)\n' + enforcement)
            path.write_text(source)
            result = self.probe(workspace, 2)
            check = next(check for check in result['checks'] if check['id'] == 'ticket2.cache')
            self.assertFalse(check['passed'], result)
            self.assertEqual(check['reason'], 'cache hit ignored a lowered positive capacity')

    def test_empty_authored_test_directory_is_accepted(self):
        with tempfile.TemporaryDirectory() as td:
            workspace = Path(td) / 'subject'
            shutil.copytree(self.fixture / 'snapshots/08', workspace)
            authored = workspace / 'tests/agent'
            shutil.rmtree(authored)
            authored.mkdir()
            before = {str(path.relative_to(workspace)): path.read_bytes()
                      for path in workspace.rglob('*') if path.is_file()}
            result = self.probe(workspace, 0)
            self.assertTrue(result['passed'], result)
            self.assertEqual({str(path.relative_to(workspace)): path.read_bytes()
                              for path in workspace.rglob('*') if path.is_file()}, before)

    def test_authored_unittests_gate_acceptance_without_mutating_sources(self):
        with tempfile.TemporaryDirectory() as td:
            workspace = Path(td) / 'subject'
            shutil.copytree(self.fixture / 'snapshots/08', workspace)
            authored = workspace / 'tests/agent/test_authored.py'
            authored.write_text("import unittest\n\nclass AuthoredRegression(unittest.TestCase):\n"
                                "    def test_expected_behavior(self):\n        self.assertEqual(2 + 2, 5)\n")
            before = {str(path.relative_to(workspace)): path.read_bytes()
                      for path in workspace.rglob('*') if path.is_file()}
            result = self.probe(workspace, 0)
            self.assertFalse(result['passed'], 'failing authored tests must reject a checkpoint')
            self.assertTrue(any(check['id'] == 'baseline' and not check['passed']
                                for check in result['checks']), result)
            self.assertEqual({str(path.relative_to(workspace)): path.read_bytes()
                              for path in workspace.rglob('*') if path.is_file()}, before)
            authored.write_text(authored.read_text().replace('2 + 2, 5', '2 + 2, 4'))
            before = {str(path.relative_to(workspace)): path.read_bytes()
                      for path in workspace.rglob('*') if path.is_file()}
            result = self.probe(workspace, 0)
            self.assertTrue(result['passed'], result)
            self.assertEqual({str(path.relative_to(workspace)): path.read_bytes()
                              for path in workspace.rglob('*') if path.is_file()}, before)

    def test_crash_probe_really_stops_kills_and_restarts_application(self):
        self.assertTrue((self.fixture / 'snapshots/04').is_dir(), 'recovery reference fixture must exist')
        before = sorted(str(p.relative_to(self.fixture)) for p in self.fixture.rglob('*') if p.is_file())
        result = self.probe(self.fixture / 'snapshots/04', 4)
        self.assertEqual(sorted(str(p.relative_to(self.fixture)) for p in self.fixture.rglob('*') if p.is_file()), before, 'verifier mutated the immutable fixture')
        recovery = next(x for x in result['checks'] if x['id'] == 'ticket4.recovery')
        self.assertTrue(recovery['passed'], recovery)
        self.assertEqual(recovery['evidence']['signal'], 'SIGKILL')
        self.assertTrue(recovery['evidence']['persistence_boundary_reached'])
        self.assertEqual(recovery['evidence']['application_returncode'], -9)
        self.assertEqual(recovery['evidence']['final_score'], 12)


if __name__ == '__main__':
    unittest.main()
