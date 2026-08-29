import unittest

from scripts import publish_registry


API = "https://registry.test"
WORKER = "harness-e2e"
VERSION = "0.5.0-experimental"


def payload():
    return {
        "worker_name": WORKER,
        "version": VERSION,
        "type": "binary",
        "tag": "next",
        "description": "Harness E2E",
        "license": "Apache-2.0",
        "tags": ["e2e"],
        "dependencies": [{"name": "state", "version": "^0.22.2"}],
        "config": {},
        "experimental": True,
        "readme": "# Harness E2E\n",
        "repo": "https://github.com/iii-hq/harness-e2e",
        "functions": [],
        "triggers": [],
        "binaries": {"x86_64-unknown-linux-gnu": {"url": "https://example.test/a.tgz", "sha256": "a" * 64}},
    }


def resolution(version=VERSION):
    return {
        "root": {"name": WORKER, "version": version},
        "graph": [{
            "name": WORKER,
            "version": version,
            "type": "binary",
            "dependencies": {"state": "^0.22.2"},
            "binaries": payload()["binaries"],
        }],
        "edges": [],
    }


def exact_readback(method, url, body=None, **_kwargs):
    if method == "POST" and url.endswith("/resolve"):
        return 200, resolution()
    if method == "GET" and url.endswith("/versions"):
        return 200, {"versions": [{"version": VERSION, "tags": ["next"]}]}
    raise AssertionError((method, url, body))


class PublishRegistryTests(unittest.TestCase):
    def setUp(self):
        self.original_request = publish_registry.request_json
        self.original_sleep = publish_registry.time.sleep
        publish_registry.time.sleep = lambda _seconds: None

    def tearDown(self):
        publish_registry.request_json = self.original_request
        publish_registry.time.sleep = self.original_sleep

    def test_fresh_publish_requires_version_artifact_and_channel_readback(self):
        calls = []

        def request(method, url, body=None, **kwargs):
            calls.append((method, url))
            if url.endswith("/publish"):
                self.assertEqual(body, payload())
                self.assertEqual(kwargs["api_key"], "secret")
                return 200, {"version": {"version": VERSION}}
            return exact_readback(method, url, body, **kwargs)

        publish_registry.request_json = request
        result = publish_registry.publish(API, "secret", WORKER, VERSION, payload())
        self.assertEqual(result["state"], "changed")
        self.assertEqual([method for method, _url in calls], ["POST", "POST", "POST", "GET"])

    def test_timeout_after_effect_is_recovered_without_repeating_post(self):
        posts = 0

        def request(method, url, body=None, **kwargs):
            nonlocal posts
            if url.endswith("/publish"):
                posts += 1
                raise publish_registry.TransportError("timeout")
            return exact_readback(method, url, body, **kwargs)

        publish_registry.request_json = request
        result = publish_registry.publish(API, "secret", WORKER, VERSION, payload())
        self.assertEqual(result["state"], "recovered")
        self.assertEqual(posts, 1)

    def test_409_fails_when_next_does_not_identify_exact_version(self):
        def request(method, url, body=None, **kwargs):
            if url.endswith("/publish"):
                return 409, {"error": "exists"}
            if method == "POST" and body == {"worker": WORKER, "version": "next"}:
                return 200, resolution("0.4.0")
            return exact_readback(method, url, body, **kwargs)

        publish_registry.request_json = request
        with self.assertRaisesRegex(publish_registry.PublicationError, "divergent"):
            publish_registry.publish(API, "secret", WORKER, VERSION, payload())

    def test_5xx_retries_only_after_exact_version_is_proven_absent(self):
        posts = 0

        def request(method, url, body=None, **_kwargs):
            nonlocal posts
            if url.endswith("/publish"):
                posts += 1
                return 503, {}
            if method == "POST" and body == {"worker": WORKER, "version": VERSION}:
                return 422, {"error": {"code": "version_not_found"}}
            return 404, {}

        publish_registry.request_json = request
        with self.assertRaisesRegex(publish_registry.PublicationError, "after 3 attempts"):
            publish_registry.publish(API, "secret", WORKER, VERSION, payload())
        self.assertEqual(posts, 3)


if __name__ == "__main__":
    unittest.main()
