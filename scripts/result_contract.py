"""Load the result contract from its authoritative checked-in sources."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def contract_values(root: Path = ROOT) -> dict[str, str | int]:
    manifest = json.loads((root / "config/results-contract.json").read_text(encoding="utf-8"))

    def fingerprint(relative: str) -> str:
        value = json.loads((root / relative).read_text(encoding="utf-8"))
        canonical = json.dumps(
            value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
        ).encode("utf-8")
        return f"sha256:{hashlib.sha256(canonical).hexdigest()}"

    return {
        "RESULTS_SCHEMA_VERSION": manifest["schema_version"],
        "RESULT_CONTRACT_SHA256": fingerprint(manifest["results_schema"]),
        "SCORING_PROFILE_SHA256": fingerprint(manifest["scoring_profile"]),
    }


_VALUES = contract_values()
RESULTS_SCHEMA_VERSION = _VALUES["RESULTS_SCHEMA_VERSION"]
RESULT_CONTRACT_SHA256 = _VALUES["RESULT_CONTRACT_SHA256"]
SCORING_PROFILE_SHA256 = _VALUES["SCORING_PROFILE_SHA256"]
