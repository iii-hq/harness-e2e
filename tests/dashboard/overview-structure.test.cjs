const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const repositoryRoot = path.join(__dirname, "..", "..");
const dashboardRoot = path.join(repositoryRoot, "dashboard");
const read = (...segments) =>
  fs.readFileSync(path.join(dashboardRoot, ...segments), "utf8");
const overviewPage = read("src", "pages", "OverviewPage.tsx");
const testsPage = read("src", "pages", "TestsPage.tsx");
const executionPage = read("src", "pages", "ExecutionPage.tsx");
const sectionNav = read("src", "components", "SectionNav.tsx");
const dataSource = read("src", "lib", "dashboard-data-source.ts");
const testCatalogView = read("src", "lib", "test-catalog-view.ts");
const legacyLoader = read("src", "hooks", "useLegacyPage.ts");
const overviewScript = read("public", "overview.js");
const executionScript = read("public", "execution.js");
const localRunner = read("public", "local-runner.js");
const styles = read("src", "index.css");
const publisher = fs.readFileSync(
  path.join(repositoryRoot, "scripts", "publish_harness_e2e_dashboard.py"),
  "utf8",
);

test("organizes evidence around tests, capability, and immutable executions", () => {
  for (const view of ["overview", "tests", "capability", "executions"]) {
    assert.match(sectionNav, new RegExp(`id: '${view}'`));
  }
  assert.doesNotMatch(sectionNav, /label: 'Scenarios'/);
  assert.match(overviewPage, /className="panel latest-health"/);
  assert.match(overviewPage, /className="panel capability-panel"/);
  assert.match(overviewPage, /className="panel executions-panel"/);
  assert.match(overviewPage, /Open versioned tests/);
  assert.doesNotMatch(overviewPage, /Baseline|Quality score|average_score/);
  assert.doesNotMatch(overviewScript, /Baseline|Quality score|average_score/);
  assert.match(overviewScript, /executionApi\.latestHealthModel\(latest\)/);
});

test("renders one version selector and lazy evidence surface per test", () => {
  assert.match(testsPage, /Tests across system versions/);
  assert.match(testsPage, /System version A/);
  assert.match(testsPage, /System version B/);
  assert.match(testsPage, /One row per test/);
  assert.match(testsPage, /row\.available_versions\.map/);
  assert.match(testsPage, /loadVersionResult\(row\.test_id/);
  assert.match(testsPage, /Retained evidence/);
  assert.match(testsPage, /result\?\.compatibility === 'compatible'/);
  assert.match(testsPage, /n=\{summary\.scored_runs\} scored/);
  assert.match(testsPage, /No retained evidence for/);
  assert.match(testsPage, /prefetchedCatalog/);
  assert.match(testCatalogView, /sortCatalogRows/);
  assert.match(testCatalogView, /compatibility === 'contract_changed'/);
  assert.match(testCatalogView, /compatibility === 'missing_side'/);
  assert.doesNotMatch(testsPage, /overall|baseline/i);
});

test("uses iii reads incrementally and preserves semantic errors", () => {
  assert.match(dataSource, /evaluated_versions_list/);
  assert.match(dataSource, /tests_list/);
  assert.match(dataSource, /test_version_get/);
  assert.match(dataSource, /const readCache = new Map/);
  assert.match(dataSource, /readCache\.clear\(\)/);
  assert.match(dataSource, /isTransportUnavailable/);
  assert.match(dataSource, /if \(!isTransportUnavailable\(cause\)\) throw/);
  assert.match(dataSource, /runtime\.functions\.changed_trigger/);
  assert.match(dataSource, /runtime\.functions\.execution_get/);
  assert.match(dataSource, /limit: runtime\.page_size/);
  assert.match(overviewScript, /HarnessDashboardData\.listExecutions/);
  assert.match(testsPage, /cohort_id: cohortId \|\| undefined/);
  assert.match(testsPage, /if \(!bridge\) return/);
});

test("keeps active executions pending instead of presenting them as failures", () => {
  assert.match(overviewScript, /Execution is still running/);
  assert.match(overviewScript, /Waiting for report evidence/);
  assert.match(overviewScript, /active \? "Started" : "Completed"/);
  assert.match(executionScript, /No scenario report has been published yet/);
  assert.match(
    executionScript,
    /execution\.status === "passed" \|\| active/,
  );
});

test("keeps the execution ledger and transcript usable on narrow screens", () => {
  for (const label of [
    "Execution",
    "Result",
    "Subject",
    "Scope",
    "Outcome",
    "Efficiency",
    "Evidence",
  ]) {
    assert.match(overviewScript, new RegExp(`data-label="${label}"`));
  }
  assert.match(
    styles,
    /@media \(max-width: 760px\)[\s\S]*\.execution-table thead\s*\{[^}]*display:\s*none/s,
  );
  assert.match(executionPage, /max-\[560px\]:w-screen/);
  assert.match(executionPage, /max-\[560px\]:h-dvh/);
});

test("publishes a compact index and lazy per-test shards without a suite score", () => {
  assert.match(publisher, /build_static_test_catalog/);
  assert.match(publisher, /tests_dir \/ "index\.json"/);
  assert.match(publisher, /data_dir \/ shard_name/);
  assert.match(publisher, /"median_score": _median\(scores\)/);
  assert.doesNotMatch(publisher, /"average_score"/);
});

test("keeps execution score attached to scenario evidence only", () => {
  assert.match(executionPage, /Scenario pass rate/);
  assert.match(executionPage, /Model cost/);
  assert.match(executionPage, /Model runtime/);
  assert.doesNotMatch(executionPage, /Quality score|detail-score/);
  assert.doesNotMatch(executionScript, /average_score|detail-score/);
  assert.match(executionScript, /scenario-detail-card\.is-focused/);
  assert.match(executionPage, /session-transcript-dialog/);
  assert.match(executionScript, /openConversationDialog/);
  assert.match(executionPage, /AssessmentWorkspace/);
  assert.match(executionPage, /Objective results, advisory interpretations/);
  assert.match(executionScript, /harness:execution-detail-ready/);
  assert.match(executionScript, /HARNESS_EXECUTION_DETAILS\[execution\.id\]/);
});

test("offers every semantic execution status as a filter", () => {
  for (const status of [
    "passed",
    "hard_gate_failed",
    "technical_failed",
    "infra_failed",
    "incomplete",
    "cancelled",
    "running",
  ]) {
    assert.match(overviewPage, new RegExp(`<option value="${status}">`));
  }
});

test("discovers local models lazily and keeps advanced runner knobs contained", () => {
  assert.match(overviewPage, /id="local-runner-dialog"/);
  assert.match(overviewPage, /id="open-local-runner"[^>]+hidden/);
  assert.match(overviewPage, /id="local-subject-picker"/);
  assert.match(overviewPage, /id="local-judge-picker"/);
  assert.match(overviewPage, /id="local-scenario-options"/);
  assert.match(overviewPage, /className="local-advanced local-field-wide"/);
  assert.match(localRunner, /getCatalog/);
  assert.match(localRunner, /function positionModelPicker\(picker\)/);
  assert.match(localRunner, /popoverRect\.height > availableBelow/);
  assert.match(localRunner, /local-model-picker-up/);
  assert.match(overviewPage, /Choose judge model/);
  assert.match(localRunner, /elements\.scenarioPicker\.open = false/);
  assert.match(localRunner, /elements\.advanced\.open = false/);
  assert.match(
    styles,
    /\.local-model-picker\.local-model-picker-up \.local-model-popover\s*\{[^}]*bottom:\s*calc\(100% \+ 6px\)/s,
  );
  assert.match(
    styles,
    /@media \(max-width: 560px\)[\s\S]*\.local-model-popover[\s\S]*position:\s*fixed/s,
  );
});

test("gives mobile test actions distinct labels", () => {
  assert.match(testsPage, /data-mobile-label="New"/);
  assert.match(testsPage, /data-mobile-label="Coverage"/);
  assert.match(testsPage, /comparable test\$\{compatibleCount === 1/);
});

test("loads local runner code only for local manifests", () => {
  assert.match(
    legacyLoader,
    /page === 'overview' && window\.HARNESS_EXECUTIONS\?\.mode === 'local'[\s\S]*loadScript\(page, 'ansi-log\.js'\)[\s\S]*loadScript\(page, 'local-runner\.js'\)/s,
  );
  assert.match(overviewScript, /if \(isLocal\) window\.HarnessLocalRunner\.initialize\(\)/);
  assert.match(publisher, /"mode": "published"/);
});

test("keeps hidden diagnostic controls out of layout", () => {
  assert.match(overviewPage, /id="preview-badge"[^>]+hidden/);
  assert.match(styles, /\[hidden\]\s*\{[^}]*display:\s*none\s*!important;/s);
  assert.match(styles, /@import "tailwindcss" important/);
  assert.match(styles, /@theme inline/);
});
