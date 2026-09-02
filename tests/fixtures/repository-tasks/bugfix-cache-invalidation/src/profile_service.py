"""Profile service backed by CacheStore."""


class ProfileService:
    def __init__(self, store, loader):
        self.store = store
        self.loader = loader

    def profile(self, user_id, version):
        cached = self.store.get(user_id)
        if cached is not None:
            return cached
        value = self.loader(user_id, version)
        self.store.put(user_id, value, version)
        return value

    def on_profile_changed(self, user_id, event_version):
        # BUG: the event version is discarded, so the store cannot reject stale events.
        return self.store.invalidate(user_id, 0)
