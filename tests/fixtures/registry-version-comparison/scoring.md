# Atomic scenario validations

Each Registry scenario uses the regular Harness criterion and score report. `metrics.json` supplies the current questions, expectations, and weights; each scenario totals 100 points. There is no metric configuration flag, catalog version, catalog hash, frozen metric copy, standalone scorer, or cross-scenario coordinator.

Each metric answers one question. Change its description or weight directly in `metrics.json` and rebuild the runner. The source commit pin belongs to the application fixture, not a metric versioning mechanism.

## Who validates

- **Planning:** deterministic checks over the submitted `plan.md` answer each planning question with a binary result and the cited plan lines. Contract questions look for the stated terms, location questions for pinned-source paths, sequence and test questions for document structure, and scope for excluded work that is not ruled out. The checks prove that the plan states the contract, not that the stated plan would work.
- **Implementation:** independent HTTP, browser, database, and patch-replay checks validate the delivered feature. The subject's report does not award points.
- **Environment:** the validator invokes the submitted environment commands in its private Docker daemon and checks the resulting database, API, frontend, artifacts, isolation, restart, and cleanup behavior.
- **Verification:** independent contract probes establish which required check IDs fail in the supplied implementation. The validator compares the tester's structured report with those observations and checks source preservation.

The implementation scenario does not score object-key order independently: the fixture persists schemas as PostgreSQL `jsonb`, which normalizes key order before the API can observe it. The former 4 points are split between the independently exercisable schema `required` and `enum` set-order checks.

Verification recall is detection of failing **contract checks**, not an estimate of every possible defect or unique root cause. Precision measures how many reported failing check IDs also fail independently. A reported failure counts toward recall and precision only when its check has a recorded execution, the same command receipt and proof execution coverage counts; a failure claimed without one is not a detection, and it stays in the precision denominator as a failure that was not reproduced. When neither side reports a failure, precision is the measured empty-set value 1, provided the report executed at least one check. Execution coverage counts required check IDs whose completed command receipt records the cited proof's final path, size, and SHA-256 from a private per-invocation DIND staging directory. The command may exit non-zero when it proves a failure. Evidence coverage uses the complete required inventory as its denominator and accepts one staged JSON proof per check with matching status and internally consistent assertions containing exact, type-sensitive expected and observed JSON values. Direct output files and self-referential reports do not count. A repeated check ID is invalid for coverage. The controller's patch-replay check is excluded from the tester's required inventory.

## Observations and scoring

Validators retain raw results, command receipts, evidence sizes, and SHA-256 values in `validation/observations.json` and `validation/verification-evidence.json`. Binary observations contain `value: 0` or `value: 1`. Ratio observations retain integer `numerator` and `denominator` counts. A measured zero-denominator observation also contains the normalized `value` defined by its metric: recall and precision use 1 for an empty failure set in a readable report with at least one executed check. Without a readable, non-empty `checks.json`, recall, coverage and source preservation score 0: an untouched source is not preserved by a tester that attempted nothing. A report that only claims results, with no check whose command receipt proves its evidence, scores 0 on recall, precision and source preservation for the same reason, whether it claims every check passes or every check fails. The run remains incomplete.

Measured points are `round(weight × value)`, using the regular integer criterion format. Raw ratios remain available in the evidence. Each scenario is scored independently through the normal Harness reports.

A product failure earns zero for the check it fails. Delivered source that cannot be started, or a verification task that changed the Registry source, earns zero on every metric. A validator or infrastructure failure, such as an unreachable Docker daemon, makes evaluation unavailable; partial observations are still retained. Explicitly not-applicable ratios remain unassessed while other criteria are scored. No weight is redistributed.

Screenshots let users inspect actual results. Their appearance is not scored.

## Completion

A metric marked `"gate": true` in `metrics.json` names the scenario's primary flow. The scenario is completed only when every gate earns its full points; the score is still the sum of all metrics.

- **Planning:** `planning.plan_delivered` — `plan.md` exists at `/workspace/output/plan.md`, is UTF-8 text, and has a Markdown heading followed by content. The other planning metrics measure plan content only, so this gate was added. Its 5 points come from merging `planning.required_order` and `planning.enum_order` into `planning.schema_sets`, which the requirements state as one rule; the set still has 20 metrics of 5 points.
- **Implementation:** `implementation.same_version` and `implementation.function_removal`: the comparison works. A patch that replays cleanly on the pinned base is delivery format and only moves the score.
- **Environment:** `environment.build` and `environment.api_readiness`.
- **Verification:** full recall, full precision, full execution coverage, full evidence coverage, and source preservation. Missing a known failure, claiming a false failure, omitting a required check or attributable artifact, or changing source leaves the task incomplete.

Delivered source that cannot be started scores zero on every metric, gates included, so it is never completed.
