# Atomic scenario validations

Each Registry scenario uses the regular Harness criterion and score report. `metrics.json` supplies the current questions, expectations, and weights; each scenario totals 100 points. The current Registry scoring contract is scenario version 2. Historical version 1 reports remain unchanged and must be read against their embedded criteria.

Each metric answers one question. Change its description or weight directly in `metrics.json` and rebuild the runner. The source commit pin belongs to the application fixture, not a metric versioning mechanism.

## Who validates

- **Planning:** a separate model call reads the submitted plan, requirements, and an independently captured inventory of paths in the pinned repository. Backend and frontend location credit is deterministically removed unless the judge cites an exact path from that inventory. Its response and the path inventory are retained.
- **Implementation:** independent HTTP, browser, database, and patch-replay checks validate the delivered feature. Exact-version selection is observed through accepted exact SemVer and rejected tag/range/wildcard requests. The subject's report does not award points.
- **Environment:** the validator allocates distinct ports inside its private Docker daemon, starts named Compose projects, verifies container/project/port identity before dependent observations, and checks database, API, frontend, artifacts, isolation, restart, and cleanup behavior. Failed preparation of either the primary or replay instance leaves observations that depend on that instance unavailable; cleanup still runs for partially created projects.
- **Verification:** independent contract probes establish which required check IDs fail in the supplied implementation. The validator compares the tester's structured report with those observations and checks source preservation.

Verification outcome accuracy has weight 55 and compares the reported pass/fail value for every one of the 24 required public check IDs with independent truth. Correct passes and correct failures count; missing, blocked, and incorrect reports do not. Incomplete independent truth makes outcome accuracy unavailable. Execution coverage and evidence coverage each have weight 20, and source preservation has weight 5. Commands should cite the immutable `command_id` returned by the execution tool. Legacy command strings match only after trimming outer whitespace. Evidence coverage requires a per-check structured record with matching check ID, status, and original command ID; non-empty factual expected and observed text without a minimum length; and raw artifacts whose bounded execution-time SHA-256 and size still match. This validates structural attribution, while independent probes determine truth. A repeated check ID counts once. The controller's patch-replay check remains outside the tester's inventory.

## Observations and scoring

Validators retain raw results and evidence paths in `validation/observations.json`. Binary observations contain `value: 0` or `value: 1`. Ratio observations retain integer `numerator` and `denominator` counts. All version 2 verification ratios use the fixed 24-check denominator, so silence earns zero instead of an empty-set reward.

Measured points are `round(weight × value)`, using the regular integer criterion format. Raw ratios remain available in the evidence. Each scenario is scored independently through the normal Harness reports.

A product failure earns zero for the check it fails. A validator or infrastructure failure remains unavailable. Other zero-denominator ratios remain not applicable. If a required metric cannot be measured, evaluation is unavailable under the existing evaluator contract; partial observations are still retained. No weight is redistributed.

Screenshots let users inspect actual results. Their appearance is not scored.
