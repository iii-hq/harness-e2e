# Trending topics build: acceptance and runtime

This is the trusted test package for the `trending_topics_build` v1 scenario,
registered as a native Harness scenario in the diagnostics catalog. The
[functional contract](../../../docs/blog-build-contract.md) defines the task;
the public skeleton and development tests live in
[e2e-fixture PR #6](https://github.com/iii-hq/e2e-fixture/pull/6).

Keep this entire package, its reference solution and its varied input outside
the model's workspace, remote and accessible Git object store. Only the fixture's
`trending-topics-build/app/` tree is given to the model.

## Run the known controls

Use Node 24.18.0 and npm 11.6.2. Install the fixture's frozen dependencies in its
app directory with `npm ci`, then run from this package:

```bash
npm ci
npx --no-install playwright install chromium
node validate-controls.mjs /absolute/path/to/e2e-fixture/trending-topics-build/app /absolute/path/to/new-control-output
```

The output directory must not exist and must be outside the fixture/evaluator.
Port 4187 must be free. The driver creates disposable copies, builds each known
control, starts and stops its preview, and runs this package's Playwright CLI
and configuration independently of the application. It retains build/server/test
logs, JSON and HTML reports, failure traces and screenshots. `summary.json`
records whether each control had its intended outcome. A nonzero process exit
alone is not accepted as a successfully detected defect.

Four positive controls combine two different designs with the original and
deterministically varied feeds. A fifth checks article line breaks rendered with
`<br>`. Variation preserves six topics and the schema,
but changes edition, IDs, nonsequential ranks, text and URLs and shuffles the
array. The alternate design uses native ordered-list numbering. The 22 negative
controls exercise a placeholder, hardcoded data, missing/duplicate topics, hidden
ranks and misleading accessible labels, incorrect DOM and CSS ordering, hidden
or incomplete/clipped content, source/back links, history and direct entry,
missing keyboard/focus support (including animated links without a focus indicator),
mobile overflow and a blocking overlay.
The summary records test-package hashes and rejects changes to those files
during validation. Reports identify control variants, not model-delivery SHAs.

These are **local, trusted controls**, not model deliveries or isolation tests.
Their disposable apps share the fixture's installed dependencies through a
symlink. This driver must not execute untrusted model code. The isolated lifecycle
below provides dependency installation and remote-SHA verification for attempts.

## Acceptance coverage

Each data/design case runs 82 checks across desktop 1440 × 900 and mobile
390 × 844 in Chromium: 80 criterion observations and two evidence captures.
All six topics are exercised; no result is inferred from only the first article.

| Criterion | Independent observations |
| --- | --- |
| B03 | Edition, exact six title links, destinations, associated visible ranks |
| B04 | Ascending rank in DOM and visual reading order |
| B05 | Correct heading and complete visible article body |
| B06 | Home/title/return links and browser Back |
| B07 | Direct entry and reload for every article |
| B08 | Exact accessible Source link and supplied URL |
| B09 | Main/h1/document title, real Tab and Enter, visible-focus indicator |
| B10 | Page width, link bounds and unobstructed interactions |

Tests use published accessible names, native links and observable layout; they
do not require `data-testid`, a component structure or particular CSS classes.
There is no screenshot diff, prescribed design or aesthetic score.

The public brief defines visible rank as text or native ordered-list numbering.
Visual reading order is top-to-bottom, then left-to-right for vertically
overlapping title rectangles. CSS-counter-only ranks are not part of this contract.
The focus check observes common style changes on a link, its isolated ancestors
and pseudo-elements. It does not establish WCAG contrast conformance or recognize
every possible animated/masked effect. Body checks reject hidden text and common
vertical clipping; they are not a general proof of visual legibility. Screenshots
remain available for inspection without assigning aesthetic points.

## Run against an already started instance

The test package does not start a server or load application-owned test code.
Provide all of the following explicitly:

```bash
TT_BASE_URL=http://127.0.0.1:4173 \
TT_FEED_PATH=/absolute/path/to/evaluated/content/feed.json \
TT_OUTPUT_DIR=/absolute/path/to/new-evidence-directory \
TT_DATASET=original \
TT_COMMIT_SHA=full-delivered-sha \
npm test
```

Repeat with the fixed varied feed after rebuilding a disposable copy of the same
code, labeled `TT_DATASET=varied`. External browser requests are aborted; Source
activation is observed as a request without fetching the destination. This
network rule is for deterministic browser checks, not a security sandbox.

The unweighted evidence test captures home and then the lowest-rank article
through an actual title-link click in each viewport. Each PNG is attached to the
report with a JSON record of dataset, commit/control marker, exact feed hash,
URL, viewport, browser version and image hash. The original feed is preserved
byte-for-byte. Varied-feed captures are additional evidence, not original-delivery
screenshots. Failed tests retain their own screenshots/traces; failed captures
are not replaced by older images.

B01 (Git delivery/scope/ancestry/workspace) and B02 (frozen install/build/start)
are **not assessed by this suite**. The controls' successful builds do not prove
B02 for any model attempt. The native lifecycle below supplies these observations;
do not report a 100-point score from the browser suite alone.

## Isolated Harness lifecycle

`src/scenarios/trending_topics_build.rs` embeds only the runtime/evaluator assets,
registers one attempt-specific execution tool, captures the independent result
before cleanup, and maps observed B01–B10 outcomes to the contract weights.
The subject can access that tool and function discovery only. It receives the
public task and must clone, implement, commit and push branch `build` itself.

Controller prerequisites are Linux amd64, Docker, Git, Python 3 and Node, plus
read access to the pinned private fixture repository. Image preparation needs
registry/package access; application execution does not. The Dockerfile pins
Node and Playwright base-image digests and the frozen public dependency lock.
Each attempt records the resolved runtime image ID. The first preparation builds
the runtime image; subsequent attempts reuse its content-derived local tag.

`lifecycle.py prepare --root /absolute/path/to/new-attempt` exports only the
fixture's app tree to a fresh bare remote. Separate non-privileged containers
run the Git service and subject workspace. They share only a network namespace
with no external route; the subject sees Git over loopback, not the bare remote
filesystem, controller credentials, Docker socket, evaluator or reference.
Containers have read-only roots, dropped capabilities, no-new-privileges and
CPU, memory, process and temporary-filesystem limits. This is container isolation,
not a claim that arbitrary browser/kernel exploits are impossible.

`lifecycle.py exec --root /absolute/path/to/attempt --command '...' --timeout-ms 120000`
executes in `/workspace`, retains bounded command logs and returns the command's
exit status. `finish` stops the subject and Git service before resolving the
remote SHA once. It validates every delivered commit and inspects the stopped
workspace with a fresh index, then independently checks out that SHA. Each
dataset is installed and built in a network-isolated container. Another container
runs the trusted Playwright CLI against a read-only preview; it cannot load the
application's test configuration or read its filesystem. Original feed bytes are
preserved and the varied build uses the same delivered code.

`result.json` retains criterion observations, infrastructure errors, the delivery
SHA/image identity and dataset report paths. Partial progress remains incomplete.
JSON/HTML browser reports, PNGs, traces and command/build/preview logs remain under
the attempt root. `cleanup` removes only containers with that attempt's unique
label; it preserves the remote and evidence. An interrupted attempt is not resumed:
clean up its resources and prepare a new attempt directory.

Run the lightweight controller/evaluator checks without Docker:

```bash
node --test tests/fixtures/trending-topics-build/evaluator.test.mjs
python3 -m unittest -v tests/fixtures/trending-topics-build/test_lifecycle.py
cargo test --locked trending_topics_build
```

Known criterion outcomes appear in the native Harness assessment even when
dependent checks cannot run. Failed product prerequisites remain product failures;
unverified criteria retain their cause and receive no invented points. The
aggregate score stays unavailable until all scored criteria are observed.
Infrastructure failures preserve prior observations while invalidating the run.
Playwright process exits must agree with the report; a nonzero exit is not itself
proof of a product defect.

The captured Harness JSON deliverable contains a portable file bundle with UTF-8
or base64 content, byte counts and SHA-256 hashes. Original-feed home/article PNGs
for both viewports are verified and prioritized within the existing 16 MiB asset
limit. Varied-feed captures, reports, inputs, metadata, traces and logs use the
remaining budget; omissions include explicit reasons. Capture verifies file paths,
image bytes and their commit/feed/route/viewport associations before cleanup.
Missing or corrupt required captures fail closed; images are not substituted.

The focus check pauses pre-existing animations during its style comparison and
then restores them. This prevents incidental animation from satisfying B09 without
a focus indicator, while preserving design freedom and avoiding aesthetic scoring.

## Historical stage-3 control run

Local validation completed on 2026-09-10 UTC with acceptance code at
`ba868a5bffa63b7f068c2281384a3887bd562cb8` and public fixture commit
`3ee24f7ace3c014db35423f14939ad3f6ce0c3d2`:

- All 26 controls had their expected outcome: five positive cases passed
  **410/410** private checks; all **21** deliberate defects were detected.
- The evaluator files remained unchanged throughout the run; their hashes and
  individual observations are retained in `summary.json`.
- The reference passed **6/6** public checks. The placeholder passed the two
  smoke checks and failed the four product checks as intended.
- Fresh-remote preparation tests passed **2/2** after the fixture update.
- All **20** positive-control PNG hashes and associated feed hashes were
  independently verified (eight original-feed and twelve varied-feed captures).
  Both designs were visually inspected at desktop and mobile widths.
- Runtime: Node 24.18.0, npm 11.6.2, Chromium 151.0.7922.34, Playwright 1.62.1.

Full local evidence is retained at
`/home/layon/workspaces/trending-topics-stage3-controls-final/`, including
`summary.json` and per-case reports under `results/`. This machine-local output
is not a remotely published CI artifact; use the command above to reproduce it.
No model run, production sandbox or full Harness score is claimed.

## Qualified native model run

Stages 5–6 are complete. The [qualification record](../../../docs/blog-build-contract.md#stage-6-qualification-and-release-control-integration)
records one real model run with B01–B10 passed, **100/100**, **164/164** independent
browser checks, eight verified portable screenshots and scoped container cleanup.
Reports are retained locally at
`/home/layon/workspaces/trending-topics-model-validation-gnp5Qu/report/`.
This is local execution evidence, not a remotely published CI artifact.

Reproduce against a compatible existing stack using a newly built PR binary:

```bash
III_NAMESPACE=my-project \
HARNESS_E2E_RUN_DIR=/absolute/path/to/new-runtime-root \
target/debug/harness-e2e run \
  --url ws://127.0.0.1:49134 \
  --model deepseek-v4-flash --provider deepseek \
  --scenario trending_topics_build --runs 1 --technical-retries 0 \
  --output /absolute/path/to/new-report-directory
```

Set the namespace, address and available model/provider for the target stack.
The report's deliverable artifact contains the portable evidence bundle; its file
entries include `encoding`, `content` and `sha256`. Use `base64` decoding for PNGs
and traces. The remote SHA and all criterion observations are in the same artifact.

The Evolution profile now includes a standalone trending-topics build group with
three repetitions and no technical retries. Its workflow prepares the pinned private
fixture before execution. A published runner containing this revision is required
before Release Control can execute the new profile; no campaign was dispatched here.
