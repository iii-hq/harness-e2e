#!/usr/bin/env python3
"""Preview/export/run a reviewed profile using the pinned native materializer.

Each campaign round contains independent per-scenario invocations. Native
results and campaign bundles stay intact; the receipt links them by digest.
Fault profiles are exported for Release Control's protected executor.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

from run_e2e_campaign import CampaignError, _canonical_sha256, _write_json_atomic, parse_campaign
from exact_stack_campaign import validate_suite

ROOT = Path(__file__).resolve().parents[1]


def native_json(binary: Path, *args: str) -> dict:
    result = subprocess.run([str(binary), 'test-plan', *args], capture_output=True, text=True)
    if result.returncode:
        raise CampaignError(result.stderr.strip() or result.stdout.strip() or 'native materialization failed')
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise CampaignError('native materializer did not return JSON') from error
    if not isinstance(value, dict):
        raise CampaignError('native materializer must return an object')
    return value


def validate_snapshot(snapshot: dict, definition: dict) -> None:
    if snapshot.get('schema') != 'harness-e2e-profile-snapshot/v1':
        raise CampaignError('unsupported profile snapshot')
    if _canonical_sha256(definition) != snapshot.get('definition_sha256'):
        raise CampaignError('profile snapshot and definition differ')
    profile = snapshot['profile']
    if profile not in definition['profiles']:
        raise CampaignError('profile is absent from the reviewed definition')
    if len(snapshot['campaigns']) != profile['repetitions']:
        raise CampaignError('profile snapshot is missing campaign repetitions')
    for repetition, campaign in enumerate(snapshot['campaigns'], 1):
        parsed = parse_campaign(campaign)
        if parsed.test_plan is None or parsed.test_plan['repetition'] != repetition:
            raise CampaignError('profile repetition identity mismatch')
        if parsed.test_plan['profile_sha256'] != snapshot['profile_sha256']:
            raise CampaignError('profile scope digest mismatch')


def release_control_suite(campaign: dict, profile: dict, subject: dict, judge: dict) -> dict:
    groups = []
    for group in campaign['groups']:
        item = dict(group)
        item['weight'] = item.pop('difficulty_weight')
        groups.append(item)
    suite = {
        'id': campaign['campaign_id'], 'label': profile['label'],
        'lane': campaign['lane'], 'seed': None, 'subject': subject, 'judge': judge,
        'groups': groups, 'test_plan': campaign['test_plan'],
    }
    return validate_suite(suite)


def write_export(root: Path, snapshot: dict, definition: dict, subject: dict | None, judge: dict | None) -> list[Path]:
    # A materialized execution is immutable. Never merge with or overwrite an
    # older execution, including one interrupted before its receipt completed.
    root.mkdir(parents=True, exist_ok=False)
    _write_json_atomic(root / 'definition.json', definition)
    _write_json_atomic(root / 'profile.json', snapshot)
    paths = []
    for campaign in snapshot['campaigns']:
        path = root / 'campaigns' / f"{campaign['campaign_id']}.json"
        _write_json_atomic(path, campaign)
        paths.append(path)
        if subject and judge:
            suite = release_control_suite(campaign, snapshot['profile'], subject, judge)
            _write_json_atomic(root / 'release-control' / f"{campaign['campaign_id']}.json", suite)
    return paths


def argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument('--profile', choices=['smoke', 'regression', 'capability', 'evolution', 'resilience', 'endurance'])
    selection.add_argument('--import-profile', type=Path, help='Campaign bundle exported from the dashboard')
    parser.add_argument('--e2e-bin', type=Path, default=ROOT / 'target/debug/harness-e2e')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--execute', action='store_true', help='Run a profile against the configured stack; default only previews')
    mode.add_argument('--export', type=Path, help='Write immutable snapshots and campaigns without running models')
    parser.add_argument('--output-root', type=Path, default=ROOT / 'target/test-plans')
    parser.add_argument('--execution-id', default=None)
    for flag in ('model', 'provider', 'judge-model', 'judge-provider', 'url'):
        parser.add_argument('--' + flag)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = argument_parser().parse_args(argv)
    try:
        binary = args.e2e_bin.resolve(strict=True)
        imported = json.loads(args.import_profile.read_text()) if args.import_profile else None
        profile_id = args.profile
        if args.import_profile:
            if not isinstance(imported, dict) or imported.get('schema') != 'harness-e2e-profile-campaigns/v1':
                raise CampaignError('unsupported dashboard profile export')
            imported_profile = imported.get('profile')
            if not isinstance(imported_profile, dict) or not isinstance(imported_profile.get('id'), str):
                raise CampaignError('dashboard export must identify a profile')
            profile_id = imported_profile['id']
        snapshot = native_json(binary, 'materialize', '--profile', profile_id)
        definition = native_json(binary, 'definition')
        validate_snapshot(snapshot, definition)
        if args.import_profile:
            if imported.get('definition_sha256') != snapshot['definition_sha256'] or imported['profile'].get('profile_sha256') != snapshot['profile_sha256'] or imported['profile'].get('campaigns') != snapshot['campaigns']:
                raise CampaignError('dashboard export differs from the pinned runner profile')
        if not args.execute and args.export is None:
            print(json.dumps(snapshot, indent=2))
            return 0
        if args.execute and os.environ.get('HARNESS_E2E_SEED'):
            raise CampaignError('master profiles require scenario-owned seeds; unset HARNESS_E2E_SEED')
        if args.execute and snapshot['protected_supervisor_required']:
            raise CampaignError('this profile requires Release Control protected fault execution; use --export')
        subject = {
            'model': args.model or os.environ.get('HARNESS_E2E_MODEL'),
            'provider': args.provider or os.environ.get('HARNESS_E2E_PROVIDER'),
        }
        judge = {
            'model': args.judge_model or os.environ.get('HARNESS_E2E_JUDGE_MODEL'),
            'provider': args.judge_provider or os.environ.get('HARNESS_E2E_JUDGE_PROVIDER'),
        }
        if bool(subject['model']) != bool(subject['provider']) or bool(judge['model']) != bool(judge['provider']):
            raise CampaignError('model/provider identities must be supplied together')
        if args.execute and not all(subject.values()):
            raise CampaignError('execution requires a subject model and provider')
        if args.execute and any(case['judge_required'] for case in snapshot['cases']) and not all(judge.values()):
            raise CampaignError('this profile contains Markdown scenarios and requires an explicit judge')
        execution_id = args.execution_id or uuid.uuid4().hex
        if not execution_id or len(execution_id) > 64 or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-' for c in execution_id):
            raise CampaignError('unsafe execution id')
        root = args.export or args.output_root / profile_id / execution_id
        campaigns = write_export(root, snapshot, definition, subject if all(subject.values()) else None, judge if all(judge.values()) else None)
        if args.export:
            print(json.dumps({'exported': str(root), 'campaigns': len(campaigns), 'definition_sha256': snapshot['definition_sha256']}))
            return 0
        with binary.open('rb') as binary_file:
            binary_digest = 'sha256:' + hashlib.file_digest(binary_file, 'sha256').hexdigest()
        receipt = {
            'schema': 'harness-e2e-profile-execution/v1', 'execution_id': execution_id,
            'plan_id': snapshot['plan_id'], 'definition_sha256': snapshot['definition_sha256'],
            'profile_id': profile_id, 'profile_sha256': snapshot['profile_sha256'],
            'runner_sha256': binary_digest, 'subject': subject, 'judge': judge,
            'url': args.url or os.environ.get('III_URL', 'ws://127.0.0.1:49134'),
            'namespace': os.environ.get('III_NAMESPACE'),
            'budget': snapshot['budget'], 'state': 'running', 'campaigns': [],
            'missing_results': [],
            'interpretation': 'descriptive_only', 'advisory': True,
        }
        receipt_path = root / 'execution.json'
        _write_json_atomic(receipt_path, receipt)
        results_paths = []
        try:
            for path in campaigns:
                campaign_id = path.stem
                summary = root / 'results' / campaign_id / execution_id / 'campaign-summary.json'
                command = [sys.executable, str(ROOT / 'scripts/run_e2e_campaign.py'), str(path),
                    '--e2e-bin', str(binary), '--output-root', str(root / 'results'),
                    '--execution-id', execution_id, '--summary', str(summary)]
                for flag, value in [('model', subject['model']), ('provider', subject['provider']),
                    ('judge-model', judge['model']), ('judge-provider', judge['provider']), ('url', args.url)]:
                    if value: command.extend(['--' + flag, value])
                result = subprocess.run(command)
                entry = {'campaign_id': campaign_id, 'exit_code': result.returncode, 'summary': str(summary)}
                if summary.is_file():
                    evidence = json.loads(summary.read_text())
                    for group in evidence['groups']:
                        result_path = Path(group['output']) / 'results.json'
                        if result_path.is_file():
                            results_paths.append(result_path)
                        else:
                            receipt['missing_results'].append({
                                'campaign_id': campaign_id, 'group_id': group['group_id'],
                                'reason': 'native Results artifact unavailable',
                            })
                    entry['objective_passed'] = evidence.get('objective_passed')
                    entry['summary_sha256'] = 'sha256:' + hashlib.sha256(summary.read_bytes()).hexdigest()
                receipt['campaigns'].append(entry)
                _write_json_atomic(receipt_path, receipt)
                if result.returncode != 0 or not summary.is_file():
                    raise CampaignError(f'campaign {campaign_id} failed to produce a complete bundle; evidence retained at {root}')
            if results_paths:
                measure_args = [arg for path in results_paths for arg in ('--results', str(path))]
                measurements = native_json(binary, 'measure', *measure_args)
                _write_json_atomic(root / 'measurements.json', measurements)
                receipt['measurements'] = str(root / 'measurements.json')
            receipt['state'] = 'complete'
            receipt['objective_passed'] = all(entry.get('objective_passed') is True for entry in receipt['campaigns'])
            receipt['results_artifacts'] = len(results_paths)
        except BaseException:
            receipt['state'] = 'partial'
            raise
        finally:
            _write_json_atomic(receipt_path, receipt)
        print(json.dumps({'receipt': str(receipt_path), 'state': receipt['state'], 'advisory': True}))
        return 0
    except (CampaignError, OSError, ValueError, KeyError) as error:
        print(f'test plan error: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
