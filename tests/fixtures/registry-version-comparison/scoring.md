# Atomic scenario validations

Each Registry scenario uses the regular Harness criterion and score report. `metrics.json` supplies the current questions, expectations, and weights; each scenario totals 100 points. There is no metric configuration flag, catalog version, catalog hash, frozen metric copy, standalone scorer, or cross-scenario coordinator.

Each metric answers one question. Change its description or weight directly in `metrics.json` and rebuild the runner. The source commit pin belongs to the application fixture, not a metric versioning mechanism.

## Who validates

- **Planning:** deterministic checks over the submitted `plan.md` answer each planning question with a binary result and the cited plan lines. Contract questions look for the stated terms, location questions for pinned-source paths, sequence and test questions for document structure, and scope for excluded work that is not ruled out. The checks prove that the plan states the contract, not that the stated plan would work.
- **Implementation:** independent HTTP, browser, database, and patch-replay checks validate the delivered feature. The subject's report does not award points.
- **Environment:** the validator invokes the submitted environment commands in its private Docker daemon and checks the resulting database, API, frontend, artifacts, isolation, restart, and cleanup behavior.
- **Verification:** independent contract probes establish which required check IDs fail in the supplied implementation. The validator compares the tester's structured report with those observations and checks source preservation.

Verification recall is detection of failing **contract checks**, not an estimate of every possible defect or unique root cause. Precision measures how many reported failing check IDs also fail independently. Execution coverage counts required check IDs linked to recorded commands; evidence coverage counts reported outcomes with existing evidence files. These supporting metrics do not assess the semantic content of those files. A repeated check ID counts once. The controller's patch-replay check is excluded from the tester's required inventory.

## Observations and scoring

Validators retain raw results and evidence paths in `validation/observations.json`. Binary observations contain `value: 0` or `value: 1`. Ratio observations retain integer `numerator` and `denominator` counts. A measured zero-denominator observation also contains the normalized `value` defined by its metric: recall and precision use 1 for an empty failure set in a readable report, while evidence coverage uses 0 when no outcomes were reported. Without a readable `checks.json` the tester verified nothing: recall, precision, and source preservation score 0.

Measured points are `round(weight × value)`, using the regular integer criterion format. Raw ratios remain available in the evidence. Each scenario is scored independently through the normal Harness reports.

A product failure earns zero for the check it fails. Delivered source that cannot be started, or a verification task that changed the Registry source, earns zero on every metric. A validator or infrastructure failure, such as an unreachable Docker daemon, remains unavailable. Other zero-denominator ratios remain not applicable. If a required metric cannot be measured, evaluation is unavailable under the existing evaluator contract; partial observations are still retained. No weight is redistributed.

Screenshots let users inspect actual results. Their appearance is not scored.

## Completion

A metric marked `"gate": true` in `metrics.json` names the scenario's primary flow. The scenario is completed only when every gate earns its full points; the score is still the sum of all metrics.

- **Planning:** `planning.plan_delivered` — `plan.md` exists at `/workspace/output/plan.md`, is UTF-8 text, and has a Markdown heading followed by content. The other planning metrics measure plan content only, so this gate was added. Its 5 points come from merging `planning.required_order` and `planning.enum_order` into `planning.schema_sets`, which the requirements state as one rule; the set still has 20 metrics of 5 points.
- **Implementation:** `implementation.patch_application` and `implementation.function_removal`.
- **Environment:** `environment.build` and `environment.api_readiness`.
- **Verification:** `verification.recall`; a partial ratio does not complete the task.

Delivered source that cannot be started scores zero on every metric, gates included, so it is never completed.
