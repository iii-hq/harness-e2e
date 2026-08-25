import unittest

from src.client import PROFILE_ROUTE, REQUIRED_FIELDS


class LegacyClientContractTest(unittest.TestCase):
    def test_declares_unversioned_legacy_alias(self):
        self.assertEqual(PROFILE_ROUTE, "/profile")
        self.assertEqual(REQUIRED_FIELDS, {"name"})


if __name__ == "__main__":
    unittest.main()
