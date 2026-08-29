import json
import pathlib
import re

import pytest
import yaml

from scripts.release_compiler import read_pin


ROOT = pathlib.Path(__file__).parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
INDEX_WORKFLOW = ROOT / ".github" / "workflows" / "release-descriptor-index.yml"


def workflow():
    return yaml.load(WORKFLOW.read_text(encoding="utf-8"), Loader=yaml.BaseLoader)


def test_release_control_is_the_only_workflow_entrypoint():
    parsed = workflow()
    assert set(parsed["on"]) == {"workflow_dispatch"}
    inputs = parsed["on"]["workflow_dispatch"]["inputs"]
    assert len(inputs) <= 10
    assert {
        "identity",
        "source_sha",
        "descriptor_sha256",
        "descriptor_run_id",
        "descriptor_artifact",
    } <= set(inputs)
    assert not (ROOT / ".github" / "workflows" / "promote-latest.yml").exists()


def test_actions_are_immutable_and_mac_builds_use_the_warm_pool():
    text = "\n".join(
        path.read_text(encoding="utf-8") for path in (WORKFLOW, INDEX_WORKFLOW)
    )
    references = re.findall(r"^\s*-?\s*uses:\s*([^\s#]+)", text, flags=re.MULTILINE)
    assert references
    for reference in references:
        if reference.startswith("./"):
            continue
        assert re.fullmatch(r"[^@]+@[0-9a-f]{40}", reference), reference

    matrix = workflow()["jobs"]["binaries"]["strategy"]["matrix"]["include"]
    assert len(matrix) == 9
    mac = [entry for entry in matrix if entry["target"].endswith("apple-darwin")]
    assert len(mac) == 2
    assert all("workers-release-macos-12core" in entry["os"] for entry in mac)


def test_result_and_registry_publication_are_descriptor_native():
    text = WORKFLOW.read_text(encoding="utf-8")
    assert "validate-descriptor" in text
    assert "frontend-metadata" in text
    assert "build-frontends" in text
    assert "pnpm --dir dashboard" not in text
    assert "--phase harness_release" in text
    assert "release-result-${{ fromJSON(inputs.identity).candidate_id }}-${{ fromJSON(inputs.identity).step_id }}-attempt-${{ github.run_attempt }}" in text
    assert "--descriptor release-descriptor-input/release-descriptor.json" in text
    assert "--data-binary @registry-payload.json" in text
    assert workflow()["jobs"]["report"]["if"] == "${{ always() }}"


def test_descriptor_index_is_compiled_once_by_the_pinned_iii():
    text = INDEX_WORKFLOW.read_text(encoding="utf-8")
    assert "iii compose descriptor-index" not in text
    assert '"$III_BIN" compose descriptor-index' in text
    assert "--compiler-sha '${{ steps.compiler.outputs.commit }}'" in text
    assert "release-descriptor-index-${{ steps.source.outputs.sha }}" in text


def test_compiler_pin_is_full_and_fails_closed(tmp_path: pathlib.Path):
    pin = read_pin(ROOT / ".github" / "release-compiler.json")
    assert pin["repository"] == "iii-hq/iii"
    assert re.fullmatch(r"[0-9a-f]{40}", pin["commit"])

    invalid = tmp_path / "pin.json"
    invalid.write_text(json.dumps({**pin, "commit": "0" * 40}), encoding="utf-8")
    with pytest.raises(SystemExit):
        read_pin(invalid)
