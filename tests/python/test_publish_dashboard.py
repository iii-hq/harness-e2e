from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from publish_harness_e2e_dashboard import (
    _analyzer_profile_sha256,
    _assessment_profile_sha256,
    _assessment_summary,
    build_execution_efficiency_totals,
    build_scenario_metrics,
    build_static_test_catalog,
    complete_public_detail,
    publish,
)


def report(revision: str, scores: list[int]) -> dict:
    runs = [
        {
            "run_id": f"run-{index}",
            "attempt_id": f"attempt-{index}",
            "status": "passed",
            "score": score,
            "prompt": "private prompt must not be published",
            "transcript": {"messages": [{"content": "private transcript"}]},
            "wall_time_ms": 1_000 + score,
            "cost": {"total_usd": score / 1_000},
            "metrics": {
                "totals": {
                    "input_tokens": score,
                    "output_tokens": 10,
                }
            },
            "efficiency": {"context_compactions": index},
            "deliverables": [
                {
                    "id": "artifact",
                    "preview": "private asset content",
                    "artifact": {
                        "id": "artifact",
                        "kind": "generated_asset",
                        "path": "/private/output.txt",
                        "sha256": "sha256:asset",
                    },
                }
            ],
        }
        for index, score in enumerate(scores)
    ]
    assessment_runs = [
        {
            "run_id": run["run_id"],
            "attempt_id": run["attempt_id"],
            "system_status": "passed",
            "assessments": [
                {
                    "criterion_id": "correctness",
                    "target": {"kind": "criterion", "id": "correctness"},
                    "kind": "signal",
                    "policy": "advisory",
                    "dimension": "structural_integrity",
                    "source": "judge",
                    "outcome": "passed",
                    "score": {"awarded": run["score"], "possible": 100},
                    "confidence": 0.9,
                    "summary": "Correct result",
                    "evidence": [
                        {
                            "artifact_id": "transcript",
                            "artifact_sha256": "sha256:evidence",
                        }
                    ],
                    "analyzer": {
                        "analyzer": "criterion-assessment",
                        "provider": "openai",
                        "model": "judge",
                        "input_sha256": "sha256:criterion-input",
                    },
                }
            ],
            "assets": [],
            "ai_final_assessment": {
                "availability": "available",
                "result": {
                    "verdict": "pass",
                    "quality_score": run["score"],
                    "confidence": 0.95,
                    "summary": "Passed",
                    "facts": ["System passed"],
                    "strengths": [],
                    "concerns": [],
                    "recommendation": "Accept",
                    "limitations": [],
                    "evidence": [
                        {
                            "artifact_id": "transcript",
                            "artifact_sha256": "sha256:evidence",
                        }
                    ],
                },
                "analyzer": {
                    "analyzer": "final-assessment",
                    "provider": "openai",
                    "model": "judge",
                    "input_sha256": "sha256:final-input",
                },
            },
            "effective_status": "passed",
        }
        for run in runs
    ]
    return {
        "execution": {"lane": "daily"},
        "subject": {"provider": "openai", "model": "subject"},
        "judge": {"provider": "openai", "model": "judge"},
        "judge_protocol": "plain-json",
        "system_under_test": {
            "stack": {
                "mode": "source",
                "workers_repository": "iii-hq/workers",
                "workers_revision": revision,
            },
            "engine_version": "1.0.0",
            "harness_version": "2.0.0",
            "contract_hashes": {"harness::send": "sha256:contract"},
        },
        "assessment_contract": {"runs": assessment_runs},
        "scenarios": [
            {
                "scenario_id": "coordination.parallel",
                "scenario_version": 3,
                "case_id": "coordination.parallel:v3:seed-8",
                "case": {"seed": 8},
                "execution_policy": {"max_turns": 4},
                "passed": True,
                "aggregate": {
                    "hard_gate_failures": 0,
                    "technical_failures": 0,
                },
                "runs": runs,
            }
        ],
    }


def metadata(execution_id: str) -> dict:
    return {
        "id": execution_id,
        "run_id": execution_id,
        "attempt": 1,
        "workflow_name": "Harness E2E",
        "workflow_url": "https://example.test/run",
        "event": "workflow_dispatch",
        "actor": "tester",
        "started_at": "2026-08-11T00:00:00Z",
        "completed_at": "2026-08-11T00:01:00Z",
        "conclusion": "success",
        "head_sha": "a" * 40,
        "head_branch": "main",
        "repository": "iii-hq/harness-e2e",
    }


def contains_key(value: object, forbidden: str) -> bool:
    if isinstance(value, dict):
        return forbidden in value or any(
            contains_key(item, forbidden) for item in value.values()
        )
    if isinstance(value, list):
        return any(contains_key(item, forbidden) for item in value)
    return False


class PublishDashboardTests(unittest.TestCase):
    def test_compaction_samples_exclude_legacy_absence_without_inventing_totals(self) -> None:
        legacy_mixed = report("a" * 40, [10, 20])
        legacy_mixed["scenarios"][0]["runs"][1].pop("efficiency")
        detail = {
            "reports": [
                {
                    "available": True,
                    "subject_id": "openai-subject",
                    "scenario_id": "coordination.parallel",
                    "report": legacy_mixed,
                }
            ]
        }

        scenario_metric = build_scenario_metrics(detail)[0]

        self.assertEqual(scenario_metric["averages"]["context_compactions"], 0)
        self.assertEqual(scenario_metric["samples"]["context_compactions"], 1)
        self.assertIsNone(
            build_execution_efficiency_totals(detail)["context_compactions"]
        )

    def test_publish_writes_json_manifest_and_removes_legacy_runtime_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            site = Path(directory)
            (site / "executions.js").write_text(
                "window.HARNESS_EXECUTIONS = {\"executions\": []};\n"
            )
            updated = publish(
                site,
                snapshot_path=None,
                detail_path=None,
                metadata=metadata("json-manifest"),
                repo_url="https://example.test/repo",
                max_summaries=10,
                max_details=2,
            )

            manifest_path = site / "executions.json"
            self.assertEqual(json.loads(manifest_path.read_text()), updated)
            self.assertFalse((site / "executions.js").exists())
            self.assertEqual(updated["executions"][0]["availability"], "unavailable")

    def test_shared_assessment_projection_fixture(self) -> None:
        fixture = json.loads(
            (ROOT / "tests/fixtures/results/results-assessment-contract.json").read_text()
        )
        runs = fixture["assessment_contract"]["runs"]
        expected = fixture["dashboard_projection"]
        self.assertEqual(_assessment_summary(runs), expected["summary"])
        self.assertEqual(
            _assessment_profile_sha256(expected["scenario_version"], runs),
            expected["assessment_profile_sha256"],
        )
        self.assertEqual(
            _analyzer_profile_sha256(runs), expected["analyzer_profile_sha256"]
        )

    def test_static_catalog_pools_raw_runs_and_shards_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            site = Path(directory)
            runs = site / "runs"
            runs.mkdir()
            executions = []
            for index, (revision, scores) in enumerate(
                [("a" * 40, [10, 100, 100]), ("b" * 40, [80, 90])], start=1
            ):
                execution_id = f"execution-{index}"
                detail_path = f"runs/{execution_id}.json"
                raw_detail = {
                    "lane": "daily",
                    "reports": [
                        {
                            "available": True,
                            "subject_id": "openai-subject",
                            "scenario_id": "coordination.parallel",
                            "report": report(revision, scores),
                        }
                    ],
                }
                public_detail = complete_public_detail(
                    raw_detail, metadata(execution_id)
                )
                self.assertEqual(
                    public_detail["reports"][0]["report"]["scenarios"][0]["runs"][0][
                        "efficiency"
                    ]["context_compactions"],
                    0,
                )
                (site / detail_path).write_text(json.dumps(public_detail))
                for forbidden in ("prompt", "transcript", "preview", "path"):
                    self.assertFalse(contains_key(public_detail, forbidden))
                executions.append(
                    {
                        "id": execution_id,
                        "completed_at": f"2026-08-1{index}T00:00:00Z",
                        "status": "passed",
                        "detail_path": detail_path,
                    }
                )

            catalog = build_static_test_catalog(site, executions)

            self.assertEqual(catalog["tests"]["total"], 1)
            row = catalog["tests"]["rows"][0]
            self.assertEqual(row["test_id"], "coordination.parallel")
            self.assertEqual(row["available_versions"][0]["run_count"], 5)
            sides = row["version_results"]["3"]["sides"]
            medians = sorted(side["summary"]["median_score"] for side in sides.values())
            self.assertEqual(medians, [85.0, 100.0])
            compaction_medians = sorted(
                side["summary"]["median_context_compactions"]
                for side in sides.values()
            )
            self.assertEqual(compaction_medians, [0.5, 1.0])
            self.assertTrue(
                all(
                    side["summary"]["samples"]["context_compactions"]
                    == side["summary"]["total_runs"]
                    for side in sides.values()
                )
            )
            self.assertTrue(all("::" in side_id for side_id in sides))
            self.assertTrue(
                all(
                    side["summary"]["assessment_summary"]["run_count"] > 0
                    for side in sides.values()
                )
            )
            shard_path = site / row["shards"]["3"].removeprefix("./")
            shard = json.loads(shard_path.read_text())
            self.assertEqual(len(shard["observations"]), 2)
            self.assertNotIn("runs", shard["observations"][0])
            self.assertIn("cohort_id", shard["observations"][0])
            self.assertIn("median_context_compactions", shard["observations"][0])
            self.assertTrue(
                shard["observations"][0]["assessment_profile_sha256"].startswith(
                    "sha256:"
                )
            )

    def test_legacy_detail_is_explicitly_unavailable_without_analyzer_output(self) -> None:
        legacy = report("a" * 40, [100])
        legacy.pop("assessment_contract")
        public = complete_public_detail(
            {
                "reports": [
                    {
                        "available": True,
                        "subject_id": "openai-subject",
                        "scenario_id": "coordination.parallel",
                        "report": legacy,
                    }
                ]
            },
            metadata("legacy-execution"),
        )
        projected_report = public["reports"][0]["report"]
        self.assertEqual(projected_report["assessment_availability"], "unavailable")
        projected_run = projected_report["scenarios"][0]["runs"][0]["assessment"]
        self.assertEqual(projected_run["system_status"], "unavailable")
        self.assertEqual(
            projected_run["ai_final_assessment"]["availability"], "not_evaluated"
        )
        self.assertNotIn("analyzer", projected_run["ai_final_assessment"])


if __name__ == "__main__":
    unittest.main()
