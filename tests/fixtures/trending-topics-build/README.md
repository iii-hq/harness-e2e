# Trending topics build: stage 3 acceptance

This is the trusted test package for the proposed `trending_topics_build` v1
scenario. It is not yet registered as an executable Harness scenario. The
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
array. The alternate design uses native ordered-list numbering. The 21 negative
controls exercise a placeholder, hardcoded data, missing/duplicate topics, hidden
ranks and misleading accessible labels, incorrect DOM and CSS ordering, hidden
or incomplete/clipped content, source/back links, history and direct entry,
missing keyboard/focus support, mobile overflow and a blocking overlay.
The summary records test-package hashes and rejects changes to those files
during validation. Reports identify control variants, not model-delivery SHAs.

These are **local, trusted controls**, not model deliveries or isolation tests.
Their disposable apps share the fixture's installed dependencies through a
symlink. This driver must not execute untrusted model code. Production sandboxing,
dependency installation and remote-SHA verification belong to stage 4.

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
B02 for any model attempt. Harness execution, isolation, criterion aggregation,
technical validity/completion reporting, remote identities and missing-evidence
handling remain stages 4–5. Do not report a 100-point score from these checks.
