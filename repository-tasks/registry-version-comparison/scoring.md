# Atomic metric scoring

`metrics.json` is the versioned definition of the four tests' validation metrics. Each metric answers one question and declares an expected result, evidence requirements, measurement type, and positive weight. Each test's weights total 100. There is no additional score for broad categories.

`registry-tests` freezes this catalog and `score.py` under `controller-assets/` before starting subject sessions. It also creates `validation-observations.json`, initially containing unavailable observations. Supply `--metrics-config /absolute/custom-metrics.json` to choose definitions or weights before a run. Invalid catalogs are rejected before any model call. Observations bind to the exact frozen catalog bytes through SHA-256.

## Record observations

An independent validator or human reviewer fills the observations from execution artifacts. The scorer validates their structure and evidence-file existence; it does not establish whether the cited evidence proves a claim. This change supplies metric definitions and aggregation, not automated product probes, planning judges, or a known-defect suite. Subject statements and screenshots alone must not be treated as proof of correctness.

A measured binary observation uses integer 0 or 1:

```json
{
  "id": "implementation.same_version",
  "status": "measured",
  "value": 1,
  "evidence": ["test-2/output-validation/same-version-response.json"]
}
```

Evidence paths must name existing files inside the observations bundle, relative to the observations file. Absolute paths and links escaping the bundle are rejected. Keep the top-level `schema_version` and `catalog_sha256` fields from the generated template. Each observation ID must exist in that catalog and occur at most once. Metrics retain their own distinct observations even when they cite the same raw artifact.

Ratios use integer `numerator` and `denominator` counts instead of `value`. Counts must follow the metric's declared population and satisfy `0 <= numerator <= denominator`. The scorer computes the ratio; callers cannot submit a different derived value.

- `measured`: sufficient independent evidence supports a binary value or a ratio with a positive denominator.
- `unavailable`: the measurement could not be established; include a reason and omit values/counts. Missing observations also become unavailable.
- `not_applicable`: a ratio has a confirmed zero denominator; provide zero counts, evidence, and a reason. This does not earn full credit.

A required check omitted by the subject earns zero when independent evidence establishes that omission. A controller failure that prevents measurement remains unavailable. Do not treat all failures to start identically: a broken delivered Dockerfile is a measurable build failure, while an unavailable executor daemon prevents measurement.

For Test 4, recall requires a known defect inventory. Missing ground truth means unavailable, not a zero-denominator observation. Count unique defects, not duplicate descriptions of the same defect. Precision is confirmed reported defects divided by reported defects. Reproducibility requires independently reproducing the defect; complete reproduction instructions alone do not satisfy it. Freeze case IDs and the known-defect inventory outside the subject workspace before validation, and retain those inventories with the evidence.

## Aggregate

```bash
python3 /absolute/run/controller-assets/score.py score \
  --observations /absolute/run/validation-observations.json \
  --output /absolute/run/validation-scores.json
```

The scorer defaults to the catalog beside its own script. To assess older evidence with an explicitly selected catalog, pass `--catalog`; its checksum must still match the observations. New score outputs never overwrite existing artifacts.

For each measured metric, `earned_points = weight × value`. A complete test score is the sum of its earned points, on a 0–100 scale. If any metric is unavailable or not applicable, the test score is `null`; the report still shows earned points and measured weight against the scheduled 100. Missing weight is never redistributed. A not-applicable metric therefore leaves the current profile incomplete; changing applicability or weights requires a new profile before a subsequent run.

The overall `mean_score` is the arithmetic mean of all four complete test scores. It remains `null` when any test is incomplete. A one-test execution can have its own complete score while the four-test mean remains unavailable. Compare complete scores only under the same catalog identity and compatible execution inputs; keep runtime, tokens, and cost separate.

## Check the scorer without a model

```bash
python3 -m unittest discover -s tests/python -p test_registry_scoring.py -v
python3 repository-tasks/registry-version-comparison/score.py template \
  --output /absolute/new/observations.json
```

Template generation and score aggregation require only Python's standard library. They do not start Docker or call a model.
