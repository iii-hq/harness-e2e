"""Stable de-duplication used by the benchmark fixture."""


def stable_unique(values):
    """Return the first occurrence of every value while preserving order."""
    result = []
    for value in values:
        if value not in result:
            result.append(value)
    return result
