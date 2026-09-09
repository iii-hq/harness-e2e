# Registry development scenarios

Four ordinary scenarios use the regular Harness catalog, execution, reports, and scoring. There is no Registry-specific CLI or multi-test coordinator.

| Scenario | Input | Work | Main output |
| --- | --- | --- | --- |
| `registry_planning` | Pinned Registry and public requirements | Inspect the application and plan the feature | `workspace/output/plan.md` |
| `registry_implementation` | Same base, requirements, reference plan, prepared environment | Implement the feature, run tests, exercise the application | `delivery/implementation.patch`, report, screenshots |
| `registry_environment` | Same base, runtime requirements, seed and artifacts | Create Dockerfile/Compose/scripts and run the baseline application | Environment patch and reproduction report |
| `registry_verification` | Explicitly supplied delivery, requirements, fresh environment | Test the feature through API and browser, preserving source | Verification report, check results, screenshots |

Only implementation receives the reference plan. Environment receives no prepared Dockerfile, Compose file, or smoke script. Verification consumes a specific implementation delivery.

## Prerequisites

Use a Linux amd64 executor with Python 3, Git, Docker Engine, outbound dependency access, and a running Harness/iii stack. Implementation, environment, and verification use private privileged Docker-in-Docker containers. Each has its own daemon and data volume. Use a dedicated executor for these containers.

Planning needs only the pinned source and requirements. The other three scenarios clone the newest default branch of `iii-hq/e2e-fixture`. There is no fixture commit option or branch fallback. Its `registry-version-comparison` directory supplies the prepared environment and public seed assets.

Registry starts at `662eb87c1bdbb395f36264d5d26bf823e2ace783`. Dependency installation and image building occur inside each private daemon.

## Release Control and Console plans

The existing `evolution` profile includes all four Registry cases alongside its other eighteen cases, with three repetitions per case. Planning and environment construction have separate groups. Implementation and verification run sequentially in one ordinary group, with four individual scenario results retained. The existing plan summary weights groups; use each scenario's criteria to assess its specific task.

Release Control's existing `harness-evolution` plan selects this profile through the existing executor. Publish the updated runner to include the expanded scope. No additional workflow or scheduler is required.

Within the shared execution, implementation publishes its delivery for verification. A new repetition clears that input, and a missing delivery fails verification rather than selecting an older file. Concurrent executions cannot exchange deliveries. Verification applies the patch to a fresh pinned Registry checkout and starts a fresh environment.

In the E2E extension, create a plan with one scenario and an execution model. Select an explicit judge for `registry_planning`; the other three scenarios use runtime validators. Use one run and zero technical retries for initial validation so failures remain visible.

Configure `HARNESS_E2E_RUN_DIR` on the E2E worker to a writable directory on the executor workspace disk. Set `TMPDIR` there as well when the host temporary filesystem has a separate quota. The worker process must receive these variables before the run starts; setting them in the browser does not configure the worker.

For standalone verification of a previously delivered implementation, configure `HARNESS_E2E_REGISTRY_IMPLEMENTATION` on the worker with the explicit `delivery/` directory containing `implementation.patch` and `manifest.json`. Reconcile the worker while it has no active executions. The paired profile does not use this external input. Delivery files also remain in normal captured evidence after cleanup.

Use the Evolution profile template in the Console and select the four Registry cases with one repetition to exercise the same grouping locally. On a disk-constrained executor, run the Docker builds sequentially. The profile is explicit; existing plan scopes and schedules remain unchanged.

## Run

Use the regular command, selecting one scenario:

```bash
cargo run --locked -- run \
  --url ws://127.0.0.1:49134 \
  --model "$HARNESS_E2E_MODEL" \
  --provider "$HARNESS_E2E_PROVIDER" \
  --scenario registry_implementation
```

Planning uses a separate model call to assess the plan against the atomic questions. Use the regular `--judge-model` and `--judge-provider` options (or `HARNESS_E2E_JUDGE_MODEL` and `HARNESS_E2E_JUDGE_PROVIDER`). If the judge cannot run, its measurements are unavailable.

To test a previously delivered implementation:

```bash
export HARNESS_E2E_REGISTRY_IMPLEMENTATION=/absolute/previous-attempt/delivery
cargo run --locked -- run \
  --model "$HARNESS_E2E_MODEL" --provider "$HARNESS_E2E_PROVIDER" \
  --scenario registry_verification
```

Select and schedule scenarios through the normal Harness flow. The Evolution profile uses the existing group execution and scoring contracts.

Subject commands start in `/workspace`, containing `registry/`, `inputs/`, and `output/`. The scenario provides a scoped execution tool. `inputs/environment.json` records URLs, requirements, and commands. Implementation and verification use built application snapshots: rebuild after edits with `/fixture/fixture.sh up` and run project tooling inside the API/web containers.

## Results

The regular Harness report contains each scenario's criterion scores and captured evidence. Its JSON deliverable embeds evidence files (text or base64 for images), so archived results do not depend on the executor workspace. The existing capture size limit applies; omitted files are listed explicitly. [Validation details](scoring.md) explain how observations become scores.

The attempt workspace retains source patches, preparation and command logs, `validation/observations.json`, and `workspace/output/` reports. Implementation and verification also retain `screenshots/captures.json`, JPEG files, and `screenshots/index.html`; open the HTML to see actual captures made through the `browser` worker. Each attempt uses a fresh private browser session. Missing or unexpected UI is recorded with its observed state and does not change objective scoring. Environment screenshots are produced by the subject.

Cleanup removes the attempt's private runtime and daemon volume, preserving files. After an interrupted controller, run the materialized lifecycle helper against that attempt:

```bash
python3 /absolute/attempt-assets/lifecycle.py cleanup --root /absolute/attempt
```

## Local checks

```bash
python3 -m unittest discover -s tests/python -p 'test_registry_*.py' -v
node --check tests/fixtures/registry-version-comparison/validate-feature.cjs
cargo test --locked registry
```

These checks exercise contracts, patch replay, observation arithmetic, and probe behavior. A model-driven implementation run is separate evidence.
