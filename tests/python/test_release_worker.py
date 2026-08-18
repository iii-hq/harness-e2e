import unittest

from scripts.release_worker import TARGETS, TAG_RE, schema_is_typed


class ReleaseWorkerTest(unittest.TestCase):
    def test_release_tag_is_stable_and_namespaced(self):
        self.assertIsNotNone(TAG_RE.fullmatch("harness-e2e/v1.2.3"))
        for invalid in (
            "v1.2.3",
            "harness-e2e/1.2.3",
            "harness-e2e/v1.2.3-rc.1",
            "harness-e2e/v01.2.3",
        ):
            self.assertIsNone(TAG_RE.fullmatch(invalid))

    def test_release_matrix_matches_registry_binary_contract(self):
        self.assertEqual(len(TARGETS), 9)
        self.assertIn("x86_64-unknown-linux-gnu", TARGETS)
        self.assertEqual(len(TARGETS), len(set(TARGETS)))

    def test_interface_schema_must_define_a_shape(self):
        self.assertTrue(schema_is_typed({"type": "object", "properties": {}}))
        self.assertTrue(schema_is_typed({"$ref": "#/definitions/Request"}))
        self.assertFalse(schema_is_typed({}))
        self.assertFalse(schema_is_typed({"title": "AnyValue"}))


if __name__ == "__main__":
    unittest.main()
