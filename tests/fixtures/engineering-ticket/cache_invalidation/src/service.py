class DerivedService:
    def __init__(self, repository):
        self.repository = repository
        self._cache = {}

    def derived(self, key):
        if key not in self._cache:
            self._cache[key] = self.repository.read(key) * 2
        return self._cache[key]

    def write(self, key, value):
        self.repository.write(key, value)
