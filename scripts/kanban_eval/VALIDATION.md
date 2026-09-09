# Local control validation — 2026-09-09

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
