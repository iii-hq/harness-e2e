import unittest

from src.client import PROFILE_ROUTE, REQUIRED_FIELDS


class ClientContractTest(unittest.TestCase):
    def test_declares_v1_contract(self):
        self.assertEqual(PROFILE_ROUTE, "/v1/profile")
        self.assertEqual(REQUIRED_FIELDS, {"name"})


if __name__ == "__main__":
    unittest.main()
