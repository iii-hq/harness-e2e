# `trending_topics_build` v1 contract

Status: stages 1–3 delivered; stage-4 runtime positive path validated locally,
with evaluator readiness work remaining before Release Control admission.
The product is a blog about trending topics, as confirmed by the user.
The task brief, application labels and documentation are in English.

## Objective

Evaluate the construction of a usable web application from a fixed Git baseline,
with delivery to the run's remote repository and independent validation of the
delivered commit. This document defines the contract. Fixture preparation,
application implementation, executable tests and Harness integration belong to
the subsequent stages.

## Product and scope

Build a blog for discovering and reading trending topics. Relevance comes from
the fixture's ranking; this version does not fetch live trends.

The agent receives an executable skeleton and six fixed synthetic topics. It
implements the interface and navigation using the supplied data. Evaluation
covers product construction and data rendering; research, summary writing and
editorial selection of the top three topics are not required.

Article publishing and editing belong to the future editorial scenario, which
will receive a pinned reference application independently of any build attempt.
Comments, authentication, search, filters, pagination, RSS and production
deployment are outside v1.

## Input data

Reuse the `trends/feed.json` format consumed by the existing
[`trend_blog`](../src/scenarios/trend_blog.rs) scenario, materialized as
`content/feed.json` in the new fixture:

| Field | Application use |
| --- | --- |
| `edition` | Identifies the edition on the home page |
| `topics` | The six topics the application must display |
| `id` | Stable identifier used in the article URL |
| `rank` | Relevance: smaller values come first |
| `title` | Topic title on the home and article pages |
| `source_body` | Complete text rendered on the article page |
| `url` | Source-link destination |

`edition` and `topics` belong to the root object; the remaining fields belong to
each topic. IDs and ranks are unique. IDs are valid URL path segments. The
application must consume the file; titles, text and ordering must not be
hardcoded copies in the UI implementation.

Stage 2 fixes the edition's values and hash. The evaluator may also shuffle the
array and substitute valid IDs, ranks, titles, bodies and URLs in a disposable
evaluation checkout, always retaining six topics and the same schema. This data
variation must be disclosed in the public brief. It checks data consumption and
sorting by `rank`. The agent may not modify the supplied file.

## Pages, routes and interactions

| Page | Route | Requirements |
| --- | --- | --- |
| Home | `/` | Show the heading “Trending topics”, the edition and all six topics exactly once, ordered by ascending `rank`. Each item shows its rank and title; the title links to its article page. |
| Article | `/posts/:id` | Show the topic's title as the main heading, its complete `source_body`, a link named “Source” with the supplied `url` as its `href`, and an “All topics” link to `/`. |

Required flows are opening a topic from the home page, reading its content and
returning through “All topics”. Browser Back must also return to the home page
when it was the entry point. Each article URL must work on direct navigation and
after reload.

All six topics must have distinct pages displaying their own data. Visible text
must preserve the source's words and punctuation; HTML whitespace and line-break
differences are allowed. Source links are checked by destination without depending
on external site availability.

Required controls use the accessible names above; topic links use their titles.
They must be real links, reachable with Tab, activated with Enter, and have
visible focus. Each page has an identifying document title, one main landmark
and one level-one heading: the blog name on the home page and the topic title on
the article page.

Run the flows in Chromium at **1440 × 900** and **390 × 844** CSS pixels. Vertical
scrolling is allowed. The page must not overflow horizontally. Required links
remain visible after scrolling and actionable without obstructing elements.
Typography, colors and visual composition are the implementation's choice.
Ranks must be observable as visible text or native ordered-list numbering.
Visual reading order is top-to-bottom, then left-to-right within a row; these
rules are also disclosed in the public brief.

## Inputs and edit boundaries

The public brief must specify:

- Run remote repository, initial SHA, branch and authorized push destination.
- This contract's functional requirements, data schema and permitted variations,
  routes and accessible control names.
- Installation, build, public-test and server-start commands.
- Writable and protected paths.

The agent may modify application code and assets, add its own tests and document
its delivery within the allowed paths. Supplied data and reference public tests
are protected. The skeleton supplies sufficient dependencies for the published
requirements; versions, lockfile and evaluation commands are fixed during fixture
preparation.

Requirements and public data remain available to the agent. Final tests,
reference implementation and additional evaluation datasets are controlled by
the evaluator outside the agent's environment. The run remote contains only the
public baseline and attempt commits; it does not expose the reference solution's
objects, branches or history.

## Git delivery

The agent must clone the supplied baseline, implement the application, run
available tests and produce at least one non-empty implementation commit.
Additional meaningful commits are allowed; no arbitrary count or exact commit
message is required.

The delivered branch must descend from the initial SHA, contain the attempt's
non-merge commits and be published to the authorized remote. Leave the workspace
clean, including no untracked implementation files. Changes must respect the
allowed paths.

On completion, the agent reports the branch, final SHA and a concise test summary.
The runner resolves the remote ref and freezes the observed SHA for evaluation,
checking the agent's claims against that evidence. Workspace cleanliness is
checked in the attempt workspace before cleanup, and its local HEAD must equal
the delivered remote SHA. A clean verifier clone does not prove that the agent's
workspace was clean.

## Independent acceptance

| ID | Points | Behavior to verify | Expected evidence |
| --- | --- | --- | --- |
| B01 | 20 | Authorized remote delivery descending from the baseline, with allowed changes and a clean workspace at the same SHA | Remote ref, ancestry, commits, diff and workspace status |
| B02 | 10 | The delivered checkout installs with the frozen lockfile, builds and starts | Logs and exit codes from the fixture-defined commands |
| B03 | 15 | Home shows the edition and exactly six topics with their titles and ranks | Visible-interface assertions with original and varied data |
| B04 | 10 | Visual and navigation order follows ascending `rank` | Topic sequence in the DOM and on screen, including shuffled input |
| B05 | 15 | Each article shows its correct title and complete body | Content assertions for all six IDs with original and varied data |
| B06 | 10 | Opening a topic and returning works through links and browser Back | Playwright flows from home to every topic |
| B07 | 5 | Direct navigation and reload preserve the correct page | Independent navigation to all six URLs and reload |
| B08 | 5 | Every article exposes “Source” with the correct destination | Accessible name and `href` compared with input data |
| B09 | 5 | Semantics, titles, accessible names, keyboard and focus meet the contract | Semantic assertions and Tab/Enter navigation |
| B10 | 5 | Required flows work in both viewports without horizontal overflow or obstructed controls | Desktop/mobile execution and layout checks |

Total: **100 points**. Each criterion receives its weight when all corresponding
checks pass; an observed failure removes only that criterion's points.
Unexecuted checks remain identified as unverified with cause and coverage.
Individual test observations remain available even when their aggregate
criterion fails.

Every final test must correspond to a published requirement. Its private
implementation must not introduce undisclosed features, hardcoded values or
selector conventions. Data variation is deterministic and fixed by case/version;
it does not depend on current news or execution time.

The runner checks out the fixed remote SHA, builds and starts the application in
an environment isolated from evaluator tests and artifacts. The final Playwright
suite runs against that instance. Agent-written tests provide development
feedback; acceptance relies on independent execution.

## Evidence and results

Using the original data, capture home and the lowest-rank article in each
viewport: four full-page screenshots per execution that can reach those states.
Capture the article after real navigation through its home-page title link.
These images come from the clean checkout of the delivered SHA.

Record run, evaluated SHA, data hash, route, viewport, browser version and image
hash. Tests, logs, screenshots and traces identify the execution, commit and
inputs. Checks using varied data are labeled as additional evaluation of the
same code with a different input hash; the modified checkout is not presented as
the original delivery.

Screenshots allow visual inspection. There is no pixel-perfect comparison or
subjective aesthetic score in this version. Tests verify the explicit functional
and layout requirements.

Persist available results and evidence when tests fail. If the application or
browser fails to start, record the cause and missing capture; never substitute an
older screenshot as evidence for the attempt.

The report preserves individual checks and distinguishes observed failures from
unexecuted checks. Measured points, task completion and technical validity follow
the current Harness contract in the [README](../README.md). A failure does not
erase prior observations or automatically zero the complete score. Task
completion requires the delivery and product requirements and is reported
separately from measured points. Evidence capture is the runner's responsibility
and has no independent weight in the agent's score. Evaluation-environment
failures must be distinguished from application defects.

## Stage 1 completion

This contract defines product, data, pages, routes, interactions, criteria and
evidence. The proposed scenario identifier is `trending_topics_build`, version 1.
The existing `trend_blog` retains its identity and history; its score is not
directly comparable with this new build scenario.

Stage 2 prepares the skeleton and remote, fixes dependencies and commands,
materializes the feed and publishes the allowed paths. Stage 3 implements the
public and private tests and checks them against a reference solution and known
defects. This document itself is not an executable scenario.

## Stage 2 delivery

The fixture is published in [e2e-fixture PR #6](https://github.com/iii-hq/e2e-fixture/pull/6)
at commit `723aeceaa4ac7f4ebcf85a808a61fa0749e4125e`, under
[`trending-topics-build/`](https://github.com/iii-hq/e2e-fixture/tree/723aeceaa4ac7f4ebcf85a808a61fa0749e4125e/trending-topics-build).
At stage 2, the PR was a draft and had not been merged into the default branch.

The implementation provides the English task brief and placeholder, the frozen
feed, Vite/JavaScript tooling, public browser smoke checks and a preparer that
exports only the app tree into a fresh one-commit remote per attempt. The initial
app commit is `b59c7741d1691d92cc2142c91eb74816a189221d`; the work branch is `build`.

Runtime versions are Node 24.18.0, npm 11.6.2, Vite 8.0.13 and Playwright 1.62.1.
Writable paths are `src/**`, `public/**`, `index.html`, `tests/agent/**` and the
optional `DELIVERY.md`. The remaining task inputs and public checks are protected.

Local validation passed: two preparation tests, a real loopback Git
clone/commit/push with independent replay of the delivered SHA, rejection of an
off-branch push, and frozen install/build plus two public Playwright smoke checks
in the fresh verifier checkout. Both English baseline screenshots were inspected.
The temporary Git service was stopped after validation.

These results establish baseline environment and Git transport readiness. Full
product acceptance, a reference solution, Harness integration and model-driven
execution remain subsequent stages.

## Stage 3 delivery

The public fixture now includes product checks in addition to the smoke tests,
at commit `3ee24f7ace3c014db35423f14939ad3f6ce0c3d2` in the same draft PR #6.
Its app tree is `78bced7344359876226012a786c45cec58649280`, producing the
new initial app commit `76a23abebff553a8ccf1397bd2f991c273bac03c`. Feed and
lockfile bytes are unchanged. The stage-2 identities above remain historical;
new runs must use this stage-3 baseline.

The [trusted acceptance package](../tests/fixtures/trending-topics-build/README.md)
contains independent Playwright tests, two reference styles and a reproducible
positive/negative control driver. It checks B03–B10 in both viewports and retains
screenshots, logs, JSON/HTML reports and failure traces outside application
copies. The public brief discloses observable ranks and visual reading order;
neither reference style is a required design.

The placeholder intentionally passes two smoke tests and fails four public
product tests. The reference passes all six public checks. Preparation tests
pass 2/2 after the new fixture commit. Final private-control validation is
recorded in the trusted package's validation notes.

All five positive controls passed 410 private checks, and all 21 deliberate
defects were detected. The trusted implementation is published for review in
[Harness PR #126](https://github.com/iii-hq/harness-e2e/pull/126). Both PRs were
drafts at stage 3; these are local results, not a claim of CI or model success.

This stage does not register a runnable Harness scenario, run a model, grade
B01/B02, establish production isolation or publish a blog post. Those boundaries
remain assigned to stages 4–6 and the separate future editorial scenario.

## Stage 4 delivery

The native `trending_topics_build` v1 scenario is registered in the diagnostic
catalog, with an attempt-specific execution tool and trusted setup, capture,
evaluation and cleanup hooks. It keeps `trend_blog` unchanged. Its case identity
includes the fixture revision, baseline and embedded runtime/evaluator hashes.

The [runtime package](../tests/fixtures/trending-topics-build/README.md) creates a
private per-attempt Git service and restricted application containers. Only the
public app tree reaches the subject. Finalization stops subject writes, resolves
the pushed SHA once, validates commit history and workspace cleanliness, then
builds and tests independent checkouts against original and varied inputs.

A local trusted-reference control completed on 2026-09-10 UTC at
`/home/layon/workspaces/trending-topics-stage4-validation-trUv1e/attempt/`:

- Delivered commit: `81ec4b5168bce3817d42062a3db7fb8ad342f44c`; clean local
  `build` branch and matching remote SHA, descended from the pinned baseline.
- Frozen offline install/build and public tests: 6/6 passed.
- Independent private tests: 82/82 per dataset, 164/164 total, with B01–B10
  reported passed and no infrastructure errors.
- All eight screenshot hashes, feed hashes and delivered-SHA associations
  were independently checked. Original desktop/mobile home captures were inspected.
- Runtime image: `sha256:3ec84aeb7c19f7168d21a7eee967aecc1cdfc1674242aee399fe71eebb28a10e`.
- No external route, controller workspace, evaluator, remote filesystem or
  Docker socket was visible in the subject container. Attempt containers were
  removed after capture; the remote and evidence were retained.

This was a lifecycle control using the reference implementation, not a model or
iii execution, and not a remotely published CI artifact. Portable capture, partial
native assessment and model-driven qualification were deferred to stages 5–6.

## Stage 5 delivery

The native Harness assessment now preserves each known pass/failure and leaves
unverified criteria without invented points. A product build failure or protected
input violation remains a product failure even if dependent tests cannot run.
Incomplete scored coverage has no aggregate total; genuine infrastructure failures
invalidate the run without discarding prior criterion observations.

The captured deliverable is a portable, bounded JSON file bundle using the
existing Harness asset contract. Screenshot bytes and metadata are checked for
matching delivery SHA, frozen input, route, viewport and hashes. The four original
screenshots have priority; varied screenshots and supporting files use the remaining
16 MiB capture budget, with explicit omission reasons. Required evidence that is
missing or corrupt fails closed.

Readiness fixes reconcile Playwright process exits with report contents, reject
animated links without focus indicators, and stop timed-out build/evaluator
containers before evidence finalization. Regression tests cover these paths and
the actual native finalizer's partial-assessment behavior. The full expanded
control run passed 27/27 expected outcomes, including 22 deliberate defects;
final animation-restoration checks passed all 14 positive B09 observations and
rejected both animated-no-focus observations. These remain local trusted controls,
separate from the model execution recorded below.

## Stage 6 qualification and Release Control integration

One real `deepseek/deepseek-v4-flash` execution completed on 2026-09-10 UTC
against the existing iii stack in namespace `my-project`. It used the PR's local
runtime/evaluator sources, whose hashes are recorded in the materialized case.
The model received only the public task and isolated execution tool; it did not
receive the reference implementation, hidden tests or varied feed.

- Harness run `77abfae7ca9445c8b1cff06095f670a3`, attempt
  `3fbbf61fad22479cb93f1e4335f6d0b9`: technically valid, completed, **100/100**.
- Delivered SHA: `752158867fdaf2dcb9703bfece6f3d85a0230c26`; clean matching
  local and remote `build` branch, with B01–B10 passed.
- Independent original and varied builds/startups succeeded; private Playwright
  checks passed **82/82 per dataset**, **164/164 total**.
- The archived deliverable contains **88 files**, including **eight PNGs**,
  without omissions or capture-verification errors. All bundled file hashes were
  independently rechecked after cleanup; desktop/mobile home screenshots were inspected.
- Archived artifact: **923,794 bytes**, SHA-256
  `049408984617c630dd26da5dec91e854189d0eff4644c73925a500977bff1dc8`.
- Attempt containers were removed; remote, logs, reports and portable capture remain
  under `/home/layon/workspaces/trending-topics-model-validation-gnp5Qu/`.

The scenario is included as standalone group `case-trending-topics-build` in the
Evolution profile: three repetitions, no technical retries. Evolution now has
23 cases and 69 planned runs. The exact-stack workflow checks out the immutable
private fixture using a read-only GitHub App token without persisting credentials,
then routes the controller fetch to that local checkout.

The Software engineering profile also selects the same build case as an
independent group, with one repetition and no technical retries. It contains
13 cases and 13 planned runs across 12 groups, including the Linkly tutorial and preserving the ordered Registry
implementation/verification pair. The fixture and evaluation contract are shared
with Evolution; no new application benchmark or model qualification is implied.

This qualification is a local model run, not a comparative benchmark or a Release
Control campaign. Remote CI validates the implementation; enabling execution in
Release Control still requires merging the changes and publishing/selecting a
runner containing them. This work does not merge PRs, release a runner, dispatch
a campaign or publish a blog post.
