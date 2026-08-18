import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parents[1]))
from src.serialization import encode_event

result = encode_event({"id": "e1", "name": "created", "trace_id": "t1"})
if result.get("trace_id") != "t1":
    print("optional_trace_id:FAIL")
    raise SystemExit(1)
print("optional_trace_id:PASS")
