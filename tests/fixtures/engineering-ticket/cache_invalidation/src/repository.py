class Repository:
    def __init__(self):
        self._values = {}

    def write(self, key, value):
        self._values[key] = value

    def read(self, key):
        return self._values[key]
