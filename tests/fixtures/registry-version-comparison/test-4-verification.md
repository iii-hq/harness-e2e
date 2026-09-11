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
      "command_id": "<command_id returned by the scenario tool>",
      "steps": "GET /w/orders-worker/compare/1.0.0...1.0.0; verify HTTP 200 and changes=[]",
      "evidence_record": "output/evidence/same-version.json",
      "evidence": ["output/same-version-response.json"]
    }
  ]
}
```

Use `pass`, `fail`, or `blocked` for status. Use the stable `command_id` returned by the scenario tool. For compatibility with older clients, `command` may instead contain the complete command argument; only leading and trailing whitespace are ignored, while quoted content, internal whitespace, newlines, and redirects must remain unchanged. Record expected/observed results in `steps`. A blocked check is not an executed check. Report each ID once; multiple symptoms of the same check belong in that entry.

Every pass or fail also needs one `evidence_record` JSON file under `output/`:

```json
{
  "check_id": "implementation.same_version",
  "status": "pass",
  "command_id": "<the original command that produced the raw artifact>",
  "expected": "HTTP 200 with an empty changes array",
  "observed": "HTTP 200 and changes was an empty array",
  "artifacts": ["output/same-version-response.json"]
}
```

The record's check ID and status must match the `checks.json` entry. `expected` and `observed` must be non-empty factual text; short values such as `200 OK` are valid and whitespace-only values are not. Every listed artifact must also appear in the check's `evidence` array and must be a non-empty regular file created or changed by the cited command. The executor records bounded SHA-256 and size metadata immediately after each command and rejects artifacts that changed later, symlinks, files outside `output/`, oversized files, and files beyond the capture limit. Because `command_id` is returned after a command finishes, write the evidence record in a later tool call and cite the original command ID that produced the raw artifacts. The evidence schema establishes attribution only; independent probes determine whether the reported outcome is true.

If a tool call runs a shell batch, copy its entire `command` argument verbatim, including setup lines, newlines, and redirects. A subcommand or script filename alone does not identify that recorded call. Several checks may reference the same complete command when its evidence covers each check.

Required check IDs and questions:

- `implementation.same_version`: Does comparing a version with itself return no changes?
- `implementation.function_removal`: Does the API identify a removed function?
- `implementation.required_impact`: Does a newly required input property receive the prescribed impact?
- `implementation.exact_version`: Does comparison require exact versions without substitution?
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
