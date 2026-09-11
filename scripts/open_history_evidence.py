"""Resolve an imported observation using local gh credentials and its bundle manifest."""
import base64
import hashlib
import json
import mimetypes
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path, PurePosixPath


class EvidenceError(Exception):
    def __init__(self, availability, reason):
        self.availability = availability
        self.reason = reason


def fail(availability, reason):
    raise EvidenceError(availability, reason)


def relative(path):
    if not isinstance(path, str) or not path or path.startswith('/') or '\\' in path or any(p in ('', '.', '..') for p in path.split('/')):
        fail('integrity_invalid', 'Invalid relative evidence path.')
    return path


def digest(data):
    return 'sha256:' + hashlib.sha256(data).hexdigest()


def verified_file(archive, root, manifest, path):
    relative(path)
    entries = [item for item in manifest.get('files', []) if item.get('path') == path]
    names = [item for item in archive.infolist() if item.filename == root + path]
    if not entries or not names:
        fail('missing', 'The requested file is absent from the retained bundle.')
    if len(entries) != 1 or len(names) != 1 or names[0].file_size > 25 * 1024 * 1024:
        fail('integrity_invalid', 'Evidence is ambiguous or exceeds the 25 MiB file limit.')
    data = archive.read(names[0])
    if entries[0].get('sha256') != digest(data) or entries[0].get('size_bytes') != len(data):
        fail('integrity_invalid', 'Evidence checksum or size differs from its bundle manifest.')
    return data


def open_bundle(archive_path, value, bundle):
    report, execution = value['report'], value['execution']
    path = relative(value['path'])
    with zipfile.ZipFile(archive_path) as archive:
        manifests = []
        for info in archive.infolist():
            if PurePosixPath(info.filename).name != 'bundle-manifest.json':
                continue
            relative(info.filename)
            if info.file_size > 4 * 1024 * 1024:
                fail('integrity_invalid', 'Bundle manifest exceeds the size limit.')
            manifest = json.loads(archive.read(info))
            workflow = manifest.get('workflow', {})
            group = report.get('payload', {}).get('group', {}).get('group_id')
            if (manifest.get('schema') == 'e2e-observation-bundle/v1'
                    and manifest.get('execution_id') == execution['id']
                    and manifest.get('campaign_id') == execution['id']
                    and manifest.get('attempt') == execution['attempt']
                    and workflow.get('repository') == bundle['repository']
                    and workflow.get('run_id') == bundle['run_id']
                    and workflow.get('run_attempt') == report['runAttempt']
                    and (group is None or workflow.get('group_id') == group)):
                manifests.append((info.filename.removesuffix('bundle-manifest.json'), manifest))
        if len(manifests) != 1:
            fail('integrity_invalid', 'No unique bundle manifest matches the imported execution and attempt.')
        root, manifest = manifests[0]
        manifest_paths = {entry.get('path') for entry in manifest.get('files', [])}
        resolved = path
        if path not in manifest_paths:
            results = json.loads(verified_file(archive, root, manifest, 'results.json'))
            native = results.get('execution', {}).get('execution_id')
            if not isinstance(native, str):
                fail('missing', 'The bundle does not identify the native execution for this path.')
            relative(native)
            resolved = f'native/{native}/{path}'
        data = verified_file(archive, root, manifest, resolved)
        for run in report.get('payload', {}).get('runs', []):
            for deliverable in run.get('run', {}).get('deliverables', []):
                ref = deliverable.get('artifact', {})
                if ref.get('path') == path and (ref.get('sha256') != digest(data) or ref.get('size_bytes') != len(data)):
                    fail('integrity_invalid', 'Evidence differs from the original reported deliverable.')
        return {'availability': 'available', 'content_base64': base64.b64encode(data).decode(),
                'mime_type': mimetypes.guess_type(path)[0] or 'application/octet-stream'}


def gh_json(endpoint):
    result = subprocess.run(['gh', 'api', '--hostname', 'github.com', '--paginate', '--slurp', endpoint], capture_output=True, timeout=60)
    if result.returncode:
        fail('access_unavailable', 'GitHub artifacts cannot be queried with the local gh credentials.')
    return json.loads(result.stdout)


def resolve(value):
    relative(value['path'])
    report, execution = value['report'], value['execution']
    bundle = report.get('payload', {}).get('bundle')
    if not bundle:
        bundle = next((b for b in value['bundles'] if b['run_attempt'] == report['runAttempt']), None)
    if not bundle:
        fail('missing', 'No GitHub bundle reference was retained for this report.')
    name, artifact_id = bundle.get('artifact_name'), bundle.get('artifact_id')
    if not name and not artifact_id:
        group = report.get('payload', {}).get('group', {})
        if group.get('campaign_id') and group.get('group_id'):
            name = f"e2e-observation-{execution['id']}-{group['campaign_id']}-{group['group_id']}-gh-{report['runAttempt']}"
        else:
            fail('missing', 'The retained bundle reference has no artifact identity.')
    pages = gh_json(f"/repos/{bundle['repository']}/actions/runs/{bundle['run_id']}/artifacts?per_page=100")
    matches = [a for page in pages for a in page['artifacts'] if (a['id'] == artifact_id if artifact_id else a['name'] == name)]
    if not matches:
        fail('missing', 'No artifact matches the retained bundle identity.')
    if len(matches) != 1:
        fail('integrity_invalid', 'More than one artifact matches the retained bundle identity.')
    artifact = matches[0]
    if artifact['expired']:
        fail('expired', 'The GitHub artifact has expired.')
    if artifact.get('size_in_bytes', 0) > 256 * 1024 * 1024:
        fail('access_unavailable', 'The artifact exceeds the 256 MiB download limit.')
    with tempfile.TemporaryDirectory(prefix='harness-history-evidence-') as directory:
        path = Path(directory) / 'artifact.zip'
        with path.open('wb') as output:
            result = subprocess.run(['gh', 'api', '--hostname', 'github.com', f"/repos/{bundle['repository']}/actions/artifacts/{artifact['id']}/zip"], stdout=output, stderr=subprocess.DEVNULL, timeout=60)
        if result.returncode:
            fail('access_unavailable', 'GitHub artifact download is unavailable with the local gh credentials.')
        if path.stat().st_size > 256 * 1024 * 1024:
            fail('access_unavailable', 'The artifact exceeds the 256 MiB download limit.')
        if bundle.get('size_bytes') is not None and bundle['size_bytes'] != path.stat().st_size:
            fail('integrity_invalid', 'Downloaded bundle size differs from its retained reference.')
        if bundle.get('sha256') and bundle['sha256'].removeprefix('sha256:') != hashlib.sha256(path.read_bytes()).hexdigest():
            fail('integrity_invalid', 'Downloaded bundle checksum differs from its retained reference.')
        return open_bundle(path, value, bundle)


if __name__ == '__main__':
    try:
        result = resolve(json.load(sys.stdin))
    except EvidenceError as error:
        result = {'availability': error.availability, 'reason': error.reason}
    except (OSError, subprocess.TimeoutExpired):
        result = {'availability': 'access_unavailable', 'reason': 'Local Python/gh or GitHub access is unavailable.'}
    except (ValueError, KeyError, TypeError, zipfile.BadZipFile):
        result = {'availability': 'integrity_invalid', 'reason': 'The artifact or its manifest cannot be decoded.'}
    print(json.dumps(result))
