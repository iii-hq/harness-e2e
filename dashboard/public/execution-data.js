(function initHarnessExecutionData(root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) {
    module.exports = api;
  }
  root.HarnessExecutionData = api;
})(typeof globalThis !== "undefined" ? globalThis : this, function executionDataFactory() {
  "use strict";

  const SCENARIO_METRIC_IDS = [
    "tokens",
    "duration_seconds",
    "cost_usd",
    "function_calls",
    "function_call_errors",
    "sessions",
    "turns",
    "work_amplification",
    "effective_fan_out",
  ];

  function numberOrNull(value) {
    return typeof value === "number" && Number.isFinite(value) ? value : null;
  }

  function mean(values) {
    const available = values.filter((value) => numberOrNull(value) !== null);
    return available.length
      ? available.reduce((total, value) => total + value, 0) / available.length
      : null;
  }

  function stableJson(value) {
    if (Array.isArray(value)) {
      return `[${value.map(stableJson).join(",")}]`;
    }
    if (value && typeof value === "object") {
      return `{${Object.keys(value)
        .sort()
        .map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`)
        .join(",")}}`;
    }
    return JSON.stringify(value);
  }

  function contractFingerprint(contract) {
    const text = stableJson(contract);
    const bytes = typeof TextEncoder === "function"
      ? new TextEncoder().encode(text)
      : Uint8Array.from(
          unescape(encodeURIComponent(text)),
          (character) => character.charCodeAt(0),
        );
    let value = 2_166_136_261;
    bytes.forEach((byte) => {
      value ^= byte;
      value = Math.imul(value, 16_777_619) >>> 0;
    });
    return `fnv1a32:${value.toString(16).padStart(8, "0")}`;
  }

  function scenarioContract(scenario, scenarioId, runs) {
    const contract = {
      case_id: scenario?.case_id || null,
      execution_policy:
        scenario?.execution_policy && typeof scenario.execution_policy === "object"
          ? scenario.execution_policy
          : {},
      scenario_id: scenarioId,
      scenario_version: Number(scenario?.scenario_version) || 1,
    };
    if (scenario?.case && typeof scenario.case === "object") {
      contract.case = scenario.case;
    }
    return contract;
  }

  function normalizeScenarioMetric(item) {
    const metric = item && typeof item === "object" ? item : {};
    return {
      ...metric,
      subject_id: String(metric.subject_id || ""),
      scenario_id: String(metric.scenario_id || ""),
      scenario_version: Number(metric.scenario_version) || 1,
      contract_fingerprint: String(metric.contract_fingerprint || ""),
      run_count: Number(metric.run_count) || 0,
      averages:
        metric.averages && typeof metric.averages === "object"
          ? metric.averages
          : {},
      samples:
        metric.samples && typeof metric.samples === "object"
          ? metric.samples
          : {},
    };
  }

  function normalizeStatus(value) {
    const semanticStatuses = [
      "passed",
      "hard_gate_failed",
      "technical_failed",
      "infra_failed",
      "incomplete",
      "cancelled",
      "running",
      "cancelling",
    ];
    if (semanticStatuses.includes(value)) return value;
    return "incomplete";
  }

  function normalizeScenarioStatus(value) {
    const scenario = value && typeof value === "object" ? value : {};
    const status = String(scenario.status || "");
    if (status === "cancelled") return "cancelled";
    if (status === "running") return "running";
    if (status === "missing_report" || status === "incomplete") return "incomplete";
    if (status === "technical_failed" || Number(scenario.technical_failures || 0) > 0) {
      return "technical_failed";
    }
    if (status === "hard_gate_failed" || Number(scenario.hard_gate_failures || 0) > 0) {
      return "hard_gate_failed";
    }
    if (status === "infra_failed") return "infra_failed";
    if (scenario.passed || status === "passed" || status === "success") return "passed";
    return "infra_failed";
  }

  function normalizeExecution(entry) {
    const execution = entry && typeof entry === "object" ? entry : {};
    const subjects = Array.isArray(execution.subjects)
      ? execution.subjects.filter((subject) => subject && typeof subject === "object")
      : [];
    return {
      ...execution,
      id: String(execution.id || ""),
      label: String(execution.label || ""),
      run_id: String(execution.run_id || ""),
      attempt: Number(execution.attempt) || 1,
      status: normalizeStatus(execution.status),
      conclusion: String(execution.conclusion || ""),
      event: String(execution.event || ""),
      actor: String(execution.actor || ""),
      workflow_url: String(execution.workflow_url || ""),
      started_at: String(execution.started_at || execution.generated_at || ""),
      completed_at: String(execution.completed_at || execution.generated_at || ""),
      availability: ["full", "aggregate", "unavailable"].includes(execution.availability)
        ? execution.availability
        : subjects.length
          ? "aggregate"
          : "unavailable",
      detail_path: execution.detail_path || null,
      source: execution.source && typeof execution.source === "object"
        ? execution.source
        : {},
      release: execution.release && typeof execution.release === "object"
        ? execution.release
        : {},
      subjects,
      scenario_metrics: Array.isArray(execution.scenario_metrics)
        ? execution.scenario_metrics
            .filter((item) => item && typeof item === "object")
            .map(normalizeScenarioMetric)
        : [],
      totals: execution.totals && typeof execution.totals === "object"
        ? execution.totals
        : {},
    };
  }

  function mergeExecutionHistory(manifest) {
    const raw = manifest && typeof manifest === "object" ? manifest : {};
    const executions =
      (Array.isArray(raw.executions) ? raw.executions : [])
        .map(normalizeExecution)
        .filter((entry) => entry.id)
        .sort(
      (left, right) =>
        Date.parse(right.completed_at || right.started_at || 0) -
        Date.parse(left.completed_at || left.started_at || 0),
    );
    return {
      mode:
        raw.mode === "local"
          ? "local"
          : raw.mode === "observed"
            ? "observed"
            : "published",
      lastUpdate: raw.last_update || "",
      repoUrl: raw.repo_url || "",
      preview: Boolean(globalThis.HARNESS_BENCHMARK_PREVIEW),
      retention: raw.retention || { summaries: 100, details: 30 },
      executions,
    };
  }

  function filterExecutions(executions, filters = {}) {
    const query = String(filters.query || "").trim().toLowerCase();
    return (executions || []).filter((execution) => {
      if (filters.status && filters.status !== "all" && execution.status !== filters.status) {
        return false;
      }
      if (filters.event && filters.event !== "all" && execution.event !== filters.event) {
        return false;
      }
      if (!query) return true;
      const haystack = [
        execution.label,
        execution.id,
        execution.run_id,
        execution.source?.sha,
        execution.source?.ref,
        execution.completed_at,
        execution.started_at,
      ]
        .join(" ")
        .toLowerCase();
      return haystack.includes(query);
    });
  }

  function latestHealthModel(entry) {
    const execution = normalizeExecution(entry);
    const release = execution.release || {};
    const releaseIdentity = [release.worker, release.version]
      .filter(Boolean)
      .join("@");
    const firstFailure =
      execution.first_failure && typeof execution.first_failure === "object"
        ? execution.first_failure
        : null;
    return {
      status: execution.status,
      lane: String(execution.lane || "daily"),
      identity:
        releaseIdentity ||
        String(release.tag || "") ||
        String(execution.source?.sha || "").slice(0, 12) ||
        "Unknown",
      expectedReports: numberOrNull(execution.totals?.expected_reports),
      receivedReports: numberOrNull(execution.totals?.received_reports),
      availability: execution.availability,
      firstFailure,
      workflowUrl: execution.workflow_url,
    };
  }

  function executionsWithinDays(executions, days, now = Date.now()) {
    const windowDays = Number(days);
    if (!Number.isFinite(windowDays) || windowDays <= 0) return [...(executions || [])];
    const windowEnd = Number(now);
    const windowStart = windowEnd - windowDays * 24 * 60 * 60 * 1000;
    return (executions || []).filter((execution) => {
      const timestamp = Date.parse(
        execution?.completed_at || execution?.started_at || "",
      );
      return Number.isFinite(timestamp) && timestamp >= windowStart && timestamp <= windowEnd;
    });
  }

  function matrixRows(executions) {
    const rows = new Map();
    for (const execution of executions || []) {
      for (const subject of execution.subjects || []) {
        for (const scenario of subject.scenarios || []) {
          const key = `${subject.id}::${scenario.id}`;
          if (!rows.has(key)) {
            rows.set(key, {
              key,
              subjectId: subject.id,
              subjectLabel: `${subject.provider || ""}/${subject.model || subject.id}`.replace(
                /^\//,
                "",
              ),
              scenarioId: scenario.id,
            });
          }
        }
      }
    }
    return [...rows.values()].sort(
      (left, right) =>
        left.subjectLabel.localeCompare(right.subjectLabel) ||
        left.scenarioId.localeCompare(right.scenarioId),
    );
  }

  function matrixCell(execution, row) {
    const subject = execution?.subjects?.find((item) => item.id === row.subjectId);
    const scenario = subject?.scenarios?.find((item) => item.id === row.scenarioId);
    if (!scenario) return null;
    const status = normalizeScenarioStatus(scenario);
    return { ...scenario, status };
  }

  function matrixCellLabel(cell, status) {
    if (["failed", "hard_gate_failed", "technical_failed", "infra_failed"].includes(status)) {
      return "×";
    }
    if (status === "cancelled") return "○";
    if (status === "running" || status === "cancelling") return "•";
    if (status !== "passed") return "–";

    const score = numberOrNull(cell?.median_score);
    const passRate = numberOrNull(cell?.pass_rate);
    const percentage = score ?? (passRate === null ? null : passRate * 100);
    if (percentage === null) return "—";

    const rounded = Math.round(percentage * 10) / 10;
    return `${rounded.toLocaleString("en-US", {
      maximumFractionDigits: 1,
    })}%`;
  }

  function runMetric(run, metricId) {
    const totals = run?.metrics?.totals || {};
    if (metricId === "tokens") {
      const input = numberOrNull(totals.input_tokens);
      const output = numberOrNull(totals.output_tokens);
      return input === null || output === null ? null : input + output;
    }
    if (metricId === "duration_seconds") {
      const wallTime = numberOrNull(run?.wall_time_ms);
      return wallTime === null ? null : wallTime / 1000;
    }
    if (metricId === "cost_usd") {
      return numberOrNull(run?.cost?.total_usd);
    }
    if (metricId === "work_amplification") {
      return numberOrNull(run?.efficiency?.work_amplification);
    }
    if (metricId === "effective_fan_out") {
      return numberOrNull(run?.efficiency?.effective_fan_out);
    }
    return numberOrNull(totals[metricId]);
  }

  function executionEfficiencyTotalsFromDetail(detail) {
    const tokens = [];
    const functionCalls = [];
    for (const reportEntry of detail?.reports || []) {
      if (!reportEntry?.available) continue;
      for (const scenario of reportEntry?.report?.scenarios || []) {
        for (const run of scenario?.runs || []) {
          const tokenValue = runMetric(run, "tokens");
          const callValue = runMetric(run, "function_calls");
          if (tokenValue !== null) tokens.push(tokenValue);
          if (callValue !== null) functionCalls.push(callValue);
        }
      }
    }
    return {
      total_tokens: tokens.length
        ? tokens.reduce((total, value) => total + value, 0)
        : null,
      function_calls: functionCalls.length
        ? functionCalls.reduce((total, value) => total + value, 0)
        : null,
    };
  }

  function scenarioMetricsFromDetail(detail) {
    const grouped = new Map();
    for (const reportEntry of detail?.reports || []) {
      if (!reportEntry?.available) continue;
      for (const scenario of reportEntry?.report?.scenarios || []) {
        const scenarioId = String(
          scenario?.scenario_id || reportEntry?.scenario_id || "",
        );
        if (!scenarioId) continue;
        const runs = Array.isArray(scenario?.runs)
          ? scenario.runs.filter((run) => run && typeof run === "object")
          : [];
        const subjectId = String(reportEntry?.subject_id || "");
        const key = `${subjectId}::${scenarioId}`;
        if (!grouped.has(key)) {
          grouped.set(key, { subjectId, scenarioId, scenario, runs: [] });
        }
        grouped.get(key).runs.push(...runs);
      }
    }
    return [...grouped.values()]
      .sort(
        (left, right) =>
          left.subjectId.localeCompare(right.subjectId) ||
          left.scenarioId.localeCompare(right.scenarioId),
      )
      .map(({ subjectId, scenarioId, scenario, runs }) => {
        const averages = {};
        const samples = {};
        SCENARIO_METRIC_IDS.forEach((metricId) => {
          const values = runs
            .map((run) => runMetric(run, metricId))
            .filter((value) => value !== null);
          averages[metricId] = mean(values);
          samples[metricId] = values.length;
        });
        const contract = scenarioContract(scenario, scenarioId, runs);
        return {
          subject_id: subjectId,
          scenario_id: scenarioId,
          scenario_version: contract.scenario_version,
          contract_fingerprint: contractFingerprint(contract),
          run_count: runs.length,
          averages,
          samples,
        };
      });
  }

  function findExecution(history, id) {
    return history?.executions?.find((execution) => execution.id === id) || null;
  }

  function groupRunFailures(reports) {
    const groups = [];
    (Array.isArray(reports) ? reports : []).forEach((record) => {
      (record?.report?.scenarios || []).forEach((scenario) => {
        (scenario.runs || []).forEach((run, runIndex) => {
          const items = [];
          (run.failures || []).forEach((failure) => {
            items.push({
              kind: "failure",
              phase: failure.phase || "failure",
              message: failure.message || "No failure message",
            });
          });
          (run.hard_gates || [])
            .filter((gate) => gate && gate.passed === false)
            .forEach((gate) => {
              items.push({
                kind: "gate",
                gateId: gate.id,
                message: gate.reason || "Hard gate failed",
              });
            });
          if (items.length) {
            groups.push({
              subjectId: record.subject_id,
              scenarioId: scenario.scenario_id,
              runIndex,
              items,
            });
          }
        });
      });
    });
    return groups;
  }

  return {
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
    normalizeStatus,
    scenarioMetricsFromDetail,
    scenarioContract,
  };
});
