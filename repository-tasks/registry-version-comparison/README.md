# Registry development scenarios

Four independent scenarios use the regular Harness catalog, execution, reports, and scoring. There is no Registry-specific CLI or multi-test coordinator.

| Scenario | Input | Work | Main output |
| --- | --- | --- | --- |
| `registry_planning` | Pinned Registry and public requirements | Inspect the application and plan the feature | `workspace/output/plan.md` |
| `registry_implementation` | Same base, requirements, reference plan, prepared environment | Implement the feature, run tests, exercise the application | `delivery/implementation.patch`, report, screenshots |
| `registry_environment` | Same base, runtime requirements, seed and artifacts | Create Dockerfile/Compose/scripts and run the baseline application | Environment patch and reproduction report |
| `registry_verification` | Explicitly supplied delivery, requirements, fresh environment | Test the feature through API and browser, preserving source | Verification report, check results, screenshots |

Only implementation receives the reference plan. Environment receives no prepared Dockerfile, Compose file, or smoke script. Verification has an explicit delivery input; it does not automatically depend on another scheduled scenario.

## Prerequisites

Use a Linux amd64 executor with Python 3, Git, Docker Engine, outbound dependency access, and a running Harness/iii stack. Implementation, environment, and verification use private privileged Docker-in-Docker containers. Each has its own daemon and data volume. Use a dedicated executor for these containers.

Every scenario clones the newest default branch of `iii-hq/e2e-fixture`. There is no fixture commit option or branch fallback. Its `registry-version-comparison` directory must exist on that branch before execution; the original fixture work is in [fixture PR #3](https://github.com/iii-hq/e2e-fixture/pull/3).

Registry starts at `662eb87c1bdbb395f36264d5d26bf823e2ace783`. Dependency installation and image building occur inside each private daemon.

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

Select and schedule each scenario through the normal Harness flow. No automatic sequence or combined four-test score is added.

Subject commands start in `/workspace`, containing `registry/`, `inputs/`, and `output/`. The scenario provides a scoped execution tool. `inputs/environment.json` records URLs, requirements, and commands. Implementation and verification use built application snapshots: rebuild after edits with `/fixture/fixture.sh up` and run project tooling inside the API/web containers.

## Results

The regular Harness report contains each scenario's criterion scores and captured evidence. Its JSON deliverable embeds evidence files (text or base64 for images), so archived results do not depend on the executor workspace. The existing capture size limit applies; omitted files are listed explicitly. [Validation details](scoring.md) explain how observations become scores.

The attempt workspace retains source patches, preparation and command logs, `validation/observations.json`, and `workspace/output/` reports. Implementation and verification also retain `screenshots/captures.json` and `screenshots/index.html`; open the HTML to see actual browser captures. Missing UI is recorded as unavailable. Environment screenshots are produced by the subject.

Cleanup removes the attempt's private runtime and daemon volume, preserving files. After an interrupted controller, run the materialized lifecycle helper against that attempt:

```bash
python3 /absolute/attempt-assets/lifecycle.py cleanup --root /absolute/attempt
```

## Local checks

```bash
python3 -m unittest discover -s tests/python -p 'test_registry_*.py' -v
node --check repository-tasks/registry-version-comparison/validate-feature.cjs
cargo test --locked registry
```

These checks exercise contracts, patch replay, observation arithmetic, and probe behavior. A model-driven implementation run is separate evidence.
