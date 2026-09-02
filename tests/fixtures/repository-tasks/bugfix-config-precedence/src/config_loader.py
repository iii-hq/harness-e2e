"""Configuration precedence rules."""


def merge_config(file_values, env_values, cli_values):
    """Return a new configuration without mutating any input mapping."""
    merged = dict(file_values)
    merged.update(cli_values)
    # BUG: environment values incorrectly override explicit CLI flags.
    merged.update(env_values)
    return merged
