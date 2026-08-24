import unittest

from src.profile_api import get_profile_v1


class ProfileApiTest(unittest.TestCase):
    def test_v1_shape(self):
        self.assertEqual(get_profile_v1("7"), {"name": "user-7"})


if __name__ == "__main__":
    unittest.main()
