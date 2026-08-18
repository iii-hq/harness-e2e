import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parents[1]))
from src.repository import Repository
from src.service import DerivedService

repository = Repository()
service = DerivedService(repository)
service.write("a", 2)
service.derived("a")
service.write("a", 7)
if service.derived("a") != 14:
    print("write_invalidates_cache:FAIL")
    raise SystemExit(1)
print("write_invalidates_cache:PASS")
