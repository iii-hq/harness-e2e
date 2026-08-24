import unittest

from src.deduplicate import stable_unique


class StableUniqueTests(unittest.TestCase):
    def test_preserves_first_occurrence_order(self):
        self.assertEqual(stable_unique([3, 1, 3, 2, 1]), [3, 1, 2])

    def test_handles_empty_input(self):
        self.assertEqual(stable_unique([]), [])

    def test_accepts_an_iterator(self):
        self.assertEqual(stable_unique(iter(["a", "b", "a"])), ["a", "b"])


if __name__ == "__main__":
    unittest.main()
