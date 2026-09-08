# Atomic scenario validations

Each Registry scenario uses the regular Harness criterion and score report. `metrics.json` supplies the current questions, expectations, and weights; each scenario totals 100 points. There is no metric configuration flag, catalog version, catalog hash, frozen metric copy, standalone scorer, or cross-scenario coordinator.

Each metric answers one question. Change its description or weight directly in `metrics.json` and rebuild the runner. The source commit pin belongs to the application fixture, not a metric versioning mechanism.

## Who validates

- **Planning:** a separate model call reads the submitted plan and the requirements, then answers each planning question with a binary result and cited plan evidence. Its response is retained. This is a model judgment, not a deterministic proof of plan quality.
- **Implementation:** independent HTTP, browser, database, and patch-replay checks validate the delivered feature. The subject's report does not award points.
- **Environment:** the validator invokes the submitted environment commands in its private Docker daemon and checks the resulting database, API, frontend, artifacts, isolation, restart, and cleanup behavior.
- **Verification:** independent contract probes establish which required check IDs fail in the supplied implementation. The validator compares the tester's structured report with those observations and checks source preservation.

Verification recall is detection of failing **contract checks**, not an estimate of every possible defect or unique root cause. Precision measures how many reported failing check IDs also fail independently. Execution coverage counts required check IDs linked to recorded commands; evidence coverage counts reported outcomes with existing evidence files. These supporting metrics do not assess the semantic content of those files. A repeated check ID counts once. The controller's patch-replay check is excluded from the tester's required inventory.

## Observations and scoring

Validators retain raw results and evidence paths in `validation/observations.json`. Binary observations contain `value: 0` or `value: 1`. Ratio observations retain integer `numerator` and `denominator` counts; they never accept a caller-supplied derived percentage.

Measured points are `round(weight × value)`, using the regular integer criterion format. Raw ratios remain available in the evidence. Each scenario is scored independently through the normal Harness reports.

A product failure earns zero for the check it fails. A validator or infrastructure failure remains unavailable. A confirmed zero ratio denominator is not applicable and does not earn full credit. If a required metric cannot be measured, evaluation is unavailable under the existing evaluator contract; partial observations are still retained. No weight is redistributed and no substitute score is invented.

Screenshots let users inspect actual results. Their appearance is not scored.
