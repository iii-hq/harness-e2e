(function loadHarnessExecutionPreview() {
  "use strict";

  const executionApi = window.HarnessExecutionData;
  const benchmark = window.HarnessBenchmarkData.normalizeBenchmarkData(
    window.BENCHMARK_DATA,
  );
  function metric(subject, category, scenarioId, metricId) {
    return subject?.metrics?.[category]?.[scenarioId]?.[metricId]?.value ?? null;
  }

  const previewExecutions = (benchmark.snapshots || []).map((snapshot) => {
    const subjects = Object.values(snapshot.subjects || {}).map((subject) => {
      const scenarioIds = new Set();
      Object.values(subject.metrics || {}).forEach((category) => {
        Object.keys(category || {}).forEach((scenarioId) => {
          if (scenarioId !== "suite") scenarioIds.add(scenarioId);
        });
      });
      const scenarios = [...scenarioIds].sort().map((scenarioId) => {
        const score = subject.metrics?.quality?.[scenarioId]?.median_score;
        const passRate = subject.metrics?.quality?.[scenarioId]?.pass_rate;
        const passed = score?.passed ?? passRate?.passed ?? false;
        return {
          id: scenarioId,
          status: passed ? "passed" : "hard_gate_failed",
          passed,
          median_score: score?.value ?? null,
          pass_rate: passRate?.value == null ? null : passRate.value / 100,
          hard_gate_failures:
            metric(subject, "reliability", scenarioId, "hard_gate_failures") ?? 0,
          technical_failures:
            metric(subject, "reliability", scenarioId, "technical_failures") ?? 0,
          retries: metric(subject, "reliability", scenarioId, "retry_attempts") ?? 0,
          total_cost_usd:
            metric(subject, "efficiency", scenarioId, "total_cost_usd"),
          wall_time_seconds:
            metric(subject, "efficiency", scenarioId, "wall_time_seconds"),
        };
      });
      return {
        id: subject.id,
        model: subject.model,
        provider: subject.provider,
        judge: subject.judge || {},
        engine_revision: subject.engineRevision || "",
        passed: Boolean(subject.passed),
        expected_reports: scenarios.length,
        received_reports: scenarios.length,
        scenario_pass_rate:
          (metric(subject, "quality", "suite", "scenario_pass_rate") ?? 0) / 100,
        report_coverage:
          (metric(subject, "quality", "suite", "report_coverage") ?? 0) / 100,
        hard_gate_failures:
          metric(subject, "reliability", "suite", "hard_gate_failures") ?? 0,
        technical_failures:
          metric(subject, "reliability", "suite", "technical_failures") ?? 0,
        retry_attempts:
          metric(subject, "reliability", "suite", "retry_attempts") ?? 0,
        total_cost_usd: metric(subject, "efficiency", "suite", "total_cost_usd"),
        wall_time_seconds:
          metric(subject, "efficiency", "suite", "wall_time_seconds"),
        scenarios,
      };
    });
    const scenarios = subjects.flatMap((subject) => subject.scenarios);
    const passed = subjects.length > 0 && subjects.every((subject) => subject.passed);
    return {
      id: snapshot.execution?.id || snapshot.id,
      run_id: snapshot.execution?.run_id || "",
      status: passed ? "passed" : "hard_gate_failed",
      completed_at: snapshot.generatedAt || new Date(snapshot.date).toISOString(),
      started_at: snapshot.generatedAt || new Date(snapshot.date).toISOString(),
      lane: snapshot.lane,
      source: snapshot.source,
      release: snapshot.release,
      subjects,
      totals: {
        expected_reports: scenarios.length,
        received_reports: scenarios.length,
        passed_scenarios: scenarios.filter((scenario) => scenario.passed).length,
        hard_gate_failures: subjects.reduce(
          (total, subject) => total + subject.hard_gate_failures,
          0,
        ),
        technical_failures: subjects.reduce(
          (total, subject) => total + subject.technical_failures,
          0,
        ),
      },
    };
  });
  const executions = previewExecutions.map((execution, index) => {
    const runId = execution.run_id || String(30490000000 - index * 731);
    const id = execution.id || `${runId}-1`;
    return {
      ...execution,
      id,
      run_id: runId,
      attempt: 1,
      workflow_name: "Harness E2E Daily",
      workflow_url: `https://github.com/iii-hq/workers/actions/runs/${runId}`,
      event: index === 3 ? "workflow_dispatch" : "schedule",
      actor: index === 3 ? "iii-team" : "github-actions",
      conclusion:
        execution.status === "passed"
          ? "success"
          : execution.status === "cancelled"
            ? "cancelled"
            : "failure",
      availability: index < 3 ? "full" : "aggregate",
      detail_path: index < 3 ? `runs/${id}.json` : null,
      workflow_duration_seconds:
        (execution.totals?.wall_time_seconds || 0) + 104,
      execution: {
        id,
        run_id: runId,
        attempt: 1,
        event: index === 3 ? "workflow_dispatch" : "schedule",
        actor: index === 3 ? "iii-team" : "github-actions",
      },
    };
  });

  function previewRun(execution, subject, scenario, runIndex) {
    const runId = `${execution.run_id.slice(-4)}-${scenario.id}-${runIndex + 1}`;
    const sessionId = `e2e_${runId}`;
    const prompt =
      `Run the ${scenario.id} E2E scenario against ${subject.provider}/${subject.model}. ` +
      "Use the registered tools, preserve the requested invariants, and report the evidence.";
    const recoveredError = scenario.id === "reactive_automation" && runIndex === 1;
    const status = scenario.passed
      ? "passed"
      : scenario.hard_gate_failures
        ? "hard_gate_failed"
        : "infrastructure_error";
    return {
      run_id: runId,
      session_id: sessionId,
      prompt,
      wall_time_ms: Math.round(
        ((scenario.wall_time_seconds || 30) * 1000) /
          Math.max(1, execution.requested_runs || 3),
      ),
      score: scenario.median_score,
      status,
      hard_gates: [
        {
          id: "required_state_present",
          passed: !scenario.hard_gate_failures,
          reason: scenario.hard_gate_failures
            ? "The expected state record was not found."
            : "All expected state records were present.",
        },
      ],
      criteria: [
        {
          id: "task_completion",
          possible: 60,
          awarded: Math.min(60, Math.round((scenario.median_score || 0) * 0.6)),
          reason: "Awarded from the observed task result and persisted evidence.",
        },
        {
          id: "tool_discipline",
          possible: 40,
          awarded: Math.min(40, Math.round((scenario.median_score || 0) * 0.4)),
          reason: "Awarded for bounded tool use and a factual final response.",
        },
      ],
      failures: scenario.passed
        ? []
        : [{
            phase: scenario.hard_gate_failures ? "evaluate" : "collect",
            message: scenario.hard_gate_failures
              ? "A required hard gate did not pass."
              : "The required run pass rate was not met.",
          }],
      transcript: {
        messages: [
          {
            entry_id: `${runId}-user`,
            message: {
              role: "user",
              timestamp: "2026-07-30T12:00:00Z",
              content: [{ type: "text", text: prompt }],
            },
          },
          {
            entry_id: `${runId}-assistant-1`,
            message: {
              role: "assistant",
              model: subject.model,
              provider: subject.provider,
              timestamp: "2026-07-30T12:00:01Z",
              content: [
                {
                  type: "text",
                  text: "I will inspect the available functions, execute the scenario, and verify the resulting state.",
                },
                {
                  type: "function_call",
                  id: `${runId}-call-state-read`,
                  function_id: "state::get",
                  arguments: { key: `benchmark:${scenario.id}:result` },
                },
              ],
            },
          },
          {
            entry_id: `${runId}-result-1`,
            message: {
              role: "function_result",
              function_call_id: `${runId}-call-state-read`,
              function_id: "state::get",
              is_error: recoveredError,
              timestamp: "2026-07-30T12:00:02Z",
              details: recoveredError
                ? { error: "State was not available on the first read." }
                : { value: { status: "verified", scenario: scenario.id } },
              content: [
                {
                  type: "text",
                  text: recoveredError
                    ? "State was not available on the first read."
                    : `{"value":{"status":"verified","scenario":"${scenario.id}"}}`,
                },
              ],
            },
          },
          {
            entry_id: `${runId}-assistant-2`,
            message: {
              role: "assistant",
              model: subject.model,
              provider: subject.provider,
              timestamp: "2026-07-30T12:00:03Z",
              content: [
                {
                  type: "text",
                  text: recoveredError
                    ? "The initial read failed, but the final state and required effects were independently verified."
                    : "The final state and all required effects were verified successfully.",
                },
              ],
            },
          },
        ],
      },
      metrics: {
        totals: {
          sessions: 1,
          turns: 3,
          function_calls: 1,
          function_call_errors: Number(recoveredError),
          input_tokens: 1200,
          output_tokens: 180,
        },
        by_session: [
          {
            session_id: sessionId,
            depth: 0,
            turns: 3,
            function_calls: 1,
            function_call_errors: Number(recoveredError),
            input_tokens: 1200,
            output_tokens: 180,
            cost_usd:
              (scenario.total_cost_usd || 0) /
              Math.max(1, execution.requested_runs || 3),
          },
        ],
        traces: {
          trace_count: 1,
          span_count: 8,
          error_span_count: Number(recoveredError),
          duration_ms: Math.round((scenario.wall_time_seconds || 30) * 1000),
        },
      },
      judge_usage: { input_tokens: 900, output_tokens: 90, cost_usd: 0.01 },
      cost: {
        subject_usd:
          ((scenario.total_cost_usd || 0) * 0.8) /
          Math.max(1, execution.requested_runs || 3),
        judge_usd:
          ((scenario.total_cost_usd || 0) * 0.2) /
          Math.max(1, execution.requested_runs || 3),
        total_usd:
          (scenario.total_cost_usd || 0) /
          Math.max(1, execution.requested_runs || 3),
      },
      retry_attempts: [],
      judge_attempts: scenario.passed ? 1 : 2,
    };
  }

  function previewDetail(execution) {
    const reports = [];
    (execution.subjects || []).forEach((subject) => {
      (subject.scenarios || []).forEach((scenario) => {
        const runs = Array.from(
          { length: execution.requested_runs || 3 },
          (_, index) => previewRun(execution, subject, scenario, index),
        );
        reports.push({
          subject_id: subject.id,
          scenario_id: scenario.id,
          available: true,
          report: {
            subject: { model: subject.model, provider: subject.provider },
            judge: { model: subject.judge?.model, provider: subject.judge?.provider },
            engine_revision: subject.engine_revision,
            passed: scenario.passed,
            scenarios: [
              {
                scenario_id: scenario.id,
                scenario_version: scenario.scenario_version || 1,
                execution_policy: { max_turns: 16, max_total_tokens: 250000 },
                aggregate: {
                  runs: runs.length,
                  scored_runs: runs.filter((run) => run.score !== null).length,
                  passed_runs: runs.filter((run) => run.status === "passed").length,
                  required_passes: 2,
                  pass_rate: scenario.pass_rate,
                  median_score: scenario.median_score,
                  hard_gate_failures: scenario.hard_gate_failures,
                  technical_failures: scenario.technical_failures,
                  cost: { total_usd: scenario.total_cost_usd },
                },
                passed: scenario.passed,
                runs,
              },
            ],
          },
        });
      });
    });
    return {
      execution: execution.execution,
      generated_at: execution.completed_at,
      lane: execution.lane,
      source: execution.source,
      workflow_url: execution.workflow_url,
      release: execution.release,
      requested_runs: execution.requested_runs,
      subjects: execution.subjects,
      reports,
    };
  }

  const details = Object.fromEntries(
    executions
      .filter((execution) => execution.availability === "full")
      .map((execution) => {
        const detail = previewDetail(execution);
        execution.scenario_metrics = executionApi.scenarioMetricsFromDetail(detail);
        return [execution.id, detail];
      }),
  );

  window.HARNESS_EXECUTIONS = {
    last_update: executions[0]?.completed_at,
    repo_url: "https://github.com/iii-hq/workers",
    retention: { summaries: 100, details: 30 },
    executions,
  };
  window.HARNESS_EXECUTION_DETAILS = details;
})();
