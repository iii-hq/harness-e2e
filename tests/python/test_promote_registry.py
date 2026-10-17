import importlib.util
import json
import tempfile
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


    @patch.object(promote_registry, "upload_file")
    @patch.object(promote_registry, "request_json")
    def test_publish_hosts_the_archives_in_a_registry_release(self, request, upload):
        assets = [{"name": "harness-e2e-x86_64-unknown-linux-gnu.tar.gz", "target": "x86_64-unknown-linux-gnu",
                   "sha256": "a" * 64, "size_bytes": 3}]
        headers = {"Content-Type": "application/gzip", "Content-Length": "3"}
        request.side_effect = [
            (410, {"code": "release_expired"}),
            (200, {"release": {"id": "rel-1"}, "resumed": False}),
            (200, {"assets": [{**assets[0], "upload": {"url": "https://staging.example/put", "headers": headers}}]}),
            (200, {"version": "0.3.0", "changed": True}),
        ]
        payload = {"worker_name": "harness-e2e", "version": "0.3.0", "type": "binary", "tag": "next"}

        result = promote_registry.publish("https://registry.example/", "key", payload, assets, Path("dist"))

        self.assertEqual(result["release_id"], "rel-1")
        self.assertTrue(result["changed"])
        start, retry, register, publish = request.call_args_list
        self.assertEqual(start.args, ("POST", "https://registry.example/w/harness-e2e/releases",
                                      {"version": "0.3.0", "type": "binary"}))
        self.assertEqual(retry.args, start.args)
        self.assertEqual(register.args[1], "https://registry.example/w/harness-e2e/releases/rel-1/assets")
        self.assertEqual(register.args[2], {"assets": assets})
        upload.assert_called_once_with("https://staging.example/put", headers,
                                       Path("dist/harness-e2e-x86_64-unknown-linux-gnu.tar.gz"))
        # The Registry records CDN URLs only when the payload names no artifact URL.
        self.assertEqual(publish.args[:2], ("POST", "https://registry.example/publish"))
        self.assertEqual(publish.args[2], {**payload, "release_id": "rel-1"})
        self.assertEqual({call.kwargs["api_key"] for call in request.call_args_list}, {"key"})

    @patch.object(promote_registry, "upload_file")
    @patch.object(promote_registry, "request_json")
    def test_publish_replay_requires_next_to_resolve_to_the_version(self, request, upload):
        payload = {"worker_name": "harness-e2e", "version": "0.3.0", "type": "binary"}
        request.side_effect = [
            (409, {"error": "exists", "code": "version_already_published"}),
            (200, {"root": {"version": "0.3.0"}}),
        ]
        self.assertFalse(promote_registry.publish("https://r.example", "key", payload, [], Path("."))["changed"])

        request.side_effect = [
            (409, {"error": "exists", "code": "version_already_published"}),
            (200, {"root": {"version": "0.4.0"}}),
        ]
        with self.assertRaises(promote_registry.RegistryError):
            promote_registry.publish("https://r.example", "key", payload, [], Path("."))
        upload.assert_not_called()

    def test_upload_sends_the_returned_headers_but_its_own_content_length(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "harness-e2e-x86_64-unknown-linux-gnu.tar.gz"
            archive.write_bytes(b"tgz")
            with patch.object(promote_registry.urllib.request, "urlopen") as urlopen:
                promote_registry.upload_file(
                    "https://staging.example/put",
                    {"Content-Type": "application/gzip", "Content-Length": "999"},
                    archive,
                )
        sent = urlopen.call_args.args[0]
        self.assertEqual(sent.get_method(), "PUT")
        self.assertEqual(sent.data, b"tgz")
        self.assertEqual(sent.get_header("Content-type"), "application/gzip")
        self.assertIsNone(sent.get_header("Content-length"))


if __name__ == "__main__":
    unittest.main()
