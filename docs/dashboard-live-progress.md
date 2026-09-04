# Dashboard progress during execution

The execution detail and plan lifecycle show `live_progress` while a run is
active, without waiting for `results.json`. The projection reads the verified
execution journal and only artifacts referenced by committed events. It does
not mutate the journal or create a partial final report. The run contract stays
v1; resume and re-evaluation are not part of this change.

The dashboard exposes the current phase/attempt, last recorded update, planned
slots, committed runs, deferred slots, and finished/started physical attempts.
Recorded rows retain completion, technical validity, objective score, and
quality separately. Pending slots have no outcome.

Metrics are explicitly provisional and include coverage:

- Completion rate: completed / (completed + task-incomplete). Undetermined,
  pending, and deferred counts remain visible rather than becoming failures.
- Quality: median quality of scored completed runs, alongside the number of
  completed runs with a score. This is not a final execution verdict.
- Tokens observed: sum of complete input/output telemetry in committed physical
  attempt observations, including retries exactly once. Missing or incomplete
  telemetry is unknown, not zero; coverage shows observed/started attempts.
- Cost observed: sum of known committed logical-run costs. These costs already
  include retries, so they are not added again from physical observations.
  Coverage shows runs with cost / recorded runs.

Usage from an active attempt is not streamed token by token. It appears when the
corresponding observation is checkpointed. Long phases can therefore have no
new checkpoint for some time; the last-update age is not a liveness verdict.

Change events trigger a coalesced refresh; a five-second polling fallback
recovers missed events/disconnections. Active summaries bypass the historical
read-model cache. Background request failures preserve the last received detail
and show a warning. Journal verification failures expose `live_progress_error`
without displaying unverified progress or hiding the execution itself.

Once lifecycle metadata is terminal and a final report is available, the final
report replaces the live projection. If interrupted without a final report, the
same retained checkpoints stay visible as partial evidence. Executions created
before journaling remain readable without a progress projection.

## Whole-execution summary after completion

The execution detail includes an **execution summary** above scenario results,
with a direct **metrics** tab in the section navigation.
It pools the logical runs from every compatible scenario report, rather than
averaging scenario rates or medians. It shows completion, technical reliability,
completion evidence coverage, quality on completed tasks, objective score, and
subject/judge tokens, failed-attempt tokens, cost, accumulated run time, and
function activity. Deferred and undetermined runs remain explicit.

Run efficiency and cost already include retries and are counted once. Judge
usage is summed across physical attempts. Completed token p50 is calculated
from completed logical runs, including their retries; tokens per completion is
all subject consumption divided by completed runs.

Unknown consumption never becomes zero. The summary retains known subtotals
with their telemetry coverage, but does not report tokens per completion or a
completed-token median when the required samples are missing. Incompatible,
inconsistent, or duplicate scenario evidence is isolated and the remaining
figures are explicitly labelled as a partial subset, not an execution total.
