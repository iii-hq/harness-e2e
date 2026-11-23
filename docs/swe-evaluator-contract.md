# SWE evaluator contract

Kanban rubric version 3 separates candidate-authored quality checks from independent
functional observations. `pnpm test` still runs and retains its exit code and log,
but failure or its 90-second timeout does not prevent independent probes. Typecheck,
build, application presence and startup remain prerequisites. This is a scoring
contract change; older results keep their original rubric and are not rescored.
HTTP restart tests should bind a fresh ephemeral port rather than reusing a port
whose old socket can remain in a connection pool.

Kanban rubric version 4 changes only C4 evidence. Details read persisted fields
from the widest ancestor of the detail panel that adds no link or button named with
the ticket title, no title copy inside a link, button, list item, article or
draggable element, no other heading named like the title and no board lane heading,
so fields beside a heading block that holds both the title and the Delete action
count. Visible read-only or disabled form values also count, where a select counts
only its selected option. Creation clicks the exact `Create ticket` action
and deletion the whole-name `Delete ticket` action. Version 3 results are not
rescored. `provenance.controller_sha256["probe.mjs"]` = `082752870f1c…` identifies
the version 3 results from 0.35.0 to 0.37.0, which already apply #402; every earlier
version 3 result carries a different digest.

Kanban rubric version 5 changes how board counts and text values are matched in
C3, C4, C5 and C7 evidence. A column heading is the column name alone or the name
followed by `N`, `N ticket(s)` or `(N)`, and a separator (`,`, `:`, `·`, `–`, `—`
or `-`) may now stand between them, so `Backlog, 0 tickets` names the Backlog
column and `Backlog archive, 0 tickets` does not. A lane count is an element
holding only `N` or `(N)`, or the heading itself when its visible text reads as
the column name and that count (`Backlog, 0 tickets`, `Backlog (0)`); a count
carried only by the heading's aria-label does not count, nor does a number elsewhere
in its text, and a lane whose heading and badge both show
the count counts once. A visible board total is `N ticket(s)`, optionally followed
by `on (the) board`, `total` or `in total` and an optional trailing period, or
`Total`, optionally followed by `ticket(s)`, then an optional colon, then `N`,
optionally followed by `ticket(s)`, or an element holding only `N` and labeled
`Total` or `Total tickets`; matching ignores case. Playwright tests a regular
expression against an element's raw text and raw label, so a lane count (`N` or
`(N)`), the total and its label, card priority, detail status and detail
presentation values now ignore whitespace before and after the text. Other
surrounding text still fails, so `10 tickets total` is not a zero total and `0
tickets selected` is not a total. Version 4 results are not rescored.
`provenance.controller_sha256["probe.mjs"]` = `bbb269d804a3…` identifies the
version 4 results of 0.38.0.

Kanban rubric version 6 changes C3, C4 and C5 evidence. A failed first load counts
when an alert or status reads `could not`, `couldn't`, `cannot` or `can't`, as well as
`unable`, `error` or `fail`. A card's priority and assignee may follow their own field
label (`Priority: High`, `Assignee: Grace`, `Assigned to Grace`, `Owner: Grace`);
another value, another label or extra words (`High priority`) still fail. Settings
navigation in C3 and C5 follows the first visible link named `Settings`, so a header
link beside an in-page action no longer fails; the settings form checks still decide
where it led. C4 creation opens from a visible `New ticket` button or a link whose
whole name is `New ticket`, optionally with `+` before or `…` after it; creation still
submits with the exact `Create ticket` action. A C5 drag PATCH may wrap its status in
one key (`{"changes":{"status":"done"}}`); a second key or any other field still
fails. Version 5 results are not rescored. `provenance.controller_sha256["probe.mjs"]`
= `9524ad8ca7a6…` identifies the version 5 results of 0.39.0 and 0.40.0.

C5 selects a visible, hit-tested point in the target lane and checks a status-only
PATCH for the dragged ticket, sent bare or under one wrapper key. Failure, success and navigation probes
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
