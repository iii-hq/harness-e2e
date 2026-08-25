import importlib.util
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).parents[2] / "scripts" / "promote_registry.py"
SPEC = importlib.util.spec_from_file_location("promote_registry", SCRIPT)
assert SPEC and SPEC.loader
promote_registry = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(promote_registry)


class PromoteRegistryTests(unittest.TestCase):
    def test_promotion_payload_omits_missing_latest_precondition(self):
        self.assertEqual(
            promote_registry.promotion_payload("0.3.0-experimental", None),
            {"version": "0.3.0-experimental", "expected_tag": "next"},
        )

    @patch.object(promote_registry, "request_json")
    def test_promote_uses_raw_latest_pointer_and_verifies_resolution(self, request):
        request.side_effect = [
            (200, {"versions": []}),
            (200, {"versions": [{"version": "0.3.0-experimental", "tags": ["next"]}]}),
            (200, {"changed": True}),
            (200, {"versions": [{"version": "0.3.0-experimental", "tags": ["latest"]}]}),
            (200, {"root": {"version": "0.3.0-experimental"}}),
        ]

        result = promote_registry.promote(
            "https://registry.example",
            "secret-that-is-never-printed",
            "harness-e2e",
            "0.3.0-experimental",
            "0.3.0-experimental",
            None,
        )

        self.assertEqual(result["latest"], "0.3.0-experimental")
        self.assertTrue(result["changed"])
        put_call = request.call_args_list[2]
        self.assertEqual(put_call.args[0], "PUT")
        self.assertEqual(
            put_call.args[2],
            {"version": "0.3.0-experimental", "expected_tag": "next"},
        )
        self.assertEqual(put_call.kwargs["api_key"], "secret-that-is-never-printed")

    @patch.object(promote_registry, "request_json")
    def test_idempotent_promotion_does_not_write(self, request):
        request.side_effect = [
            (200, {"versions": [{"version": "0.3.0-experimental", "tags": ["latest"]}]}),
            (200, {"versions": [{"version": "0.3.0-experimental", "tags": ["next"]}]}),
            (200, {"root": {"version": "0.3.0-experimental"}}),
        ]

        result = promote_registry.promote(
            "https://registry.example",
            "secret",
            "harness-e2e",
            "0.3.0-experimental",
            "0.3.0-experimental",
            "0.3.0-experimental",
        )

        self.assertFalse(result["changed"])
        self.assertEqual([call.args[0] for call in request.call_args_list], ["GET", "GET", "POST"])

    @patch.object(promote_registry, "request_json")
    def test_candidate_mismatch_fails_before_mutation(self, request):
        request.side_effect = [
            (200, {"versions": []}),
            (200, {"versions": [{"version": "0.2.0-experimental", "tags": ["next"]}]}),
        ]

        with self.assertRaises(promote_registry.RegistryError):
            promote_registry.promote(
                "https://registry.example",
                "secret",
                "harness-e2e",
                "0.3.0-experimental",
                "0.3.0-experimental",
                None,
            )

        self.assertEqual([call.args[0] for call in request.call_args_list], ["GET", "GET"])


if __name__ == "__main__":
    unittest.main()
