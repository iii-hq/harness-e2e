import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parents[1]))
from src.repository import Repository
from src.service import DerivedService


class CachePublicTest(unittest.TestCase):
    def test_write_invalidates_derived_value(self):
        repository = Repository()
        service = DerivedService(repository)
        service.write("a", 2)
        self.assertEqual(service.derived("a"), 4)
        service.write("a", 7)
        self.assertEqual(service.derived("a"), 14)


if __name__ == "__main__":
    unittest.main()
