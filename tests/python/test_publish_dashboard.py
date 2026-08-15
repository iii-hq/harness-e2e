from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from publish_harness_e2e_dashboard import build_static_test_catalog


def report(revision: str, scores: list[int]) -> dict:
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
                "runs": [
                    {
                        "status": "passed",
                        "score": score,
                        "wall_time_ms": 1_000 + score,
                        "cost": {"total_usd": score / 1_000},
                        "metrics": {
                            "totals": {
                                "input_tokens": score,
                                "output_tokens": 10,
                            }
                        },
                    }
                    for score in scores
                ],
            }
        ],
    }


class PublishDashboardTests(unittest.TestCase):
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
                (site / detail_path).write_text(
                    json.dumps(
                        {
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
                    )
                )
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
            shard_path = site / row["shards"]["3"].removeprefix("./")
            shard = json.loads(shard_path.read_text())
            self.assertEqual(len(shard["observations"]), 2)
            self.assertNotIn("runs", shard["observations"][0])


if __name__ == "__main__":
    unittest.main()
