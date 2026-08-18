import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parents[1]))
from src.config_loader import load_settings

result = load_settings({"APP_TIMEOUT": "5"}, {"timeout": 20})
if result != {"timeout": 5}:
    print("environment_precedence:FAIL")
    raise SystemExit(1)
print("environment_precedence:PASS")
