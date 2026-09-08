# Registry development tasks

Four native Harness tasks produce plans, source patches, command logs, runtime reports, and screenshots. Execution collects evidence without invoking an evaluator. The [atomic metric scorer](scoring.md) calculates scores separately from independent, evidence-backed observations. The `registry-tests` command is separate from the scored scenario catalog and dashboard execution history.

| Test | Input | Work | Main output |
| --- | --- | --- | --- |
| 1 — Planning | Pinned Registry, public requirements | Inspect the application and write an implementation plan | `workspace/output/plan.md` |
| 2 — Implementation | Same base, requirements, reference plan, prepared environment | Implement the feature, add and run tests, exercise the real application | `delivery/implementation.patch`, report, screenshots |
| 3 — Environment | Same base, runtime requirements, seed and artifact payloads | Create Dockerfile/Compose/scripts, migrate, seed, start, and verify the baseline application | Environment source patch and reproduction report |
| 4 — Verification | Test 2's patch applied to a clean base, requirements, fresh environment | Test the actual feature through API and browser, report expected/observed behavior, preserve product source | Verification report and screenshots |

The reference plan reaches only Test 2. Test 3 does not receive the prepared Dockerfile, Compose file, or smoke script. Test 4 receives the implementation even if a nonempty partial patch was captured after an interrupted subject session; its manifest records that session status. Without a delivered patch or a normally finished Test 2 session, Test 4 is blocked.

## Prerequisites

Use a dedicated Linux amd64 executor with Python 3, Git, Docker Engine, outbound dependency-download access, and a running Harness/iii stack with the requested model/provider. Tests 2–4 run privileged Docker-in-Docker containers, each with its own daemon and data volume; the host Docker socket, controller assets, credentials, and other task workspaces are not mounted. Privileged containers require a dedicated executor rather than being treated as a hostile-code security boundary.

The [Registry fixture PR](https://github.com/iii-hq/e2e-fixture/pull/3) must be merged before normal execution. Every test fetches the latest default branch of `iii-hq/e2e-fixture` anew. There is no fixture revision argument or branch fallback. If the fixture directory is absent, preparation fails before a model session starts. The chosen fixture copy remains fixed for that test and its file checksums are saved.

Registry always starts at `662eb87c1bdbb395f36264d5d26bf823e2ace783`. The runner image is pinned by digest. Dependency installation and image building happen within each private daemon; allow disk space and time for independent builds.

## Run

From the Harness E2E checkout:

```bash
cargo run --locked -- registry-tests \
  --url ws://127.0.0.1:49134 \
  --model "$HARNESS_E2E_MODEL" \
  --provider "$HARNESS_E2E_PROVIDER" \
  --output /absolute/new/registry-run
```

Tests 1, 2, and 3 execute concurrently. Test 4 follows the captured Test 2 delivery. Each task uses a fresh Harness session; the Test 1 plan does not affect Test 2. Ports are allocated deterministically from `--base-port` (default 45000); use a different base for simultaneous executions. No existing output directory is overwritten.

Run one task with `--test 1`, `--test 2`, or `--test 3`. To verify a previously captured implementation:

```bash
cargo run --locked -- registry-tests \
  --model "$HARNESS_E2E_MODEL" --provider "$HARNESS_E2E_PROVIDER" \
  --test 4 --implementation /absolute/previous-run/test-2/delivery \
  --output /absolute/new/registry-verification --base-port 45100
```

`--timeout-seconds` bounds each subject session (default 3600). Each subject command has a maximum 120-second timeout. Preparation and final capture have their own bounded subprocess commands. A command exit code and a subject session finishing are execution facts, not evidence that the feature is correct.

Subjects access only their task's command function. Commands start at `/workspace`, containing `registry/`, `inputs/`, and `output/`. `inputs/environment.json` records URLs, runtime requirements, and commands. In Tests 2 and 4, application containers use a built snapshot: rebuild after source edits with `/fixture/fixture.sh up`. Run project tooling inside the API/web containers; the outer container supplies shell, Git, curl, and Docker.

## Inspect evidence

`report.json` records task execution statuses, errors, transcripts, and metrics. Task directories retain source patches, fixture/input checksums, preparation logs, and `workspace/output/` reports. `delivery-status.json` records source changes, scope deviations, and capture failures without grading them.

Tests 2 and 4 also produce `screenshots/captures.json` and `screenshots/index.html`. Open the HTML file to view actual browser captures with captions. Each capture manifest records the URL, viewport, Registry base, patch hash, and fixture checksums. Missing UI or failed startup is recorded as unavailable; baseline screenshots never stand in for an implemented Changelog. Test 3 screenshots are explicitly subject-produced evidence in `workspace/output/`; the executor does not assess their contents.

All private runtime containers and their daemon volumes are removed after the task. Source, reports, and screenshots remain. To clean up after an interrupted controller process, use its materialized lifecycle script for each task directory:

```bash
python3 /absolute/run/controller-assets/lifecycle.py cleanup \
  --root /absolute/run/test-2
```

## Validate the runner without a model

```bash
python3 -m unittest discover -s tests/python -p test_registry_tasks.py -v
node --check repository-tasks/registry-version-comparison/capture.cjs
cargo test --locked registry_tasks
```

The local checks cover patch replay, new/binary/deleted/committed files, preservation of the subject's Git index, rejection of mismatched or dirty replay targets, and per-test input/mount boundaries. They do not demonstrate that a model has implemented the Registry comparison feature.
