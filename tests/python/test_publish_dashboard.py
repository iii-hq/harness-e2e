from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from publish_harness_e2e_dashboard import (
    _assessment_profile_sha256,
    _assessment_summary,
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
                    "kind": "required_check",
                    "policy": "hard_gate",
                    "dimension": "structural_integrity",
                    "outcome": "passed",
                    "score": {"awarded": run["score"], "possible": 100},
                    "summary": "Correct result",
                    "evidence": [
                        {
                            "artifact_id": "transcript",
                            "artifact_sha256": "sha256:evidence",
                        }
                    ],
                }
            ],
            "assets": [
                {
                    "asset_id": "artifact",
                    "outcome": "valid",
                    "summary": "Asset matches the declared contract",
                    "evidence": [
                        {
                            "artifact_id": "artifact",
                            "artifact_sha256": "sha256:asset",
                        }
                    ],
                }
            ],
        }
        for run in runs
    ]
    return {
        "execution": {"lane": "daily"},
        "subject": {"provider": "openai", "model": "subject"},
        "judge": {"provider": "openai", "model": "judge"},
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
    def test_publish_writes_json_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            site = Path(directory)
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
            self.assertTrue(all("::" in side_id for side_id in sides))
            self.assertTrue(
                all(
                    side["summary"]["assessment_summary"]["run_count"] > 0
                    for side in sides.values()
                )
            )
            self.assertTrue(
                all("analyzer_profiles" not in side for side in sides.values())
            )
            cohort = catalog["evaluated_versions"]["cohorts"][0]
            self.assertNotIn("judge_protocol", cohort)
            self.assertEqual(cohort["judge_model"], "judge")
            shard_path = site / row["shards"]["3"].removeprefix("./")
            shard = json.loads(shard_path.read_text())
            self.assertEqual(len(shard["observations"]), 2)
            self.assertNotIn("runs", shard["observations"][0])
            self.assertIn("cohort_id", shard["observations"][0])
            self.assertNotIn("analyzer_profile_sha256", shard["observations"][0])
            self.assertTrue(
                shard["observations"][0]["assessment_profile_sha256"].startswith(
                    "sha256:"
                )
            )

    def test_projected_assessment_keeps_only_deterministic_conclusions(self) -> None:
        public = complete_public_detail(
            {
                "reports": [
                    {
                        "available": True,
                        "subject_id": "openai-subject",
                        "scenario_id": "coordination.parallel",
                        "report": report("a" * 40, [100]),
                    }
                ]
            },
            metadata("deterministic-execution"),
        )
        projected_report = public["reports"][0]["report"]
        self.assertEqual(projected_report["assessment_availability"], "available")
        projected_run = projected_report["scenarios"][0]["runs"][0]["assessment"]
        self.assertEqual(projected_run["system_status"], "passed")
        self.assertEqual(
            projected_run["assets"],
            [
                {
                    "asset_id": "artifact",
                    "outcome": "valid",
                    "summary": "Asset matches the declared contract",
                    "evidence": [
                        {
                            "artifact_id": "artifact",
                            "artifact_sha256": "sha256:asset",
                        }
                    ],
                }
            ],
        )
        summary = projected_report["assessment_summary"]
        self.assertEqual(summary["asset_validation_outcomes"]["valid"], 1)
        self.assertEqual(summary["evidence_reference_count"], 2)
        for forbidden in (
            "ai_final_assessment",
            "effective_status",
            "analyzer",
            "confidence",
            "qualitative_assessment",
            "judge_protocol",
        ):
            self.assertFalse(contains_key(public, forbidden), forbidden)

    def test_legacy_detail_without_contract_is_explicitly_unavailable(self) -> None:
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
        self.assertEqual(projected_run["assessments"], [])
        self.assertEqual(projected_run["assets"], [])


if __name__ == "__main__":
    unittest.main()
