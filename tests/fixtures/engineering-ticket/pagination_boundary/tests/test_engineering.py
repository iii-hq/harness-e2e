import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parents[1]))
from src.pagination import page


class PaginationPublicTest(unittest.TestCase):
    def test_exact_page_keeps_final_item(self):
        self.assertEqual(
            page(list(range(20)), 10, 10),
            {"items": list(range(10, 20)), "next_cursor": None},
        )

    def test_partial_page_is_unchanged(self):
        self.assertEqual(page(list(range(13)), 10, 10)["items"], [10, 11, 12])


if __name__ == "__main__":
    unittest.main()
