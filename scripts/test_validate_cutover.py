import copy
import datetime as dt
import json
import pathlib
import unittest

from validate_cutover import CutoverError, evaluate_cutover, validate_policy


SHA_A = "a" * 40
SHA_B = "b" * 40
DIGEST = "c" * 64
NOW = dt.datetime(2026, 8, 12, tzinfo=dt.timezone.utc)


def policy():
    return {
        "schema_version": 1,
        "lane_order": ["pull_request", "main", "daily", "release"],
        "canonical": {
            "entrypoint": "e2e::run",
            "results_schema_version": 2,
            "archive_scheme": "iii-storage",
            "required_identity_fields": [
                "execution_id",
                "lane",
                "subject_revision",
                "e2e_revision",
            ],
        },
        "minimum_shadow_windows": 3,
        "rollback_window_days": 14,
        "legacy": {
            "workflow": ".github/workflows/_harness-e2e.yml",
            "immutable_tag_prefix": "refs/tags/e2e-legacy-",
        },
    }


def lane(name, mode="new_path"):
    return {
        "lane": name,
        "mode": mode,
        "entrypoint": "e2e::run",
        "results_schema_version": 2,
        "identity": {
            "execution_id": f"execution-{name}",
            "lane": name,
            "subject_revision": SHA_A,
            "e2e_revision": SHA_B,
        },
        "archive": {
            "uri": f"iii-storage://e2e-longitudinal/{name}?sha256={DIGEST}",
            "sha256": DIGEST,
            "availability": "available",
        },
    }


def evidence():
    return {
        "schema_version": 1,
        "subject_revision": SHA_A,
        "e2e_revision": SHA_B,
        "lanes": [lane(name) for name in policy()["lane_order"]],
        "shadow_windows": [
            {
                "sequence": sequence,
                "subject_revision": SHA_A,
                "e2e_revision": SHA_B,
                "seed": seed,
                "completed_at": f"2026-07-{sequence:02d}T00:00:00Z",
                "equivalent": True,
            }
            for sequence, seed in enumerate((8008, 8009, 8010), start=1)
        ],
        "consumers": [
            {
                "id": "release-gate",
                "active": True,
                "entrypoint": "e2e::run",
                "results_schema_version": 2,
            }
        ],
        "rollback": {
            "legacy_ref": "refs/tags/e2e-legacy-2026-08",
            "legacy_revision": "d" * 40,
            "window_started_at": "2026-07-01T00:00:00Z",
            "window_ends_at": "2026-07-15T00:00:00Z",
            "drill": {
                "executed_at": "2026-07-10T00:00:00Z",
                "succeeded": True,
                "restored_entrypoint": ".github/workflows/_harness-e2e.yml",
                "evidence_sha256": "e" * 64,
            },
        },
    }


class CutoverValidationTests(unittest.TestCase):
    def test_checked_in_policy_is_valid(self):
        path = pathlib.Path(__file__).parent.parent / "policies" / "cutover-v1.json"
        validate_policy(json.loads(path.read_text(encoding="utf-8")))

    def test_policy_requires_ordered_lanes_and_iii_contract(self):
        validate_policy(policy())
        invalid = policy()
        invalid["lane_order"] = list(reversed(invalid["lane_order"]))
        with self.assertRaises(CutoverError):
            validate_policy(invalid)

    def test_release_accepts_only_canonical_immutable_evidence(self):
        result = evaluate_cutover(policy(), evidence(), "release", NOW)
        self.assertTrue(result["eligible"])
        self.assertEqual(result["equivalent_shadow_windows"], 3)

        invalid = evidence()
        invalid["lanes"][3]["archive"]["availability"] = "expired"
        result = evaluate_cutover(policy(), invalid, "release", NOW)
        self.assertFalse(result["eligible"])
        self.assertIn("lane release archive is not available", result["reasons"])

    def test_lanes_cannot_cut_over_out_of_order(self):
        value = evidence()
        value["lanes"][1]["mode"] = "shadow"
        result = evaluate_cutover(policy(), value, "daily", NOW)
        self.assertFalse(result["eligible"])
        self.assertIn("lane main has not cut over to the new path", result["reasons"])

    def test_shadow_parity_must_be_consecutive(self):
        value = evidence()
        value["shadow_windows"].append(
            {
                "sequence": 4,
                "subject_revision": SHA_A,
                "e2e_revision": SHA_B,
                "seed": 8011,
                "completed_at": "2026-07-04T00:00:00Z",
                "equivalent": False,
            }
        )
        result = evaluate_cutover(policy(), value, "pull_request", NOW)
        self.assertFalse(result["eligible"])
        self.assertIn(
            "only 0 consecutive equivalent shadow windows; 3 required",
            result["reasons"],
        )

    def test_legacy_removal_requires_elapsed_window_drill_and_no_consumers(self):
        result = evaluate_cutover(policy(), evidence(), "legacy_removal", NOW)
        self.assertTrue(result["eligible"])

        invalid = evidence()
        invalid["consumers"].append(
            {
                "id": "old-daily",
                "active": True,
                "entrypoint": "legacy-binary",
                "results_schema_version": 1,
            }
        )
        invalid["rollback"]["drill"]["succeeded"] = False
        result = evaluate_cutover(policy(), invalid, "legacy_removal", NOW)
        self.assertFalse(result["eligible"])
        self.assertTrue(any("old-daily" in reason for reason in result["reasons"]))
        self.assertIn("rollback simulation has not succeeded", result["reasons"])

    def test_legacy_removal_rejects_an_open_rollback_window(self):
        invalid = copy.deepcopy(evidence())
        invalid["rollback"]["window_started_at"] = "2026-08-01T00:00:00Z"
        invalid["rollback"]["window_ends_at"] = "2026-08-15T00:00:00Z"
        result = evaluate_cutover(policy(), invalid, "legacy_removal", NOW)
        self.assertFalse(result["eligible"])
        self.assertIn("rollback window has not elapsed", result["reasons"])


if __name__ == "__main__":
    unittest.main()
