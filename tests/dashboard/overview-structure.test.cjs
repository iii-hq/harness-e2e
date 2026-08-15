const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const repositoryRoot = path.join(__dirname, "..", "..");
const dashboardRoot = path.join(repositoryRoot, "dashboard");
const index = fs.readFileSync(
  path.join(dashboardRoot, "src", "pages", "OverviewPage.tsx"),
  "utf8",
);
const execution = fs.readFileSync(
  path.join(dashboardRoot, "src", "pages", "ExecutionPage.tsx"),
  "utf8",
);
const executionScript = fs.readFileSync(
  path.join(dashboardRoot, "public", "execution.js"),
  "utf8",
);
const sampleExecutions = fs.readFileSync(
  path.join(dashboardRoot, "public", "sample-executions.js"),
  "utf8",
);
const overview = fs.readFileSync(
  path.join(dashboardRoot, "public", "overview.js"),
  "utf8",
);
const localRunner = fs.readFileSync(
  path.join(dashboardRoot, "public", "local-runner.js"),
  "utf8",
);
const styles = fs.readFileSync(
  path.join(dashboardRoot, "src", "index.css"),
  "utf8",
);
const loader = fs.readFileSync(
  path.join(dashboardRoot, "src", "hooks", "useLegacyPage.ts"),
  "utf8",
);
const publisher = fs.readFileSync(
  path.join(repositoryRoot, "scripts", "publish_harness_e2e_dashboard.py"),
  "utf8",
);

test("places latest health and comparison before deeper evidence surfaces", () => {
  const latest = index.indexOf('className="panel latest-health"');
  const comparison = index.indexOf('className="panel overview-comparison"');
  const matrix = index.indexOf('className="panel health-panel"');
  const capability = index.indexOf('className="panel capability-panel"');
  const efficiency = index.indexOf('className="panel efficiency-overview"');
  const executions = index.indexOf('className="panel executions-panel"');

  assert.ok(latest >= 0);
  assert.ok(comparison > latest);
  assert.ok(capability >= 0);
  assert.ok(efficiency >= 0);
  assert.ok(matrix >= 0);
  assert.ok(executions >= 0);
  assert.match(overview, /executionApi\.latestHealthModel\(latest\)/);
  assert.match(
    overview,
    /"\.latest-evidence",[\s\S]*"\.overview-comparison",[\s\S]*"\.health-panel",[\s\S]*"\.efficiency-overview",[\s\S]*"\.capability-panel",[\s\S]*"\.executions-panel"/,
  );
  assert.match(overview, /No report denominator/);
});

test("exposes evidence and policy separately in the capability frontier", () => {
  assert.match(index, /id="capability-reliable-tier"/);
  assert.match(index, /id="capability-statistical-tier"/);
  assert.match(index, /id="capability-sample-size"/);
  assert.match(index, /id="capability-body"/);
  assert.match(overview, /p95 cost and wall-time budgets are not fully configured/);
  assert.match(overview, /rateWithInterval/);
});

test("keeps the latest result inside the efficiency overview", () => {
  assert.match(index, /className="efficiency-result"/);
  assert.match(index, /id="efficiency-status"/);
  assert.doesNotMatch(index, /id="kpi-status"/);
});

test("exposes baseline and candidate slots in the overview", () => {
  assert.match(index, /id="overview-comparison-left"/);
  assert.match(index, /id="overview-comparison-right"/);
  assert.match(index, /id="overview-comparison-verdict"/);
  assert.match(overview, /initializeOverviewComparison/);
  assert.match(overview, /comparison\.compatibility === "eligible"/);
  assert.match(overview, /Delta disabled/);
});

test("keeps hidden preview and diagnostic controls out of layout", () => {
  assert.match(index, /id="preview-badge"[^>]+hidden/);
  assert.match(styles, /\[hidden\]\s*\{[^}]*display:\s*none\s*!important;/s);
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
    assert.match(index, new RegExp(`<option value="${status}">`));
  }
});

test("restores the per-run chat transcript surface", () => {
  assert.match(loader, /execution-transcript\.js/);
  assert.match(execution, /session-transcript-dialog/);
  assert.match(executionScript, /renderConversationLaunch/);
  assert.match(executionScript, /conversation-open/);
  assert.match(executionScript, /openConversationDialog/);
  assert.match(executionScript, /id: "prompt", label: "Prompt"/);
  assert.match(executionScript, /id: "sessions", label: "Sessions"/);
  assert.match(executionScript, /Complete run record/);
  assert.match(sampleExecutions, /availability: index < 3 \? "full"/);
  assert.match(sampleExecutions, /transcript:\s*\{/);
  assert.match(sampleExecutions, /criteria:\s*\[/);
  assert.match(sampleExecutions, /traces:\s*\{/);
});

test("uses the delta meaning to color efficiency sparklines", () => {
  assert.match(overview, /const efficiencyTrendColors =/);
  assert.match(overview, /efficiencyTrendColors\[meta\.css\]/);
  assert.doesNotMatch(overview, /const palette =/);
});

test("discovers local models and scenarios while keeping runner knobs advanced", () => {
  assert.match(index, /id="local-runner-dialog"/);
  assert.match(index, /id="open-local-runner"[^>]+hidden/);
  assert.match(index, /id="local-subject"[^>]+disabled/);
  assert.match(index, /id="local-subject-picker"[^>]+local-picker-disabled/);
  assert.match(index, /id="local-subject-search"[^>]+type="search"/);
  assert.match(index, /id="local-subject-options"[^>]+role="listbox"/);
  assert.match(index, /id="local-judge-picker"[^>]+local-picker-disabled/);
  assert.match(index, /id="local-judge-search"[^>]+type="search"/);
  assert.match(index, /id="local-judge-options"[^>]+role="listbox"/);
  assert.match(
    index,
    /<details[^>]+id="local-scenario-picker"[^>]+className="local-scenario-picker"[^>]+open/,
  );
  assert.match(index, /Scenarios <small>select one or more<\/small>/);
  assert.match(index, /id="local-scenario-options"/);
  assert.match(index, /className="local-advanced local-field-wide"/);
  assert.match(index, /id="local-catalog-refresh"/);
  assert.doesNotMatch(index, /name="model"|name="provider"/);
  assert.match(localRunner, /api\/local\/catalog/);
  assert.match(localRunner, /catalog\.models/);
  assert.match(localRunner, /catalog\.scenarios/);
  assert.match(localRunner, /className = "local-model-provider"/);
  assert.match(localRunner, /normalizedSearch\(button\.dataset\.search\)/);
  assert.match(localRunner, /providerGroup\.open = query/);
  assert.match(localRunner, /Automatic · use subject model/);
  assert.match(localRunner, /function positionModelPicker\(picker\)/);
  assert.match(localRunner, /popoverRect\.height > availableBelow/);
  assert.match(localRunner, /local-model-picker-up/);
  assert.match(
    styles,
    /\.local-model-picker\.local-model-picker-up \.local-model-popover\s*\{[^}]*bottom:\s*calc\(100% \+ 6px\)/s,
  );
  assert.match(localRunner, /input\.checked = previous\.has\(scenarioId\)/);
  assert.doesNotMatch(localRunner, /const selectAll/);
  assert.doesNotMatch(localRunner, /scenarioPicker\.addEventListener\("toggle"/);
});

test("uses the native local-run contract without import compatibility", () => {
  assert.match(overview, /Last completed/);
  assert.match(localRunner, /Results saved/);
  assert.match(localRunner, /job\?\.status === "completed" && job\.id/);
  assert.doesNotMatch(
    overview + localRunner,
    /execution_id|Results imported|results\.json file/,
  );
});

test("loads the local runner only for native local manifests", () => {
  assert.match(
    loader,
    /page === 'overview' && window\.HARNESS_EXECUTIONS\?\.mode === 'local'[\s\S]*loadScript\(page, 'ansi-log\.js'\)[\s\S]*loadScript\(page, 'local-runner\.js'\)/s,
  );
  assert.match(overview, /if \(isLocal\) window\.HarnessLocalRunner\.initialize\(\)/);
  assert.doesNotMatch(overview, /api\/local|local-run-form|local-run-cancel/);
  assert.match(publisher, /"mode": "published"/);
});

test("keeps the completed runner log inside a padded local panel", () => {
  assert.match(
    index,
    /id="local-run-log"[\s\S]{0,120}className="local-run-log"/,
  );
  assert.match(localRunner, /HarnessAnsiLog\?\.tokenizeAnsiLog/);
  assert.match(localRunner, /document\.createTextNode/);
  assert.doesNotMatch(localRunner, /runLog\.innerHTML/);
  assert.match(styles, /\.local-runner\s*\{[^}]*padding:\s*28px 30px;[^}]*overflow:\s*hidden;/s);
  assert.match(styles, /\.local-run-log-shell\s*\{[^}]*overflow:\s*hidden;/s);
  assert.match(styles, /\.local-run-log\s*\{[^}]*max-width:\s*100%;[^}]*overflow-wrap:\s*anywhere;/s);
});

test("keeps comparison content padded with contained long values", () => {
  assert.match(index, /href="\.\/compare\.html/);
  const compare = fs.readFileSync(
    path.join(dashboardRoot, "src", "pages", "ComparePage.tsx"),
    "utf8",
  );
  assert.match(compare, /id="compare-content" className="compare-content"/);
  assert.match(styles, /\.compare-content\s*>\s*\.panel\s*\{[^}]*padding:\s*28px 30px;[^}]*overflow:\s*hidden;/s);
  assert.match(styles, /\.compare-selection-card h2\s*\{[^}]*overflow-wrap:\s*anywhere;/s);
  assert.match(styles, /\.compare-metric-card\s*\{[^}]*overflow:\s*hidden;/s);
});
