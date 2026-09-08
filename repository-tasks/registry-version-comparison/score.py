#!/usr/bin/env python3
"""Validate and aggregate independent observations of the Registry task metrics."""
import argparse
import hashlib
import json
import math
from pathlib import Path


def load_catalog(path):
    raw = path.read_bytes()
    catalog = json.loads(raw)
    if catalog.get("schema_version") != 1:
        raise ValueError("Unsupported metric catalog version")
    tests = catalog["tests"]
    if [test["id"] for test in tests] != [1, 2, 3, 4]:
        raise ValueError("Catalog must define tests 1 through 4 in order")
    ids = set()
    for test in tests:
        for metric in test["metrics"]:
            if not isinstance(metric["id"], str) or not metric["id"].strip():
                raise ValueError("Metric IDs must be nonempty strings")
            if metric["id"] in ids:
                raise ValueError(f"Duplicate metric: {metric['id']}")
            ids.add(metric["id"])
            if not isinstance(metric["question"], str) or metric["question"].count("?") != 1:
                raise ValueError(f"Metric must contain one question: {metric['id']}")
            if not isinstance(metric["expected"], str) or not metric["expected"].strip() or not isinstance(metric["evidence"], list) or not metric["evidence"] or any(not isinstance(e, str) or not e.strip() for e in metric["evidence"]):
                raise ValueError("Expected result and required evidence must be explicit")
            if type(metric["weight"]) not in (int, float) or not math.isfinite(metric["weight"]) or metric["weight"] <= 0:
                raise ValueError("Metric weights must be finite and positive")
            if metric["measurement"] not in ("binary", "ratio"):
                raise ValueError("Unsupported metric measurement")
            if metric["measurement"] == "ratio" and (not metric.get("numerator") or not metric.get("denominator")):
                raise ValueError("Ratio metrics require explicit numerator and denominator definitions")
        if not math.isclose(sum(m["weight"] for m in test["metrics"]), 100, abs_tol=1e-9, rel_tol=0):
            raise ValueError(f"Test {test['id']} weights must total 100")
    return catalog, hashlib.sha256(raw).hexdigest()


def template(catalog, identity):
    return {"schema_version": 1, "catalog_sha256": identity, "observations": [
        {"id": metric["id"], "status": "unavailable", "reason": "awaiting_independent_validation"}
        for test in catalog["tests"] for metric in test["metrics"]
    ]}


def score(catalog, identity, observations, evidence_root):
    if observations.get("schema_version") != 1 or observations.get("catalog_sha256") != identity:
        raise ValueError("Observations must match the frozen catalog version and checksum")
    known = {m["id"] for t in catalog["tests"] for m in t["metrics"]}
    by_id = {}
    for item in observations["observations"]:
        if item["id"] not in known or item["id"] in by_id:
            raise ValueError(f"Unknown or duplicate observation: {item['id']}")
        by_id[item["id"]] = item
    tests = []
    for test in catalog["tests"]:
        results = []
        for metric in test["metrics"]:
            item = by_id.get(metric["id"], {"status": "unavailable", "reason": "observation_missing"})
            status = item["status"]
            value = None
            if status not in ("measured", "unavailable", "not_applicable"):
                raise ValueError(f"Unknown observation status: {status}")
            if status == "unavailable":
                if not isinstance(item.get("reason"), str) or not item["reason"].strip() or any(key in item for key in ("value", "numerator", "denominator")):
                    raise ValueError("Unavailable observations require a reason and no measurement")
            else:
                evidence = item.get("evidence")
                if not isinstance(evidence, list) or not evidence:
                    raise ValueError(f"Observation requires evidence files: {metric['id']}")
                for reference in evidence:
                    if not isinstance(reference, str) or Path(reference).is_absolute():
                        raise ValueError("Evidence paths must be relative to the observations file")
                    path = (evidence_root / reference).resolve()
                    if not path.is_relative_to(evidence_root.resolve()) or not path.is_file():
                        raise ValueError(f"Evidence must be an existing file inside the observation bundle: {reference}")
                if metric["measurement"] == "binary":
                    if status != "measured" or type(item.get("value")) is not int or item["value"] not in (0, 1):
                        raise ValueError("Binary observations require measured integer 0 or 1")
                    if "numerator" in item or "denominator" in item:
                        raise ValueError("Binary observations must not include ratio counts")
                    value = item["value"]
                else:
                    numerator, denominator = item.get("numerator"), item.get("denominator")
                    if type(numerator) is not int or type(denominator) is not int or not 0 <= numerator <= denominator or "value" in item:
                        raise ValueError("Ratios require integer counts with 0 <= numerator <= denominator")
                    if denominator == 0:
                        if status != "not_applicable" or not isinstance(item.get("reason"), str) or not item["reason"].strip():
                            raise ValueError("Zero denominators require not_applicable with a reason")
                    elif status != "measured":
                        raise ValueError("Nonzero denominators require measured status")
                    else:
                        value = numerator / denominator
            results.append({"id": metric["id"], "question": metric["question"],
                            "status": status, "value": value, "weight": metric["weight"],
                            "earned_points": None if value is None else value * metric["weight"],
                            "observation": item})
        complete = all(m["status"] == "measured" for m in results)
        earned = sum(m["earned_points"] for m in results if m["earned_points"] is not None)
        tests.append({"test": test["id"], "score": earned if complete else None,
                      "earned_points": earned,
                      "measured_weight": sum(m["weight"] for m in results if m["status"] == "measured"),
                      "scheduled_weight": 100, "metrics": results})
    return {"schema_version": 1, "catalog_sha256": identity, "tests": tests,
            "mean_score": sum(t["score"] for t in tests) / 4 if all(t["score"] is not None for t in tests) else None}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["template", "score"])
    parser.add_argument("--catalog", type=Path, default=Path(__file__).with_name("metrics.json"))
    parser.add_argument("--observations", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.action == "score" and args.observations is None:
        parser.error("score requires --observations")
    catalog, identity = load_catalog(args.catalog)
    result = template(catalog, identity) if args.action == "template" else score(
        catalog, identity, json.loads(args.observations.read_text()), args.observations.resolve().parent)
    # Scores and templates are new artifacts; never overwrite reviewed observations.
    with args.output.open("x") as output:
        json.dump(result, output, indent=2, allow_nan=False)
        output.write("\n")
    print(json.dumps({"output": str(args.output), "catalog_sha256": identity}))


if __name__ == "__main__":
    main()
