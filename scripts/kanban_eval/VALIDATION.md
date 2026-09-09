# Kanban validation — 2026-09-09

## Native CI integration: not yet end-to-end approved

The seven cases are native `ScenarioId`s and materialize through the existing
`capability` profile (55 planned runs, seven Kanban cases). The implementation is
rebased on `4b31b62`; the fixture catalog still passes its seven exact, linear
transition checks. No fixture implementation or reference commit was changed.

Corrections since the historical controls below:

- Fixed ignored-file Git staging and capture the evaluated diff again to detect
  source changes during grading. External-subject lifecycle, command bounds,
  cancellation and cleanup now have regression coverage.
- Added private restart, raw-store corruption/restore, generic source hot-reload
  and Node inspector checks. Control protocol errors stay evaluator failures;
  missing hot reload is a functional failure. A real, separate Node HTTP/inspector
  process validates SSE response counting, collection and deliberate retention.
- Replaced host system-library mounts with the pinned official Playwright image
  `mcr.microsoft.com/playwright@sha256:cf0daee9b994042e011bc29f20cdff1a9f682a039b43fcd738f7d8a9d3bcd9d6`.
  CI uses Node 24.18.0, locked JavaScript tooling, no install lifecycle scripts
  and no persisted checkout credentials.
- Fixed engine inspection routing for a namespaced stack and selected the Harness
  version from the matching namespace. Normalized build/startup failure remains
  task failure; unavailable grading never becomes an invented zero score.
- Persist bounded logs, screenshots and diffs as native evidence; preserve and
  hash terminal controller diagnostics even without a subject transcript. The
  CI extractor now copies the allowlisted Kanban files before removing the stack.

Current validation:

- Rust library: 667 passed, one external-fixture test ignored; fresh binary build
  passed. Rust formatting, JavaScript syntax and shell syntax passed.
- Full Python suite: 251 passed using the freshly built native binary, including
  the real Node inspector/SSE instrument and the output-symlink regression.
- `ci-c3-reference-1` passed complete functional/criterion coverage and
  `ci-c3-base-1` failed the missing board as expected, before the runtime-image
  change. Expanded C2/C5/C6 references exposed a private-control payload mismatch;
  those calibration errors were fixed and are not model failures.
- `native-c3-preflight-5` reached native setup against `my-project` with requested
  `deepseek/deepseek-v4-flash`. It failed closed because the pinned image is not
  installed. `results.json` records infrastructure error, null score and no model
  usage; the hashed controller diagnostic and its exact error survived cleanup
  and extraction into CI artifacts. No model was invoked in this attempt.

Outstanding gates: pull/smoke the new image, repeat all seven reference/base
pairs with the expanded probes, visually inspect the new artifacts, and run a
native DeepSeek Flash attempt. The old controls below do not validate the new
image or newly added assertions. The host root filesystem has no free space;
shared Docker images, containers, volumes and cache were not removed.

Remote prerequisites remain unconfigured: publish the fixture to a user-selected
GitHub repository, grant CI read access, and publish/select a compatible immutable
`harness-e2e` worker release after merge. Merely selecting a new `runner_sha` does
not replace the Registry-resolved worker binary.

## Historical local controls

Seven reference snapshots passed the implemented functional probes. All seven
base snapshots failed for the feature intentionally missing at that revision.
These are functional controls, **not** full acceptance: reference results remain
`incomplete` because the coverage file explicitly lists unverified criteria.

| Case | Reference functional result | Base failure observed |
| --- | --- | --- |
| C1 foundation | passed (attempt 2) | No application exists |
| C2 persistence | passed | iii create/list functions absent |
| C3 board | passed | Board and ticket counts absent |
| C4 ticket flow | passed | Creation button and endpoint absent |
| C5 edit/move | passed | Update function absent |
| C6 discussion | passed (attempt 2) | Comment form and function absent |
| C7 live | passed (attempt 3) | Browser misses direct iii creation |

Artifacts are retained locally under
`/tmp/kanban-harness-WOOZ2Q/runs/docker-c{N}-{reference|base}-{attempt}/evidence/`.
Each run records its actual controller, runtime, dependency and browser digests;
image tags are resolved to immutable Docker IDs. These local temporary artifacts
are not uploaded or committed into the repository.

Visual inspection covered settings on mobile, the five-column board on desktop
and mobile, non-modal mobile details, the board after a real pointer drag,
the comments/replies timeline on mobile, and the third session after remote
deletion. Mobile horizontal scrolling remains confined to the board.

Calibration findings were retained, not counted as subject failures:

- C1 incorrectly required later settings navigation; its status locator also
  matched an HTML output element rather than the save confirmation.
- C6 incorrectly required C7 live synchronization. Comment locators in C6/C7
  also matched the activity list instead of only the comment textbox.
- C7's development process restarted during the initial request sequence.
  Readiness now requires 12 consecutive healthy HTTP samples before probing;
  failed checks retain screenshots, and comment creation verifies its HTTP status.

Docker isolation was probed with separate mount/PID namespaces, shared isolated
loopback, denied outbound network and inaccessible evaluator files. Containers
are non-root, read-only, capability-free, bounded in CPU/memory/PIDs, and removed
after each run. No host security policy was disabled. The optional model path
also uses bounded tmpfs volumes and an output limit; its snapshot copy was tested
with no privileged setup or host workspace writes by candidate code.

## DeepSeek Flash C2 smoke

`deepseek-c2-smoke-3` verified actual provider/model `deepseek/deepseek-v4-flash`.
The model made 12 real command calls, reached `max_turns (12)`, and left an empty
diff. Private C2 probes failed because create/list functions were absent. Base
typecheck, tests and build passed; session `completed` is not task success.
Reported subject duration was 38,182 ms and cost US$0.0034219976, with 15,185 input,
2,595 output and 203,392 cache-read tokens; cache-write was unreported (`null`).
This bounded smoke is not a model-quality benchmark or a seven-case campaign.

Attempt 1 was evaluation-invalid: native tool discovery in the live namespace
returned no contracts, and the bridge incorrectly required an optional cache-write
counter. The bridge now uses the existing restricted `agent_trigger` dispatcher
and preserves missing cache counters. Attempt 2 rejected a short case alias before
invoking any model. Neither attempt is counted as a subject capability failure.

Final local regression validation: 206 Python tests passed using the existing
Harness release binary; no Rust source changed. Subject syntax and diff checks
also passed. Full criterion coverage and native Harness integration remain open.

## Higher-budget C2 attempts

Budgets were raised on request to 100 turns, 1,000,000 total tokens, US$5 and
1,800 seconds. The controller allows another 250 seconds for harvesting and
cleanup. Commands retain isolation and use 120-second/256-KiB limits.

- `deepseek-c2-high-budget-1`: evaluation-invalid after a command exceeded the
  original 16-KiB output bound and removed the candidate. Cost US$0.0070302848.
  The bridge now terminates the session on a fatal command bound, preserving its
  actual cause. Regression coverage exercises output overflow and cleanup.
- `deepseek-c2-high-budget-2`: private functional probes failed with an empty diff;
  duration 208,348 ms, cost US$0.0121278192. The final assistant entry used exactly
  the 16,384-output-token ceiling, with no final text or tool call. The runtime
  reported `stop_reason: end` and session `completed`, not a successful task.
  The next attempt raises per-response output to 65,536 tokens, below the live
  catalog's reported 384,000-token maximum. No reference implementation changed.
- `deepseek-c2-high-budget-3`: model completed 43 turns with implementation and
  test commands; duration 531,080 ms, cost US$0.0348094376. Evaluation failed before
  private probes: `git add --all` with excluded ignored directories reported
  `kanban/dist` as ignored and returned nonzero. This was reproduced read-only
  using `git add --dry-run` on the fixture. The candidate was cleaned up; its
  transcript and usage survive, but no complete diff or functional verdict does.
  Fix and regression-test change capture before another model run. Do not count
  this attempt as a functional pass or a model implementation failure.

Final higher-budget runner checks: all 206 Python tests passed, including fatal
output-bound handling; JavaScript syntax and whitespace validation passed.
