"""Small configuration cache used by the benchmark fixture."""

from src.config_loader import merge_config


class ConfigCache:
    def __init__(self):
        self._values = {}

    def resolve(self, name, file_values, env_values, cli_values):
        # BUG: distinct CLI overrides share one cached entry.
        key = (name, tuple(sorted(env_values.items())))
        if key not in self._values:
            self._values[key] = merge_config(file_values, env_values, cli_values)
        return dict(self._values[key])
