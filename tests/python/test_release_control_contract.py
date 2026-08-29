import hashlib
import json
import pathlib
import subprocess
import sys


ROOT = pathlib.Path(__file__).parents[2]
SCRIPT = ROOT / "scripts" / "release_control_contract.py"
SCHEMA = ROOT / ".github" / "contracts" / "release-execution.schema.json"
OPERATION_ID = "11111111-1111-4111-8111-111111111111"
STEP_ID = "22222222-2222-4222-8222-222222222222"
INTENT_ID = "33333333-3333-4333-8333-333333333333"
CANDIDATE_ID = "44444444-4444-4444-8444-444444444444"
ATTEMPT_ID = "55555555-5555-4555-8555-555555555555"
NONCE = "66666666-6666-4666-8666-666666666666"


def run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        text=True,
        capture_output=True,
        check=False,
    )


def test_contract_schema_is_pinned_to_release_control():
    assert hashlib.sha256(SCHEMA.read_bytes()).hexdigest() == (
        "947ccf41d51918901d994a484c5c0efaac7e8966310627354dccbb37e53733fa"
    )


def test_executor_callbacks_use_the_root_mounted_release_control_routes():
    source = SCRIPT.read_text(encoding="utf-8")
    assert 'args.api_url.rstrip("/") + "/executor-dispatches/authorize"' in source
    assert 'args.api_url.rstrip("/") + "/executor-results"' in source
    assert 'args.api_url.rstrip("/") + "/api/executor' not in source


def test_mutating_harness_reruns_are_rejected():
    common = [
        "validate-dispatch",
        "--operation-id",
        OPERATION_ID,
        "--step-id",
        STEP_ID,
        "--dispatch-nonce",
        NONCE,
        "--descriptor-sha256",
        "d" * 64,
        "--plan-hash",
        "b" * 64,
        "--source-sha",
        "a" * 40,
        "--mutating",
    ]
    assert run(*common, "--run-attempt", "1").returncode == 0
    assert run(*common, "--run-attempt", "2").returncode == 2


def test_harness_release_result_has_exact_contract(tmp_path: pathlib.Path):
    output = tmp_path / "release-result.json"
    result = run(
        "write-result",
        "--repository",
        "iii-hq/harness-e2e",
        "--operation-id",
        OPERATION_ID,
        "--step-id",
        STEP_ID,
        "--run-id",
        "123",
        "--run-attempt",
        "1",
        "--workflow",
        "release.yml",
        "--event",
        "workflow_dispatch",
        "--sha",
        "a" * 40,
        "--release-intent-id",
        INTENT_ID,
        "--candidate-id",
        CANDIDATE_ID,
        "--attempt-id",
        ATTEMPT_ID,
        "--dispatch-nonce",
        NONCE,
        "--plan-hash",
        "b" * 64,
        "--worker",
        "harness-e2e",
        "--phase",
        "harness_release",
        "--source-sha",
        "a" * 40,
        "--prepared-sha",
        "a" * 40,
        "--candidate-version",
        "0.1.1-experimental",
        "--descriptor-sha256",
        "d" * 64,
        "--outcome",
        "succeeded",
        "--effects",
        '[{"surface":"registry-version","state":"present","immutable_id":"harness-e2e@0.1.1-experimental"}]',
        "--artifacts-json",
        '[{"name":"harness-e2e.tar.gz","role":"bundle","sha256":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","size":42}]',
        "--error-json",
        "null",
        "--output",
        str(output),
    )
    assert result.returncode == 0, result.stderr
    payload = json.loads(output.read_text())
    assert set(payload) == {
        "contract",
        "identity",
        "executor",
        "subject",
        "outcome",
        "effects",
        "artifacts",
        "error",
        "completed_at",
    }
    assert payload["contract"] == "release-execution"
    assert payload["executor"]["repository"] == "iii-hq/harness-e2e"
    assert payload["executor"]["workflow"] == "release.yml"
    assert payload["subject"]["worker"] == "harness-e2e"
    assert payload["subject"]["phase"] == "harness_release"
