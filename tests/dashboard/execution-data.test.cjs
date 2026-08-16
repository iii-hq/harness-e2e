const test = require("node:test");
const assert = require("node:assert/strict");
const { loadBrowserModule } = require("./load-browser-module.cjs");

const {
  contractFingerprint,
  executionEfficiencyTotalsFromDetail,
  executionsWithinDays,
  filterExecutions,
  findExecution,
  groupRunFailures,
  latestHealthModel,
  matrixCell,
  matrixCellLabel,
  matrixRows,
  mergeExecutionHistory,
  normalizeExecution,
  normalizeScenarioStatus,
  scenarioMetricsFromDetail,
  scenarioContract,
} = loadBrowserModule("dashboard/public/execution-data.js");

function execution(overrides = {}) {
  return {
    id: "123-1",
    run_id: "123",
    attempt: 1,
    status: "passed",
    conclusion: "success",
    event: "schedule",
    completed_at: "2026-07-29T06:10:00Z",
    source: { sha: "a".repeat(40), ref: "main" },
    availability: "full",
    detail_path: "runs/123-1.json",
    totals: { scenario_pass_rate: 100 },
    subjects: [
      {
        id: "glm",
        model: "glm-5.2",
        provider: "zai",
        scenarios: [
          {
            id: "direct_answer",
            passed: true,
            status: "passed",
            median_score: 92,
            pass_rate: 1,
          },
        ],
      },
    ],
    ...overrides,
  };
}

test("keeps only current execution statuses and availability", () => {
  const normalized = normalizeExecution({
    id: 99,
    status: "infra_failed",
    subjects: [],
  });

  assert.equal(normalized.id, "99");
  assert.equal(normalized.status, "infra_failed");
  assert.equal(normalized.availability, "unavailable");
  assert.equal(
    normalizeExecution({ status: "cancelling", subjects: [] }).status,
    "cancelling",
  );
});

test("rejects removed status aliases", () => {
  assert.equal(normalizeExecution({ status: "failed" }).status, "incomplete");
  assert.equal(normalizeExecution({ status: "success" }).status, "incomplete");
});

test("builds the latest health identity, completeness, and compact first failure", () => {
  const model = latestHealthModel(
    execution({
      status: "infra_failed",
      lane: "daily",
      release: { worker: "harness", version: "1.7.3" },
      totals: { expected_reports: 16, received_reports: 0 },
      first_failure: {
        kind: "job",
        job_name: "harness e2e build",
        step_name: "Validate E2E manifests and lockfiles",
        message: "provider-deepseek/Cargo.lock needs to be updated",
      },
    }),
  );

  assert.equal(model.status, "infra_failed");
  assert.equal(model.lane, "daily");
  assert.equal(model.identity, "harness@1.7.3");
  assert.equal(model.expectedReports, 16);
  assert.equal(model.receivedReports, 0);
  assert.equal(model.firstFailure.step_name, "Validate E2E manifests and lockfiles");
  assert.equal(model.workflowUrl, "");
});

test("keeps unavailable latest health counts null", () => {
  const model = latestHealthModel(execution({ totals: {} }));

  assert.equal(model.expectedReports, null);
  assert.equal(model.receivedReports, null);
});

test("merges manifest executions and finds a retained detail", () => {
  const history = mergeExecutionHistory(
    {
      mode: "local",
      last_update: "2026-07-29T06:10:00Z",
      executions: [execution()],
    },
  );

  assert.equal(history.executions.length, 1);
  assert.equal(history.mode, "local");
  assert.equal(findExecution(history, "123-1").detail_path, "runs/123-1.json");
});

test("defaults execution history to published mode", () => {
  assert.equal(
    mergeExecutionHistory({ executions: [] }).mode,
    "published",
  );
});

test("preserves observed report mode without enabling the local runner", () => {
  const history = mergeExecutionHistory(
    { mode: "observed", executions: [] },
  );
  assert.equal(history.mode, "observed");
});

test("keeps workflow attempts distinct and newest first", () => {
  const history = mergeExecutionHistory(
    {
      executions: [
        execution({ id: "123-1", attempt: 1 }),
        execution({
          id: "123-2",
          attempt: 2,
          completed_at: "2026-07-29T06:20:00Z",
        }),
      ],
    },
  );

  assert.deepEqual(
    history.executions.map((item) => item.id),
    ["123-2", "123-1"],
  );
});

test("filters by status, trigger, run id, commit, and date", () => {
  const entries = [
    execution(),
    execution({
      id: "456-1",
      run_id: "456",
      status: "failed",
      conclusion: "failure",
      event: "workflow_dispatch",
      completed_at: "2026-07-30T07:15:00Z",
      source: { sha: "b".repeat(40), ref: "main" },
    }),
  ];

  assert.deepEqual(
    filterExecutions(entries, { status: "failed" }).map((item) => item.id),
    ["456-1"],
  );
  assert.deepEqual(
    filterExecutions(entries, { event: "schedule" }).map((item) => item.id),
    ["123-1"],
  );
  assert.equal(filterExecutions(entries, { query: "456" }).length, 1);
  assert.equal(filterExecutions(entries, { query: "bbbbbbb" }).length, 1);
  assert.equal(filterExecutions(entries, { query: "2026-07-29" }).length, 1);
});

test("filters execution metrics to a rolling day window", () => {
  const now = Date.parse("2026-07-30T12:00:00Z");
  const entries = [
    execution({ id: "recent", completed_at: "2026-07-29T12:00:00Z" }),
    execution({ id: "edge", completed_at: "2026-06-30T12:00:00Z" }),
    execution({ id: "old", completed_at: "2026-06-30T11:59:59Z" }),
  ];

  assert.deepEqual(
    executionsWithinDays(entries, 30, now).map((item) => item.id),
    ["recent", "edge"],
  );
});

test("builds subject and scenario matrix rows with result cells", () => {
  const entry = execution();
  const rows = matrixRows([entry]);
  const cell = matrixCell(entry, rows[0]);

  assert.deepEqual(rows, [
    {
      key: "glm::direct_answer",
      subjectId: "glm",
      subjectLabel: "zai/glm-5.2",
      scenarioId: "direct_answer",
    },
  ]);
  assert.equal(cell.status, "passed");
  assert.equal(cell.median_score, 92);
  assert.equal(matrixCellLabel(cell, cell.status), "92%");
  assert.equal(matrixCellLabel({ passed: false }, "failed"), "×");
  assert.equal(matrixCellLabel(null, "infra_failed"), "×");
  assert.equal(matrixCellLabel(null, "running"), "•");
  assert.equal(matrixCellLabel(null, "incomplete"), "–");
  assert.equal(matrixCellLabel(null, "cancelled"), "○");
  assert.equal(
    matrixCellLabel({ passed: true, median_score: null, pass_rate: 0.875 }, "passed"),
    "87.5%",
  );
});

test("normalizes scenario outcomes with the shared blocking precedence", () => {
  assert.equal(normalizeScenarioStatus({ status: "missing_report" }), "incomplete");
  assert.equal(
    normalizeScenarioStatus({ status: "cancelled", technical_failures: 1 }),
    "cancelled",
  );
  assert.equal(
    normalizeScenarioStatus({
      passed: false,
      hard_gate_failures: 1,
      technical_failures: 1,
    }),
    "technical_failed",
  );
  assert.equal(
    normalizeScenarioStatus({ passed: false, hard_gate_failures: 1 }),
    "hard_gate_failed",
  );
  assert.equal(
    normalizeScenarioStatus({ passed: false, hard_gate_failures: 0 }),
    "infra_failed",
  );
  assert.equal(
    normalizeScenarioStatus({ passed: true, hard_gate_failures: 1 }),
    "hard_gate_failed",
  );
  assert.equal(normalizeScenarioStatus({ passed: true }), "passed");
});

test("derives per-scenario run averages from a full execution detail", () => {
  const detail = {
    reports: [
      {
        available: true,
        subject_id: "glm",
        scenario_id: "direct_answer",
        report: {
          scenarios: [
            {
              scenario_id: "direct_answer",
              scenario_version: 1,
              execution_policy: {
                max_turns: 2,
                max_total_tokens: 32768,
              },
              runs: [
                {
                  run_id: "first",
                  prompt: "answer for first",
                  wall_time_ms: 10_000,
                  cost: { total_usd: 0.2 },
                  metrics: {
                    totals: {
                      input_tokens: 100,
                      output_tokens: 20,
                      function_calls: 2,
                      function_call_errors: 0,
                      sessions: 1,
                      turns: 4,
                    },
                  },
                  efficiency: {
                    work_amplification: 1.5,
                    effective_fan_out: 1,
                  },
                },
                {
                  run_id: "second",
                  prompt: "answer for second",
                  wall_time_ms: 20_000,
                  cost: { total_usd: 0.6 },
                  metrics: {
                    totals: {
                      input_tokens: 200,
                      output_tokens: 40,
                      function_calls: 4,
                      function_call_errors: 2,
                      sessions: 3,
                      turns: 8,
                    },
                  },
                  efficiency: {
                    work_amplification: 2.5,
                    effective_fan_out: 3,
                  },
                },
              ],
            },
          ],
        },
      },
    ],
  };
  const metrics = scenarioMetricsFromDetail(detail);
  const scenario = detail.reports[0].report.scenarios[0];

  assert.deepEqual(executionEfficiencyTotalsFromDetail(detail), {
    total_tokens: 360,
    function_calls: 6,
  });

  assert.deepEqual(metrics, [
    {
      subject_id: "glm",
      scenario_id: "direct_answer",
      scenario_version: 1,
      contract_fingerprint: contractFingerprint(
        scenarioContract(scenario, "direct_answer", scenario.runs),
      ),
      run_count: 2,
      averages: {
        tokens: 180,
        duration_seconds: 15,
        cost_usd: 0.4,
        function_calls: 3,
        function_call_errors: 1,
        sessions: 2,
        turns: 6,
        work_amplification: 2,
        effective_fan_out: 2,
      },
      samples: {
        tokens: 2,
        duration_seconds: 2,
        cost_usd: 2,
        function_calls: 2,
        function_call_errors: 2,
        sessions: 2,
        turns: 2,
        work_amplification: 2,
        effective_fan_out: 2,
      },
    },
  ]);
});

test("groupRunFailures returns empty for missing or empty reports", () => {
  assert.deepEqual(groupRunFailures(undefined), []);
  assert.deepEqual(groupRunFailures([]), []);
  assert.deepEqual(
    groupRunFailures([
      {
        subject_id: "s",
        report: {
          scenarios: [
            {
              scenario_id: "clean",
              runs: [{ failures: [], hard_gates: [{ id: "g", passed: true }] }],
            },
          ],
        },
      },
    ]),
    [],
  );
});

test("groupRunFailures groups failures and failed gates per run", () => {
  const groups = groupRunFailures([
    {
      subject_id: "anthropic-sonnet",
      report: {
        scenarios: [
          {
            scenario_id: "persistent_state",
            runs: [
              {
                failures: [{ phase: "execute", message: "boom" }],
                hard_gates: [
                  { id: "no_secrets", passed: false, reason: "leaked" },
                  { id: "compiles", passed: true },
                ],
              },
              { failures: [], hard_gates: [] },
              { failures: [{ message: "" }] },
            ],
          },
        ],
      },
    },
  ]);
  assert.equal(groups.length, 2);
  assert.deepEqual(groups[0], {
    subjectId: "anthropic-sonnet",
    scenarioId: "persistent_state",
    runIndex: 0,
    items: [
      { kind: "failure", phase: "execute", message: "boom" },
      { kind: "gate", gateId: "no_secrets", message: "leaked" },
    ],
  });
  assert.equal(groups[1].runIndex, 2);
  assert.deepEqual(groups[1].items, [
    { kind: "failure", phase: "failure", message: "No failure message" },
  ]);
});
