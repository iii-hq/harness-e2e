import io
import json
import sys
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'scripts'))
import open_history_evidence as evidence


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.content = b'{"observation":"retained"}'
        self.bundle = {'kind': 'github', 'repository': 'owner/repo', 'run_id': 42, 'run_attempt': 2, 'artifact_name': 'group'}
        self.input = {'execution': {'id': 'exec', 'attempt': 1}, 'report': {'runAttempt': 2, 'payload': {'bundle': self.bundle, 'group': {'group_id': 'group'}}}, 'path': 'result.txt', 'bundles': [self.bundle]}
        self.manifest = {'schema': 'e2e-observation-bundle/v1', 'execution_id': 'exec', 'campaign_id': 'exec', 'attempt': 1,
                         'workflow': {'repository': 'owner/repo', 'run_id': 42, 'run_attempt': 2, 'group_id': 'group'},
                         'files': [{'path': 'result.txt', 'sha256': evidence.digest(self.content), 'size_bytes': len(self.content)}]}

    def archive(self, content=None):
        value = io.BytesIO()
        with zipfile.ZipFile(value, 'w') as archive:
            archive.writestr('bundle-manifest.json', json.dumps(self.manifest))
            archive.writestr('result.txt', self.content if content is None else content)
        value.seek(0)
        return value

    def test_verified_non_image_file_and_identity(self):
        result = evidence.open_bundle(self.archive(), self.input, self.bundle)
        self.assertEqual(result['availability'], 'available')
        self.assertEqual(evidence.base64.b64decode(result['content_base64']), self.content)
        self.manifest['workflow']['run_attempt'] = 1
        with self.assertRaises(evidence.EvidenceError) as error:
            evidence.open_bundle(self.archive(), self.input, self.bundle)
        self.assertEqual(error.exception.availability, 'integrity_invalid')

    def test_tampering_missing_file_and_traversal(self):
        with self.assertRaises(evidence.EvidenceError) as error:
            evidence.open_bundle(self.archive(b'changed'), self.input, self.bundle)
        self.assertEqual(error.exception.availability, 'integrity_invalid')
        self.manifest['files'] = []
        with self.assertRaises(evidence.EvidenceError) as error:
            evidence.open_bundle(self.archive(), self.input, self.bundle)
        self.assertEqual(error.exception.availability, 'missing')
        self.input['path'] = '../credentials'
        with self.assertRaises(evidence.EvidenceError) as error:
            evidence.open_bundle(self.archive(), self.input, self.bundle)
        self.assertEqual(error.exception.availability, 'integrity_invalid')

    @patch.object(evidence, 'gh_json')
    def test_expiry_and_missing_are_distinct(self, query):
        query.return_value = [{'artifacts': []}]
        with self.assertRaises(evidence.EvidenceError) as error:
            evidence.resolve(self.input)
        self.assertEqual(error.exception.availability, 'missing')
        query.return_value = [{'artifacts': [{'id': 8, 'name': 'group', 'expired': True}]}]
        with self.assertRaises(evidence.EvidenceError) as error:
            evidence.resolve(self.input)
        self.assertEqual(error.exception.availability, 'expired')

    @patch.object(evidence.subprocess, 'run')
    def test_authentication_unavailable(self, command):
        command.return_value.returncode = 1
        with self.assertRaises(evidence.EvidenceError) as error:
            evidence.gh_json('/repos/owner/repo/actions/runs/42/artifacts')
        self.assertEqual(error.exception.availability, 'access_unavailable')
