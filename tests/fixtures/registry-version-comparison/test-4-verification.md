# Test 4 — Verify the delivered feature in a real environment

The `registry/` workspace contains the Test 2 delivery applied to the pinned starting commit. Read `inputs/requirements.md` and `inputs/environment.json`. Treat the running API, frontend, and PostgreSQL instance as the system under test.

Exercise the required flows through the real API and browser: additive, breaking, reverse, same-version, unavailable metadata, invalid/missing versions, worker scoping, shareable URL and browser history, error/stale-state behavior, expandable values, accessibility basics, local downloads, and preserved existing Registry flows. Record expected and observed behavior with reproducible steps, URLs, responses or logs, and actual 1440 × 1000 screenshots. Do not generate mock images.

Do not edit product source or fix defects. Do not change requirements to make the delivery pass. Write `output/report.md` in English, classify each check as observed pass, observed failure, or blocked with evidence, and place screenshots and raw evidence under `output/`. Do not score or rank the implementation.


Also write `output/checks.json` with one entry per required check ID:

```json
{
  "checks": [
    {
      "id": "implementation.same_version",
      "status": "pass",
      "command": "python verify-same-version.py",
      "steps": "GET /w/orders-worker/compare/1.0.0...1.0.0; verify HTTP 200 and changes=[]",
      "evidence": ["execution-evidence/<receipt_id>/implementation.same_version.json"]
    }
  ]
}
```

Each scenario-tool call receives a private `HARNESS_EVIDENCE_DIR`. Write one JSON proof per check there. The tool accepts at most 64 regular files, 256 total directory entries, 1 MiB per file and 8 MiB total; directories and files must not be symbolic links. It collects a bounded snapshot and materializes those exact bytes at controller-owned `execution-evidence/<receipt_id>/` paths, then returns their final paths, sizes, and SHA-256 values in `artifacts`; use those returned paths in `checks.json`. Files written directly to `output/` are deliverables but are not execution evidence.

Use `pass`, `fail`, or `blocked` for status. The `command` must exactly match the scenario-tool call that produced the cited artifact; a non-zero exit is valid evidence of an observed failure. Record expected and observed results in non-empty `steps`. Report each required ID exactly once, and give each check its own proof file with this shape:

```json
{
  "check_id": "implementation.same_version",
  "status": "pass",
  "assertions": [
    {
      "id": "http_status",
      "passed": true,
      "expected": {"status": 200},
      "observed": {"status": 200}
    }
  ]
}
```

Assertions must have unique IDs and contain `expected` and `observed` JSON values. Empty arrays, nulls, strings, and other JSON values are valid. Equality is exact and type-sensitive: a passing assertion has equal values, while a failing assertion has different values. Every assertion in a `pass` proof passes, while a `fail` proof contains at least one failing assertion. `checks.json`, `report.md`, screenshots, and free-form text do not count as these per-check proofs. A blocked check is not an executed check.

If a tool call runs a batch that produces several proof files, multiple checks may reference that same complete command. Copy its entire `command` argument verbatim and cite the separate returned artifact for each check.

Required check IDs and questions:

- `implementation.same_version`: Does comparing a version with itself return no changes?
- `implementation.function_removal`: Does the API identify a removed function?
- `implementation.required_impact`: Does a newly required input property receive the prescribed impact?
- `implementation.required_order`: Does changing only schema required-array order preserve equality?
- `implementation.enum_order`: Does changing only schema enum-array order preserve equality?
- `implementation.config_array_order`: Does changing configuration-array order produce a difference?
- `implementation.missing_metadata`: Does absent metadata produce an unavailable notice?
- `implementation.worker_lookup`: Is a version resolved only within the requested worker?
- `implementation.reverse_kinds`: Does reversal swap addition and removal kinds?
- `implementation.reverse_values`: Does reversal swap before and after values?
- `implementation.reverse_impact`: Does reversal recalculate impact?
- `implementation.shared_url`: Does reopening a shared URL restore the selected pair?
- `implementation.stale_results`: Does a failed request avoid displaying the previous result as current?
- `implementation.invalid_version`: Does invalid SemVer return the prescribed error?
- `implementation.missing_worker`: Does a missing worker return the prescribed error?
- `implementation.missing_version`: Does a missing version return the prescribed error?
- `implementation.history`: Does Changelog history show the seeded releases?
- `implementation.expanded_detail`: Can a user expand the timeout change detail?
- `implementation.keyboard_selectors`: Can the version selectors be operated using the keyboard?
- `implementation.versions_regression`: Does the existing Versions tab retain its baseline behavior?
- `implementation.readme_regression`: Does the existing README view retain its baseline behavior?
- `implementation.api_reference_regression`: Does the existing API reference retain its baseline behavior?
- `implementation.download_regression`: Does the existing download flow retain its baseline behavior?
