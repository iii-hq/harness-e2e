import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parents[1]))
from src.pagination import page

result = page(list(range(20)), 10, 10)
if result != {"items": list(range(10, 20)), "next_cursor": None}:
    print("pagination_exact_page:FAIL")
    raise SystemExit(1)
print("pagination_exact_page:PASS")
