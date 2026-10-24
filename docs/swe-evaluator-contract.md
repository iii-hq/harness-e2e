# SWE evaluator contract

Kanban rubric version 3 separates candidate-authored quality checks from independent
functional observations. `pnpm test` still runs and retains its exit code and log,
but failure or its 90-second timeout does not prevent independent probes. Typecheck,
build, application presence and startup remain prerequisites. This is a scoring
contract change; older results keep their original rubric and are not rescored.
HTTP restart tests should bind a fresh ephemeral port rather than reusing a port
whose old socket can remain in a connection pool.

C5 selects a visible, hit-tested point in the target lane and checks the exact
status-only PATCH for the dragged ticket. Failure, success and navigation probes
record separate observations. A failed subprobe does not cancel the others.

Form and State Machine accept edits through either the domain operation or the
active Canvas operation. The edit and its later observable effect must use matched
request/result identities for the current execution. Canvas edits must return the
active Canvas ID and the expected source. Requests without matching results do not
establish execution.

Registry uses the fixture's private comparison-fault gateway. Arm before restarting
the web service and opening a fresh browser; test both direct navigation and a
native version change from a successful comparison. The latter must begin with
visible comparison data, then show an error and remove the old result. Both paths
need attempt-scoped HTTP 503 receipts. Missing receipts, restart or cleanup failure
leave this observation unavailable; a proven request followed by incorrect UI is
a measured failure. Other independent observations remain available. Faults are
always cleared; seeded versions are not renamed to induce a failure.

Chess retains DOM, paint, hit testing, observed FENs, interactions and matched RPC.
The playable UI criterion remains strict. Screenshot completeness is evaluated
independently, with PNG hashes, URL/session, candidate identity and observed FEN
transitions bound to the portable captures.

The required Python CI gate runs browser controls, not just source-string checks:
100 drags per captured Kanban geometry, RPC and board visibility controls, and 100
Registry reproductions per SSR/SPA positive and negative case. Adversarial controls
include missing/uncorrelated results, wrong Canvas identity/source, stale results,
missing alerts and invalid gateway receipts. Missing Playwright fails these gates.
The fixture's Node gate separately checks 100 real HTTP faults per fixed route,
passthrough, private controls and cleanup. These controls isolate evaluator
behavior; they do not claim a new model campaign or production deployment passed.
