# Kanban validation — 2026-09-09

## Native CI integration: local acceptance passed; publication pending

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

- Rust library: 667 passed, one external-fixture test ignored; fresh binary build,
  Rust formatting, JavaScript syntax and shell syntax passed.
- Full Python suite: 258 passed using the fresh native binary. Focused command,
  standalone subject and controller regressions also passed after integration.
- The same bootstrap used by CI installed locked tooling and pulled/verified the
  pinned Playwright image. Its browsers and system libraries run without host
  library mounts. Only unused Docker build cache was removed with user approval
  (8.996 GB reclaimed); images, containers and volumes were preserved.
- Final controls under `/tmp/kanban-ci-runtime-pxVNnP/acceptance-controls/`: all seven
  references passed with complete criterion coverage; all seven bases failed
  for missing functionality, with no infrastructure/evaluator errors. C1/base
  stops at the explicit missing-application prerequisite, not browser coverage.
  Every control records probe SHA-256
  `0b58f77e919e327453bbdb753a0fde69fee11f44104b9a829ea67dde0a0e96d1`.
- Desktop/mobile board, settings, non-modal details, editing, real pointer drag,
  discussion and third-session deletion screenshots were visually inspected.
  C7 also passed real SSE heap collection, reconnect and restart checks.

Calibration fixes from real runs:

- Keep the browser dependency mount under `node_modules` for npm package lookup.
- Use Docker init and preserve its fixed keeper PID while terminating/reaping
  other candidate processes. A real Compose proof observed a new server PID,
  no stale adoption and no zombies after restart. Do not re-seed configuration
  on restart: this previously overwrote the selected directory and unrelated keys.
- Read hot-reload markers from native `compose::logs`, where managed worker
  stdout lives; restore the original source and require readiness afterward.
- Wait for raw/effective configuration to converge, ignore hidden-dialog text
  duplicates, inspect cleared drafts after restoring the original store, and
  capture mobile edited details before navigating away.
- Missing CRUD no longer turns a dependent store probe into evaluator failure.
  Each criterion has explicit functional dependencies; unrelated failures retain
  partial credit, while missing/unverified evidence remains unavailable.

`native-c3-1` invoked DeepSeek Flash through the native lifecycle but a subject
test command reached 120 seconds. The old executor deleted the candidate and
cancelled the session before diff capture. Official usage/cost were unavailable;
they are not replaced by transcript sums. Its hashed diagnostic was exported by
the real CI extractor. This attempt is not full native acceptance.

Command limits are now recoverable: timeout/output overflow stops all candidate
processes except init and the pre-recorded keeper, returns bounded partial output
with exit 124/125, and keeps the workspace for correction. Cleanup failure still
removes the exact container and aborts. A real 120-second Docker proof observed
no runaway process/zombies and a successful next command in the same container.

`native-c3-2` completed the native lifecycle in 478,375 ms, preserving an identical
delivered/evaluated diff and exporting the application audit with the real CI
extractor. Official metrics are complete: 27 turns, 26 function calls, 26,845
input tokens, 62,234 output tokens, 1,594,240 cache-read tokens and US$0.025647692.
One truncated tool call was rejected and recovered; this is not a command-timeout
proof. Typecheck, all 15 candidate tests and build passed.
The recorded final context reports an effective output ceiling of 32,000 tokens,
despite the requested 65,536; preserve that distinction when comparing runs.

Its initial zero score is **evaluation-invalid**, not a model capability failure:
the probe required the reference's exact unknown-count text and a count inside
the lane heading. The captured candidate correctly displayed an ellipsis while
loading and a separate count badge next to each lane title. Correction and exact
candidate replay are recorded separately; the original report is not rewritten.
`native-c3-2-replay-1` passed all four criteria with complete coverage after the
semantic fix (probe `23f75aad09d85e11cb98d308732c32e2322930986bbf7c73ad4b8f5973d3c80e`).
It reconstructed the exact base in a fresh isolated workspace and reapplied the
captured diff; delivered and evaluated diffs match the original byte for byte.
This is re-evaluation, not a new model run; it incurs no new model usage/cost.
Desktop/mobile captures were inspected and preserve literal malicious-looking
text while confining horizontal scrolling to the board.

The final exact replay, `native-c3-2-acceptance-replay`, also passed all four
criteria with complete coverage and unchanged delivered/evaluated bytes using
the final probe hash above. The native binary was rebuilt with this probe;
Rust tests/formatting, Python tests, JavaScript/shell syntax and diff checks pass.

The semantic audit corrections are included in the final controls:

- C1 inspects declared Compose containers and saves through the form without
  requiring reference copy; mobile and desktop settings are inspected.
- C2 verifies UUIDs, increasing keys and timestamps, and duplicates a ticket
  collection inside either a root array or an object envelope. An unsupported
  store representation remains evaluator-unavailable, not a model zero.
- C3/C5 accept sibling count badges and wait for asynchronous board counts.
  C3's four criteria are independent; C5 checks status-only changes and advancing
  timestamps. Earlier rounds exposed navigation races and remain calibration
  evidence rather than candidate failures.
- C4 rejects non-JSON with 400/415, verifies same-tab non-modal details and checks
  that late deletion removes the visible board card, not only the API record.
- C6 checks visible authors/times, DOM posting order, trimmed input and whitespace
  rejection. Parent navigation must reach the actual parent via focus or anchor;
  focusing the back-reference label alone no longer passes.
- C7 checks native named SSE events/payloads and no event after failed persistence,
  uses observable state instead of connection/error copy, verifies remote moves,
  and preserves a real pointer drag through an ordinary remote update before
  switching stores. Disconnect collection and runtime restart also pass.

Real Chromium regressions cover equivalent board DOMs, incorrect counts and
parent navigation. A separate synthetic-server EventSource proof accepted named
events and rejected wrong names/invalid payloads; failed persistence emitted no
event. Passing the earlier reference/base controls alone did not expose these
semantic gaps, so those runs are not the final acceptance evidence.

The public repository `iii-hq/kanban-e2e-fixture` was created; history is not yet
pushed pending confirmation about existing author metadata. CI checkout now uses
this fixed public source without a dedicated variable or secret. Remaining remote
gates: publish the exact fixture history and publish/select a compatible immutable
`harness-e2e` worker release after merge. Selecting a new `runner_sha` alone does
not replace the Registry-resolved worker binary. No Release Control CI execution
or worker release has been performed for this change.

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
