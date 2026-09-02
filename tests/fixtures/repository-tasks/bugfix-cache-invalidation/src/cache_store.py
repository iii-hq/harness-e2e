"""Versioned in-memory cache."""


class CacheStore:
    def __init__(self):
        self._entries = {}

    def put(self, key, value, version):
        self._entries[key] = {"value": value, "version": version, "stale": False}

    def invalidate(self, key, version):
        entry = self._entries.get(key)
        if entry is None:
            return False
        # BUG: an older invalidation marks a newer value as stale.
        entry["stale"] = True
        return True

    def get(self, key):
        entry = self._entries.get(key)
        if entry is None or entry["stale"]:
            return None
        return entry["value"]
