import datetime as dt
import json
import pathlib
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from validate_cutover import CutoverError, evaluate_cutover, validate_policy


SHA_A = "a" * 40
SHA_B = "b" * 40
DIGEST = "c" * 64
NOW = dt.datetime(2026, 8, 12, tzinfo=dt.timezone.utc)


def policy():
    return {
        "lane_order": ["pull_request", "main", "daily", "release"],
        "canonical": {
            "entrypoint": "e2e::run",
            "results_file": "results.json",
            "archive_scheme": "iii-storage",
            "required_identity_fields": [
                "execution_id",
                "lane",
                "subject_revision",
                "e2e_revision",
            ],
        },
        "minimum_shadow_windows": 3,
    }


def lane(name, mode="canonical"):
    return {
        "lane": name,
        "mode": mode,
        "entrypoint": "e2e::run",
        "results_file": "results.json",
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
    }


class CutoverValidationTests(unittest.TestCase):
    def test_checked_in_policy_is_valid(self):
        path = ROOT / "config" / "policies" / "cutover.json"
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
        self.assertIn("lane main has not promoted the canonical path", result["reasons"])

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

    def test_versioned_policy_and_evidence_are_rejected(self):
        versioned_policy = policy()
        versioned_policy["schema_version"] = 1
        with self.assertRaises(CutoverError):
            validate_policy(versioned_policy)

        versioned_evidence = evidence()
        versioned_evidence["schema_version"] = 1
        with self.assertRaises(CutoverError):
            evaluate_cutover(policy(), versioned_evidence, "release", NOW)


if __name__ == "__main__":
    unittest.main()
