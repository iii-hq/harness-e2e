import unittest

from src.cache_store import CacheStore
from src.profile_service import ProfileService


class CacheInvalidationTests(unittest.TestCase):
    def test_stale_event_does_not_invalidate_newer_entry(self):
        store = CacheStore()
        store.put("u1", {"name": "new"}, version=8)
        service = ProfileService(store, lambda _user, _version: {"name": "loaded"})
        self.assertFalse(service.on_profile_changed("u1", event_version=7))
        self.assertEqual(service.profile("u1", version=8), {"name": "new"})

    def test_matching_event_invalidates_entry(self):
        store = CacheStore()
        store.put("u1", {"name": "old"}, version=8)
        service = ProfileService(store, lambda _user, version: {"name": f"v{version}"})
        self.assertTrue(service.on_profile_changed("u1", event_version=8))
        self.assertEqual(service.profile("u1", version=9), {"name": "v9"})


if __name__ == "__main__":
    unittest.main()
