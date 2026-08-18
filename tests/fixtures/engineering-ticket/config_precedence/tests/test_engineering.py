import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parents[1]))
from src.config_loader import load_settings


class ConfigPublicTest(unittest.TestCase):
    def test_environment_wins_over_file(self):
        self.assertEqual(
            load_settings({"APP_TIMEOUT": "5"}, {"timeout": 20}),
            {"timeout": 5},
        )

    def test_file_wins_over_default(self):
        self.assertEqual(load_settings({}, {"timeout": 20}), {"timeout": 20})


if __name__ == "__main__":
    unittest.main()
