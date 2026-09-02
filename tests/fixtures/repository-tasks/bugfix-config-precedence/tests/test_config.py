import unittest

from src.config_cache import ConfigCache
from src.config_loader import merge_config


class ConfigPrecedenceTests(unittest.TestCase):
    def test_cli_overrides_environment_and_file(self):
        result = merge_config(
            {"region": "file", "timeout": 10},
            {"region": "env"},
            {"region": "cli"},
        )
        self.assertEqual(result, {"region": "cli", "timeout": 10})

    def test_cache_distinguishes_cli_overrides(self):
        cache = ConfigCache()
        first = cache.resolve("api", {}, {}, {"timeout": 10})
        second = cache.resolve("api", {}, {}, {"timeout": 20})
        self.assertEqual(first["timeout"], 10)
        self.assertEqual(second["timeout"], 20)


if __name__ == "__main__":
    unittest.main()
