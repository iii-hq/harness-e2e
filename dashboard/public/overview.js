(function renderHarnessExecutionOverview() {
  "use strict";

  const benchmarkApi = window.HarnessBenchmarkData;
  const executionApi = window.HarnessExecutionData;
  const benchmarkData = benchmarkApi.normalizeBenchmarkData(window.BENCHMARK_DATA);
  const history = executionApi.mergeExecutionHistory(
    window.HARNESS_EXECUTIONS,
    benchmarkData,
  );
  const isLocal = history.mode === "local";
  const isObserved = history.mode === "observed";
  const state = {
    matrixCount: 14,
    page: 1,
    pageSize: 25,
    query: "",
    status: "all",
    event: "all",
    scenarioHistoryRow: null,
    scenarioHistoryMetric: "cost_usd",
    comparison: [],
    comparisonLimitReached: false,
  };
  const efficiencyTrendColors = {
    improved: "var(--success)",
    regressed: "var(--danger)",
    neutral: "var(--text-muted)",
  };
  const chartAccentColor = "var(--accent)";
  const scenarioHistoryMetricIds = [
    "cost_usd",
    "tokens",
    "duration_seconds",
    "function_calls",
    "function_call_errors",
    "work_amplification",
    "effective_fan_out",
  ];
  const scenarioMetricDefinitions = {
    tokens: {
      label: "Tokens",
      description:
        "Input plus output tokens. Cache reads remain part of input usage and are not counted twice.",
      format: (value) => compactNumber(value, value < 100 ? 1 : 0),
    },
    duration_seconds: {
      label: "Time",
      description: "Wall-clock runtime for one scenario execution.",
      format: formatDuration,
    },
    cost_usd: {
      label: "Cost",
      description: "Combined subject and judge cost for one scenario execution.",
      format: formatCurrency,
    },
    function_calls: {
      label: "Function calls",
      description: "iii function calls made during one scenario execution.",
      format: (value) => compactNumber(value, 1),
    },
    function_call_errors: {
      label: "Function errors",
      description: "iii function results marked as errors during one scenario execution.",
      format: (value) => compactNumber(value, 1),
    },
    sessions: {
      label: "Sessions",
      description: "Root and descendant sessions observed during one scenario execution.",
      format: (value) => compactNumber(value, 1),
    },
    turns: {
      label: "Turns",
      description: "Agent turns observed during one scenario execution.",
      format: (value) => compactNumber(value, 1),
    },
    work_amplification: {
      label: "Work amplification",
      description: "Observed orchestration work divided by the scenario minimum.",
      format: (value) => compactNumber(value, 2),
    },
    effective_fan_out: {
      label: "Effective fan-out",
      description: "Maximum number of child branches observed concurrently.",
      format: (value) => compactNumber(value, 1),
    },
  };

  const elements = {
    actionsLink: document.querySelector("#actions-link"),
    body: document.querySelector("#execution-body"),
    capabilityBody: document.querySelector("#capability-body"),
    capabilityPolicy: document.querySelector("#capability-policy"),
    capabilityPolicyVersion: document.querySelector("#capability-policy-version"),
    capabilityReliableReason: document.querySelector("#capability-reliable-reason"),
    capabilityReliableTier: document.querySelector("#capability-reliable-tier"),
    capabilityRevision: document.querySelector("#capability-revision"),
    capabilitySampleContext: document.querySelector("#capability-sample-context"),
    capabilitySampleSize: document.querySelector("#capability-sample-size"),
    capabilityStatisticalTier: document.querySelector("#capability-statistical-tier"),
    content: document.querySelector("#overview-content"),
    count: document.querySelector("#execution-count"),
    empty: document.querySelector("#empty-state"),
    emptyDescription: document.querySelector("#empty-description"),
    emptyTitle: document.querySelector("#empty-title"),
    efficiencyBody: document.querySelector("#efficiency-body"),
    efficiencyCost: document.querySelector("#efficiency-cost"),
    efficiencyCostDelta: document.querySelector("#efficiency-cost-delta"),
    efficiencyCostBaseline: document.querySelector("#efficiency-cost-baseline"),
    efficiencyCostSparkline: document.querySelector("#efficiency-cost-sparkline"),
    efficiencyDuration: document.querySelector("#efficiency-duration"),
    efficiencyDurationDelta: document.querySelector("#efficiency-duration-delta"),
    efficiencyDurationBaseline: document.querySelector(
      "#efficiency-duration-baseline",
    ),
    efficiencyDurationSparkline: document.querySelector(
      "#efficiency-duration-sparkline",
    ),
    efficiencyErrors: document.querySelector("#efficiency-errors"),
    efficiencyErrorsDelta: document.querySelector("#efficiency-errors-delta"),
    efficiencyErrorsBaseline: document.querySelector(
      "#efficiency-errors-baseline",
    ),
    efficiencyErrorsSparkline: document.querySelector(
      "#efficiency-errors-sparkline",
    ),
    efficiencyGuardrail: document.querySelector("#efficiency-guardrail"),
    efficiencyRunLabel: document.querySelector("#efficiency-run-label"),
    efficiencyTokens: document.querySelector("#efficiency-tokens"),
    efficiencyTokensDelta: document.querySelector("#efficiency-tokens-delta"),
    efficiencyTokensBaseline: document.querySelector(
      "#efficiency-tokens-baseline",
    ),
    efficiencyTokensSparkline: document.querySelector(
      "#efficiency-tokens-sparkline",
    ),
    event: document.querySelector("#event-filter"),
    footerSummary: document.querySelector("#dashboard-footer-summary"),
    commitHeading: document.querySelector("#execution-commit-heading"),
    kpiCost: document.querySelector("#kpi-cost"),
    kpiCoverage: document.querySelector("#kpi-coverage"),
    kpiFailures: document.querySelector("#kpi-failures"),
    kpiPassRate: document.querySelector("#kpi-pass-rate"),
    kpiRuntime: document.querySelector("#kpi-runtime"),
    kpiScore: document.querySelector("#kpi-score"),
    efficiencyStatus: document.querySelector("#efficiency-status"),
    efficiencyStatusCaption: document.querySelector("#efficiency-status-caption"),
    lastUpdate: document.querySelector("#last-update"),
    latestAvailability: document.querySelector("#latest-health-availability"),
    latestCompleted: document.querySelector("#latest-health-completed"),
    latestDetailLink: document.querySelector("#latest-detail-link"),
    latestFirstFailure: document.querySelector("#latest-first-failure"),
    latestIdentity: document.querySelector("#latest-health-identity"),
    latestLane: document.querySelector("#latest-health-lane"),
    latestStatus: document.querySelector("#latest-health-status"),
    latestSummary: document.querySelector("#latest-health-summary"),
    latestTitle: document.querySelector("#latest-health-title"),
    latestWorkflowLink: document.querySelector("#latest-workflow-link"),
    localRunnerClose: document.querySelector("#close-local-runner"),
    localRunnerDialog: document.querySelector("#local-runner-dialog"),
    localRunnerOpen: document.querySelector("#open-local-runner"),
    localRunner: document.querySelector("#local-runner"),
    matrix: document.querySelector("#health-matrix"),
    next: document.querySelector("#next-page"),
    pageLabel: document.querySelector("#page-label"),
    preview: document.querySelector("#preview-badge"),
    previous: document.querySelector("#previous-page"),
    search: document.querySelector("#execution-search"),
    scenarioHistoryBody: document.querySelector("#scenario-history-body"),
    scenarioHistoryChart: document.querySelector("#scenario-history-chart"),
    scenarioHistoryClose: document.querySelector("#scenario-history-close"),
    scenarioHistoryContext: document.querySelector("#scenario-history-context"),
    scenarioHistoryDescription: document.querySelector(
      "#scenario-history-description",
    ),
    scenarioHistoryDialog: document.querySelector("#scenario-history-dialog"),
    scenarioHistoryTitle: document.querySelector("#scenario-history-title"),
    syncLabel: document.querySelector("#sync-label"),
    status: document.querySelector("#status-filter"),
    comparisonBar: document.querySelector("#comparison-bar"),
    comparisonCount: document.querySelector("#comparison-count"),
    comparisonLink: document.querySelector("#comparison-link"),
    overviewComparisonLeft: document.querySelector("#overview-comparison-left"),
    overviewComparisonMetrics: document.querySelector("#overview-comparison-metrics"),
    overviewComparisonOpen: document.querySelector("#overview-comparison-open"),
    overviewComparisonRight: document.querySelector("#overview-comparison-right"),
    overviewComparisonSummary: document.querySelector("#overview-comparison-summary"),
    overviewComparisonSwap: document.querySelector("#overview-comparison-swap"),
    overviewComparisonVerdict: document.querySelector("#overview-comparison-verdict"),
  };

  function escapeHtml(value) {
    return String(value ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#039;");
  }

  function safeUrl(value) {
    if (!value) return "";
    try {
      const url = new URL(value, window.location.href);
      return ["http:", "https:"].includes(url.protocol) ? url.href : "";
    } catch (_error) {
      return "";
    }
  }

  function detailUrl(execution, row = null) {
    const query = `./execution.html?id=${encodeURIComponent(execution.id)}`;
    return row ? `${query}#${scenarioAnchor(row.subjectId, row.scenarioId)}` : query;
  }

  function scenarioAnchor(subjectId, scenarioId) {
    return `scenario-${subjectId}-${scenarioId}`.replace(/[^a-zA-Z0-9_-]/g, "-");
  }

  function titleCase(value) {
    return String(value || "")
      .replaceAll("_", " ")
      .replaceAll("-", " ")
      .replace(/\b\w/g, (letter) => letter.toUpperCase());
  }

  function compactNumber(value, digits = 1) {
    return typeof value !== "number" || !Number.isFinite(value)
      ? "—"
      : new Intl.NumberFormat("en-US", {
          maximumFractionDigits: digits,
          minimumFractionDigits: 0,
        }).format(value);
  }

  function formatPercent(value) {
    return typeof value === "number" ? `${compactNumber(value, 1)}%` : "—";
  }

  function formatCurrency(value) {
    return typeof value !== "number"
      ? "—"
      : new Intl.NumberFormat("en-US", {
          style: "currency",
          currency: "USD",
          minimumFractionDigits: value < 1 ? 3 : 2,
          maximumFractionDigits: value < 1 ? 3 : 2,
        }).format(value);
  }

  function formatDuration(seconds) {
    if (typeof seconds !== "number") return "—";
    if (seconds < 60) return `${compactNumber(seconds, 0)}s`;
    const minutes = Math.floor(seconds / 60);
    const remainder = Math.round(seconds % 60);
    return `${minutes}m ${String(remainder).padStart(2, "0")}s`;
  }
  function formatScenarioHistoryValue(metricId, value) {
    if (metricId === "cost_usd" && typeof value === "number") {
      return new Intl.NumberFormat("en-US", {
        style: "currency",
        currency: "USD",
        minimumFractionDigits: 4,
        maximumFractionDigits: 4,
      }).format(value);
    }
    if (
      metricId === "duration_seconds" &&
      typeof value === "number" &&
      value < 60
    ) {
      return `${compactNumber(value, 1)}s`;
    }
    return scenarioMetricDefinitions[metricId].format(value);
  }

  function formatDate(timestamp, withTime = false) {
    if (!timestamp || Number.isNaN(Date.parse(timestamp))) return "Unknown date";
    return new Intl.DateTimeFormat("en-US", {
      month: "short",
      day: "numeric",
      year: "numeric",
      hour: withTime ? "2-digit" : undefined,
      minute: withTime ? "2-digit" : undefined,
    }).format(new Date(timestamp));
  }

  function statusMeta(status) {
    return {
      passed: { label: "Passed", short: "", css: "pass" },
      failed: { label: "Failed", short: "×", css: "fail" },
      hard_gate_failed: { label: "Hard gate failed", short: "×", css: "fail" },
      technical_failed: { label: "Technical failure", short: "×", css: "fail" },
      infra_failed: { label: "Infrastructure failure", short: "×", css: "fail" },
      incomplete: { label: "Incomplete", short: "–", css: "incomplete" },
      cancelled: { label: "Cancelled", short: "○", css: "cancelled" },
      running: { label: "Running", short: "•", css: "running" },
    }[status] || { label: "Unknown", short: "?", css: "incomplete" };
  }

  function failureCount(execution) {
    const totals = execution.totals || {};
    return (
      Number(totals.hard_gate_failures || 0) +
      Number(totals.technical_failures || 0) +
      Number(totals.missing_reports || 0)
    );
  }

  function renderLatestHealth() {
    const latest = history.executions[0];
    if (!latest) return;
    const health = executionApi.latestHealthModel(latest);
    const meta = statusMeta(health.status);
    const expected = health.expectedReports;
    const received = health.receivedReports;
    const hasReportDenominator = typeof expected === "number" && expected > 0;

    elements.latestStatus.className = `status-pill status-${meta.css}`;
    elements.latestStatus.textContent = meta.label;
    elements.latestTitle.textContent =
      health.status === "passed"
        ? "Latest execution passed"
        : health.availability === "unavailable"
          ? "Execution stopped before report evidence"
          : `${meta.label} needs attention`;
    elements.latestSummary.textContent = hasReportDenominator
      ? `${compactNumber(received, 0)} of ${compactNumber(expected, 0)} expected reports received. ${dataLabel(health.availability)} is retained for this execution.`
      : health.availability === "unavailable"
        ? "No scenario denominator is available, so quality and reliability metrics remain unknown instead of being shown as zero."
        : `${dataLabel(health.availability)} is retained; report completeness was not published.`;
    elements.latestIdentity.textContent = health.identity;
    elements.latestLane.textContent = titleCase(health.lane);
    elements.latestAvailability.textContent = dataLabel(health.availability);
    elements.latestCompleted.textContent = formatDate(
      latest.completed_at || latest.started_at,
      true,
    );
    elements.latestDetailLink.href = detailUrl(latest);

    const workflowUrl = safeUrl(health.workflowUrl);
    elements.latestWorkflowLink.hidden = !workflowUrl;
    if (workflowUrl) elements.latestWorkflowLink.href = workflowUrl;

    const signalIcon = document.createElement("span");
    signalIcon.className = "latest-signal-icon";
    signalIcon.setAttribute("aria-hidden", "true");
    signalIcon.textContent = health.firstFailure ? "!" : health.status === "passed" ? "✓" : "i";
    const signalCopy = document.createElement("div");
    const signalTitle = document.createElement("strong");
    const signalMessage = document.createElement("p");
    if (health.firstFailure) {
      signalTitle.textContent =
        health.firstFailure.step_name ||
        health.firstFailure.phase ||
        health.firstFailure.kind ||
        "First actionable failure";
      signalMessage.textContent =
        health.firstFailure.message || "Open the execution for diagnostic evidence.";
      elements.latestFirstFailure.classList.add("has-failure");
    } else if (health.status === "passed") {
      signalTitle.textContent = "No blocking failure in the latest execution";
      signalMessage.textContent = "Efficiency can be reviewed, subject to cohort compatibility and evidence depth.";
      elements.latestFirstFailure.classList.remove("has-failure");
    } else {
      signalTitle.textContent = "No structured first failure was retained";
      signalMessage.textContent = "Open the execution detail or workflow log for the earliest actionable signal.";
      elements.latestFirstFailure.classList.add("has-failure");
    }
    signalCopy.append(signalTitle, signalMessage);
    elements.latestFirstFailure.replaceChildren(signalIcon, signalCopy);
  }

  function renderKpis() {
    const latest = history.executions[0];
    if (!latest) return;
    elements.kpiPassRate.textContent = formatPercent(
      latest.totals?.scenario_pass_rate,
    );
    const expectedReports = latest.totals?.expected_reports;
    elements.kpiCoverage.textContent =
      typeof expectedReports === "number" && expectedReports > 0
        ? `${formatPercent(latest.totals?.report_coverage)} report coverage`
        : "No report denominator";
    elements.kpiScore.textContent = compactNumber(latest.totals?.average_score, 1);
    const reliabilityFields = [
      latest.totals?.hard_gate_failures,
      latest.totals?.technical_failures,
      latest.totals?.missing_reports,
    ];
    elements.kpiFailures.textContent = reliabilityFields.some(
      (value) => typeof value === "number" && Number.isFinite(value),
    )
      ? compactNumber(failureCount(latest), 0)
      : "—";
    const hasModelCost =
      typeof latest.totals?.total_cost_usd === "number" &&
      Number.isFinite(latest.totals.total_cost_usd);
    elements.kpiCost.textContent = hasModelCost
      ? formatCurrency(latest.totals.total_cost_usd)
      : "Not reported";
    elements.kpiCost
      .closest(".kpi-card")
      ?.classList.toggle("is-unavailable", !hasModelCost);
    elements.kpiRuntime.textContent =
      typeof latest.totals?.wall_time_seconds === "number"
        ? `${formatDuration(latest.totals.wall_time_seconds)} model runtime`
        : "Runtime not reported";
  }

  function comparisonOptionLabel(execution) {
    const label = execution.label || formatDate(execution.completed_at, true);
    const subject = execution.subjects?.[0];
    const subjectLabel = subject
      ? `${subject.provider || "unknown"}/${subject.model || "unknown"}`
      : "no subject";
    return `${label} · ${statusMeta(execution.status).label} · ${subjectLabel}`;
  }

  function appendExecutionOptions(select, selectedId) {
    select.replaceChildren(
      ...history.executions.map((execution) => {
        const option = document.createElement("option");
        option.value = execution.id;
        option.textContent = comparisonOptionLabel(execution);
        option.selected = execution.id === selectedId;
        return option;
      }),
    );
  }

  function comparisonDeltaText(values, formatter, lowerIsBetter, comparable) {
    if (!comparable) return { text: "Delta disabled", css: "unavailable" };
    if (
      typeof values.left !== "number" ||
      typeof values.right !== "number" ||
      typeof values.delta !== "number"
    ) {
      return { text: "Not reported", css: "unavailable" };
    }
    if (values.delta === 0) return { text: "No change", css: "neutral" };
    const improved = lowerIsBetter ? values.delta < 0 : values.delta > 0;
    return {
      text: `${values.delta > 0 ? "+" : ""}${formatter(values.delta)} B−A`,
      css: improved ? "improved" : "regressed",
    };
  }

  function comparisonMetricCard(definition, comparison) {
    const values = definition.values(comparison);
    const delta = comparisonDeltaText(
      values,
      definition.format,
      definition.lowerIsBetter,
      comparison.comparable,
    );
    const card = document.createElement("article");
    card.className = "overview-comparison-metric";
    const label = document.createElement("span");
    label.textContent = definition.label;
    const sides = document.createElement("div");
    sides.className = "overview-comparison-values";
    const left = document.createElement("strong");
    left.textContent = definition.format(values.left);
    const arrow = document.createElement("small");
    arrow.textContent = "→";
    const right = document.createElement("strong");
    right.textContent = definition.format(values.right);
    sides.append(left, arrow, right);
    const deltaNode = document.createElement("small");
    deltaNode.className = `comparison-metric-delta delta-${delta.css}`;
    deltaNode.textContent = delta.text;
    card.append(label, sides, deltaNode);
    return card;
  }

  function renderOverviewComparison() {
    const left = executionApi.findExecution(
      history,
      elements.overviewComparisonLeft.value,
    );
    const right = executionApi.findExecution(
      history,
      elements.overviewComparisonRight.value,
    );
    const ready = left && right && left.id !== right.id;
    elements.overviewComparisonOpen.setAttribute("aria-disabled", String(!ready));
    elements.overviewComparisonSwap.disabled = !ready;
    if (!ready) {
      elements.overviewComparisonVerdict.className =
        "comparison-verdict comparison-verdict-unavailable";
      elements.overviewComparisonVerdict.textContent =
        history.executions.length < 2 ? "Need two executions" : "Choose different executions";
      elements.overviewComparisonSummary.textContent =
        "Select two retained executions to compare their outcome guardrails and efficiency.";
      elements.overviewComparisonMetrics.replaceChildren();
      elements.overviewComparisonOpen.href = "./compare.html";
      return;
    }

    const comparison = executionApi.compareExecutions(left, right);
    const eligible = comparison.compatibility === "eligible";
    elements.overviewComparisonVerdict.className =
      `comparison-verdict comparison-verdict-${eligible ? "eligible" : "exploratory"}`;
    elements.overviewComparisonVerdict.textContent = eligible
      ? "Regression eligible"
      : comparison.compatibility === "legacy_unverified"
        ? "Exploratory · identity unavailable"
        : "Exploratory only";
    const comparableScenarios = comparison.scenarios.filter(
      (scenario) => scenario.comparable,
    ).length;
    elements.overviewComparisonSummary.textContent = eligible
      ? `${comparableScenarios} scenario${comparableScenarios === 1 ? "" : "s"} share the same comparison contract. Deltas describe candidate B relative to baseline A.`
      : comparison.warnings[0] ||
        "The selected executions remain side by side, but improvement and regression labels are disabled.";
    elements.overviewComparisonOpen.href =
      `./compare.html?left=${encodeURIComponent(left.id)}&right=${encodeURIComponent(right.id)}`;

    const blockingFailures = (execution) => {
      const totals = execution.totals || {};
      const fields = [
        totals.hard_gate_failures,
        totals.technical_failures,
        totals.missing_reports,
      ];
      return fields.some((value) => typeof value === "number")
        ? fields.reduce((total, value) => total + Number(value || 0), 0)
        : null;
    };
    const metricDefinitions = [
      {
        label: "Pass rate",
        format: formatPercent,
        lowerIsBetter: false,
        values: (item) => item.totals.scenario_pass_rate,
      },
      {
        label: "Quality score",
        format: (value) => compactNumber(value, 1),
        lowerIsBetter: false,
        values: (item) => item.totals.average_score,
      },
      {
        label: "Blocking failures",
        format: (value) => compactNumber(value, 0),
        lowerIsBetter: true,
        values: (item) => {
          const leftValue = blockingFailures(item.left);
          const rightValue = blockingFailures(item.right);
          return {
            left: leftValue,
            right: rightValue,
            delta:
              item.comparable && leftValue !== null && rightValue !== null
                ? rightValue - leftValue
                : null,
          };
        },
      },
      {
        label: "Tokens",
        format: (value) => compactNumber(value, 0),
        lowerIsBetter: true,
        values: (item) => item.totals.total_tokens,
      },
      {
        label: "Cost",
        format: formatCurrency,
        lowerIsBetter: true,
        values: (item) => item.totals.total_cost_usd,
      },
      {
        label: "Runtime",
        format: formatDuration,
        lowerIsBetter: true,
        values: (item) => item.totals.wall_time_seconds,
      },
    ];
    elements.overviewComparisonMetrics.replaceChildren(
      ...metricDefinitions.map((definition) =>
        comparisonMetricCard(definition, comparison),
      ),
    );
  }

  function initializeOverviewComparison() {
    const candidate = history.executions[0] || null;
    const baseline = candidate
      ? history.executions
          .slice(1)
          .find((execution) =>
            executionApi.compareExecutions(execution, candidate).comparable,
          ) || history.executions[1] || null
      : null;
    appendExecutionOptions(elements.overviewComparisonLeft, baseline?.id || "");
    appendExecutionOptions(elements.overviewComparisonRight, candidate?.id || "");
    elements.overviewComparisonLeft.disabled = history.executions.length < 2;
    elements.overviewComparisonRight.disabled = history.executions.length < 2;
    renderOverviewComparison();
  }

  function tierLabel(value) {
    const labels = {
      l0_atomic: "L0 Atomic",
      l1_sequential: "L1 Sequential",
      l2_stateful: "L2 Stateful",
      l3_concurrent: "L3 Concurrent",
      l4_coordinated: "L4 Coordinated",
      l5_adaptive: "L5 Adaptive",
    };
    return labels[value] || titleCase(value) || "—";
  }

  function rateWithInterval(estimate) {
    if (typeof estimate?.rate !== "number") return "—";
    const rate = formatPercent(estimate.rate * 100);
    if (
      typeof estimate.ci95_lower !== "number" ||
      typeof estimate.ci95_upper !== "number"
    ) {
      return rate;
    }
    return `${rate} · CI ${formatPercent(estimate.ci95_lower * 100)}–${formatPercent(
      estimate.ci95_upper * 100,
    )}`;
  }

  function renderCapability() {
    const latest = history.executions[0];
    const capability = latest?.capability;
    const tiers = Array.isArray(capability?.tiers) ? capability.tiers : [];
    const revision = latest?.source?.sha || latest?.stack?.lock_digest || "Unknown";
    elements.capabilityRevision.textContent = String(revision).slice(0, 16);
    elements.capabilityRevision.title = String(revision);
    elements.capabilityPolicyVersion.textContent = capability?.policy?.policy_version
      ? `v${capability.policy.policy_version}`
      : "—";
    elements.capabilityReliableTier.textContent = capability?.highest_reliable_tier
      ? tierLabel(capability.highest_reliable_tier)
      : "Not established";
    elements.capabilityStatisticalTier.textContent =
      capability?.highest_statistically_eligible_tier
        ? tierLabel(capability.highest_statistically_eligible_tier)
        : "Not established";
    const observedRuns = tiers.reduce(
      (total, tier) => total + (Number(tier.sample_size) || 0),
      0,
    );
    elements.capabilitySampleSize.textContent = compactNumber(observedRuns, 0);
    elements.capabilitySampleContext.textContent = `${tiers.length} materialized tier${
      tiers.length === 1 ? "" : "s"
    }`;
    const policy = capability?.policy || {};
    const costBudget = typeof policy.maximum_p95_cost_usd === "number";
    const timeBudget = typeof policy.maximum_p95_wall_time_ms === "number";
    elements.capabilityReliableReason.textContent = capability?.highest_reliable_tier
      ? "All reliability and p95 budget thresholds passed"
      : !costBudget || !timeBudget
        ? "p95 cost and wall-time budgets are not fully configured"
        : "No observed tier meets every configured threshold";
    elements.capabilityPolicy.textContent = [
      `n≥${policy.minimum_sample_size ?? "—"}`,
      `deliverable≥${formatPercent((policy.minimum_deliverable_success_rate ?? 0) * 100)}`,
      `structure≥${formatPercent((policy.minimum_structural_integrity_rate ?? 0) * 100)}`,
      `technical≤${formatPercent((policy.maximum_technical_failure_rate ?? 0) * 100)}`,
    ].join(" · ");

    elements.capabilityBody.replaceChildren();
    if (!tiers.length) {
      const row = document.createElement("tr");
      row.innerHTML =
        '<td class="table-empty" colspan="11">No materialized complexity evidence in this execution.</td>';
      elements.capabilityBody.append(row);
      return;
    }
    tiers.forEach((tier) => {
      const row = document.createElement("tr");
      const status = tier.reliable
        ? { label: "Reliable", css: "pass" }
        : tier.statistically_eligible
          ? { label: "Budget pending", css: "incomplete" }
          : { label: "Insufficient", css: "fail" };
      const amplification =
        typeof tier.p50_work_amplification === "number"
          ? `${compactNumber(tier.p50_work_amplification, 2)} p50${
              typeof tier.p95_work_amplification === "number"
                ? ` · ${compactNumber(tier.p95_work_amplification, 2)} p95`
                : ""
            }`
          : "—";
      row.innerHTML = `
        <td data-label="Tier"><strong>${escapeHtml(tierLabel(tier.tier))}</strong></td>
        <td data-label="Runs">${escapeHtml(compactNumber(tier.sample_size, 0))}</td>
        <td data-label="Deliverable">${escapeHtml(rateWithInterval(tier.deliverable_success))}</td>
        <td data-label="Structure">${escapeHtml(rateWithInterval(tier.structural_integrity))}</td>
        <td data-label="Technical failures">${escapeHtml(rateWithInterval(tier.technical_failure))}</td>
        <td data-label="Flaky rate">${escapeHtml(
          typeof tier.flaky_rate === "number"
            ? formatPercent(tier.flaky_rate * 100)
            : "—",
        )}</td>
        <td data-label="p95 latency">${escapeHtml(
          typeof tier.p95_wall_time_ms === "number"
            ? formatDuration(tier.p95_wall_time_ms / 1000)
            : "—",
        )}</td>
        <td data-label="p95 cost">${escapeHtml(formatCurrency(tier.p95_cost_usd))}</td>
        <td data-label="Cost / success">${escapeHtml(formatCurrency(tier.cost_per_successful_deliverable))}</td>
        <td data-label="Work amplification">${escapeHtml(amplification)}</td>
        <td data-label="Status"><span class="table-status status-${status.css}" title="${escapeHtml(
          (tier.reasons || []).join("; "),
        )}">${escapeHtml(status.label)}</span></td>
      `;
      elements.capabilityBody.append(row);
    });
  }

  function deltaMeta(value) {
    if (typeof value !== "number" || !Number.isFinite(value)) {
      return { label: "Collecting comparable baseline", css: "neutral" };
    }
    const absolute = Math.abs(value);
    if (absolute < 0.05) {
      return { label: "No change vs baseline median", css: "neutral" };
    }
    return {
      label: `${value < 0 ? "↓" : "↑"} ${compactNumber(absolute, 1)}% vs baseline median`,
      css: value < 0 ? "improved" : "regressed",
    };
  }

  function renderEfficiencySparkline(element, metricId, color, cohortRows, baseline) {
    const points = executionApi.cohortMetricSparkline(
      history.executions,
      cohortRows,
      metricId,
      14,
    );
    if (!points.length) {
      element.replaceChildren();
      return;
    }
    const definition = scenarioMetricDefinitions[metricId];
    const values = points.map((point) => point.value);
    const baselineValue = typeof baseline === "number" ? baseline : null;
    const scaleValues =
      baselineValue === null ? values : [...values, baselineValue];
    const width = 180;
    const height = 42;
    const minimum = Math.min(...scaleValues);
    const maximum = Math.max(...scaleValues);
    const range = maximum - minimum || 1;
    const x = (index) =>
      values.length === 1 ? width / 2 : (index / (values.length - 1)) * width;
    const y = (value) => 5 + (1 - (value - minimum) / range) * (height - 10);
    const svg = svgElement("svg", {
      viewBox: `0 0 ${width} ${height}`,
      role: "img",
      "aria-label": `${definition.label} comparable cohort trend`,
    });
    const areaPoints = [
      `0,${height}`,
      ...values.map((value, index) => `${x(index)},${y(value)}`),
      `${width},${height}`,
    ].join(" ");
    svg.append(
      svgElement("polygon", {
        points: areaPoints,
        fill: color,
        class: "efficiency-sparkline-area",
      }),
    );
    if (baselineValue !== null) {
      const baselineLine = svgElement("line", {
        x1: 0,
        x2: width,
        y1: y(baselineValue),
        y2: y(baselineValue),
        class: "efficiency-sparkline-baseline",
      });
      const baselineTitle = svgElement("title", {});
      baselineTitle.textContent =
        `Baseline ${definition.format(baselineValue)} — median of up to 7 prior comparable runs`;
      baselineLine.append(baselineTitle);
      svg.append(baselineLine);
    }
    svg.append(
      svgElement("polyline", {
        points: values
          .map((value, index) => `${x(index)},${y(value)}`)
          .join(" "),
        stroke: color,
        class: "efficiency-sparkline-line",
      }),
      svgElement("circle", {
        cx: x(values.length - 1),
        cy: y(values.at(-1)),
        r: 3,
        fill: color,
      }),
    );
    const slot = width / points.length;
    points.forEach((point, index) => {
      const hit = svgElement("rect", {
        x: index * slot,
        y: 0,
        width: slot,
        height,
        class: "efficiency-sparkline-hit",
      });
      const parts = [
        `Run ${point.executionId}`,
        formatDate(point.timestamp),
        definition.format(point.value),
      ];
      if (baselineValue !== null && baselineValue !== 0) {
        const deltaPct = ((point.value - baselineValue) / Math.abs(baselineValue)) * 100;
        parts.push(
          `${deltaPct < 0 ? "↓" : "↑"} ${compactNumber(Math.abs(deltaPct), 1)}% vs baseline`,
        );
      }
      const title = svgElement("title", {});
      title.textContent = parts.join(" · ");
      hit.append(title);
      svg.append(hit);
    });
    element.replaceChildren(svg);
  }

  function efficiencyCell(row, metricId, formatter) {
    const current = row.current?.averages?.[metricId];
    if (typeof current !== "number") {
      const previous = row.baseline?.[metricId];
      return typeof previous === "number"
        ? `<span class="efficiency-cell-muted">${escapeHtml(
            formatter(previous),
          )}<small>last observed</small></span>`
        : "—";
    }
    const delta =
      row.lifecycle === "comparable" && row.outcome.passed
        ? row.deltas?.[metricId]
        : null;
    const meta = deltaMeta(delta);
    const deltaLabel =
      typeof delta === "number"
        ? `<small class="efficiency-cell-delta delta-${meta.css}">${escapeHtml(
            `${delta < 0 ? "↓" : delta > 0 ? "↑" : ""}${compactNumber(
              Math.abs(delta),
              1,
            )}%`,
          )}</small>`
        : "";
    return `<span>${escapeHtml(formatter(current))}${deltaLabel}</span>`;
  }

  function efficiencyTrendMeta(row) {
    const values = {
      improving: ["Improving", "improved"],
      stable: ["Stable", "stable"],
      regressed: ["Regressed", "regressed"],
      mixed: ["Mixed", "mixed"],
      collecting: [
        `Baseline ${row.historyCount}/${row.established ? row.historyCount : 5}`,
        "collecting",
      ],
      new: ["New", "new"],
      changed: [`Changed · v${row.scenarioVersion}`, "changed"],
      retired: ["Removed", "retired"],
      non_comparable: ["Non-comparable", "non-comparable"],
    };
    const [label, css] = values[row.trend] || ["Unknown", "collecting"];
    return { label, css };
  }

  function renderEfficiency() {
    const latest = history.executions[0];
    if (latest) {
      const status = statusMeta(latest.status);
      const latestReference = String(latest.run_id || latest.id || "unknown");
      elements.efficiencyStatus.textContent = status.label;
      elements.efficiencyStatus.className =
        `efficiency-result-value text-${status.css}`;
      elements.efficiencyStatusCaption.textContent =
        `${formatDate(latest.completed_at, true)} · run ${latestReference.slice(0, 8)}` +
        (latest.attempt > 1 ? ` · attempt ${latest.attempt}` : "");
      elements.efficiencyStatusCaption.title = latestReference;
    }
    const overview = executionApi.buildEfficiencyOverview(history.executions);
    if (!overview.latest) {
      elements.efficiencyBody.innerHTML =
        '<tr><td colspan="7" class="table-empty">Waiting for complete efficiency reports.</td></tr>';
      return;
    }
    const cards = [
      {
        metricId: "cost_usd",
        value: elements.efficiencyCost,
        delta: elements.efficiencyCostDelta,
        baseline: elements.efficiencyCostBaseline,
        sparkline: elements.efficiencyCostSparkline,
        format: formatCurrency,
      },
      {
        metricId: "tokens",
        value: elements.efficiencyTokens,
        delta: elements.efficiencyTokensDelta,
        baseline: elements.efficiencyTokensBaseline,
        sparkline: elements.efficiencyTokensSparkline,
        format: (value) => compactNumber(value, 0),
      },
      {
        metricId: "duration_seconds",
        value: elements.efficiencyDuration,
        delta: elements.efficiencyDurationDelta,
        baseline: elements.efficiencyDurationBaseline,
        sparkline: elements.efficiencyDurationSparkline,
        format: formatDuration,
      },
      {
        metricId: "function_call_errors",
        value: elements.efficiencyErrors,
        delta: elements.efficiencyErrorsDelta,
        baseline: elements.efficiencyErrorsBaseline,
        sparkline: elements.efficiencyErrorsSparkline,
        format: (value) => compactNumber(value, 1),
      },
    ];
    const cohortRows = overview.rows.filter(
      (row) => row.lifecycle === "comparable" && row.outcome.passed,
    );
    const maximumHistory = Math.max(
      0,
      ...cohortRows.map((row) => Number(row.historyCount) || 0),
    );
    elements.efficiencyRunLabel.textContent = cohortRows.length
      ? `${cohortRows.length} comparable scenario${cohortRows.length === 1 ? "" : "s"} · up to ${maximumHistory} prior execution${maximumHistory === 1 ? "" : "s"}`
      : "No compatible baseline cohort";
    elements.efficiencyRunLabel.removeAttribute("title");
    cards.forEach((card) => {
      const metric = overview.metrics[card.metricId];
      // Value, delta, and sparkline must all read the same population; fall
      // back to the full-suite total only while no cohort exists yet.
      const currentValue = cohortRows.length
        ? metric?.comparableCurrent
        : metric?.operational;
      card.value.textContent = card.format(currentValue);
      const baselineValue =
        cohortRows.length && typeof metric?.comparableBaseline === "number"
          ? metric.comparableBaseline
          : null;
      const meta =
        typeof currentValue !== "number"
          ? {
              label:
                card.metricId === "cost_usd"
                  ? "Not reported by provider"
                  : "Not reported",
              css: "neutral",
            }
          : baselineValue === null
            ? { label: "Collecting comparable baseline", css: "neutral" }
            : currentValue === baselineValue
              ? { label: "No change vs baseline median", css: "neutral" }
              : card.metricId === "function_call_errors" && baselineValue === 0
                ? {
                    label: `↑ ${compactNumber(currentValue, 1)} vs zero baseline`,
                    css: "regressed",
                  }
                : deltaMeta(metric?.delta);
      card.delta.textContent = meta.label;
      card.delta.className = `efficiency-delta delta-${meta.css}`;
      card.value
        .closest(".efficiency-card")
        ?.classList.toggle("is-unavailable", typeof currentValue !== "number");
      card.baseline.textContent =
        baselineValue === null ? "" : `baseline ${card.format(baselineValue)}`;
      renderEfficiencySparkline(
        card.sparkline,
        card.metricId,
        efficiencyTrendColors[meta.css] || efficiencyTrendColors.neutral,
        cohortRows,
        cohortRows.length ? metric?.comparableBaseline : null,
      );
    });

    const passed = Number(overview.latest.totals?.passed_scenarios) || 0;
    const expected = Number(overview.latest.totals?.expected_reports) || 0;
    const countParts = [
      `${passed}/${expected} scenarios passed`,
      `${overview.counts.comparable} comparable`,
    ];
    if (overview.counts.new) countParts.push(`${overview.counts.new} new`);
    if (overview.counts.changed) countParts.push(`${overview.counts.changed} changed`);
    if (overview.counts.retired) countParts.push(`${overview.counts.retired} removed`);
    if (overview.counts.nonComparable) {
      countParts.push(`${overview.counts.nonComparable} non-comparable`);
    }
    const guardrailAlert =
      overview.counts.nonComparable ||
      overview.counts.changed ||
      passed < expected;
    elements.efficiencyGuardrail.className =
      `efficiency-guardrail${guardrailAlert ? " efficiency-guardrail-alert" : ""}`;
    elements.efficiencyGuardrail.innerHTML = `
      <span class="guardrail-status">${
        guardrailAlert ? "Outcome attention" : "Outcome guardrail passed"
      }</span>
      <span>${escapeHtml(countParts.join(" · "))}</span>
      <small>Lower efficiency totals are positive only for the comparable cohort.</small>
    `;

    elements.efficiencyBody.replaceChildren();
    overview.rows.forEach((row) => {
      const trend = efficiencyTrendMeta(row);
      const tableRow = document.createElement("tr");
      tableRow.className = `efficiency-row efficiency-row-${trend.css}`;
      tableRow.innerHTML = `
        <th scope="row">
          <button class="scenario-history-button" type="button">
            <span>${escapeHtml(titleCase(row.scenarioId))}</span>
            <small>${escapeHtml(row.subjectId || "default subject")} · v${escapeHtml(
            row.scenarioVersion,
          )}</small></button>
        </th>
        <td data-label="Cost">${efficiencyCell(row, "cost_usd", formatCurrency)}</td>
        <td data-label="Tokens">${efficiencyCell(row, "tokens", (value) =>
          compactNumber(value, 0),
        )}</td>
        <td data-label="Duration">${efficiencyCell(row, "duration_seconds", formatDuration)}</td>
        <td data-label="Calls">${efficiencyCell(row, "function_calls", (value) =>
          compactNumber(value, 1),
        )}</td>
        <td data-label="Errors">${efficiencyCell(row, "function_call_errors", (value) =>
          compactNumber(value, 1),
        )}</td>
        <td data-label="Trend"><span class="efficiency-trend trend-${trend.css}">${escapeHtml(
          trend.label,
        )}</span></td>
      `;
      tableRow
        .querySelector(".scenario-history-button")
        .addEventListener("click", () => openScenarioHistory(row));
      elements.efficiencyBody.append(tableRow);
    });
  }

  async function hydrateExecutionMetrics() {
    await Promise.all(
      history.executions.map(async (execution) => {
        const hasScenarioMetrics = (execution.scenario_metrics || []).length > 0;
        const hasEfficiencyTotals =
          typeof execution.totals?.total_tokens === "number" &&
          typeof execution.totals?.function_calls === "number";
        if (
          (hasScenarioMetrics && hasEfficiencyTotals) ||
          execution.availability !== "full" ||
          typeof execution.detail_path !== "string" ||
          execution.detail_path.includes("..") ||
          !execution.detail_path.startsWith("runs/")
        ) {
          return;
        }
        try {
          const preview = window.HARNESS_EXECUTION_DETAILS?.[execution.id];
          let detail = preview;
          if (!detail) {
            const url = new URL(execution.detail_path, window.location.href);
            const runsRoot = new URL("./runs/", window.location.href);
            if (
              url.origin !== runsRoot.origin ||
              !url.pathname.startsWith(runsRoot.pathname)
            ) {
              return;
            }
            const response = await fetch(url, { cache: "no-store" });
            if (!response.ok) return;
            detail = await response.json();
          }
          if (!hasScenarioMetrics) {
            execution.scenario_metrics =
              executionApi.scenarioMetricsFromDetail(detail);
          }
          execution.totals = {
            ...execution.totals,
            ...executionApi.executionEfficiencyTotalsFromDetail(detail),
          };
        } catch (_error) {
          if (!hasScenarioMetrics) execution.scenario_metrics = [];
        }
      }),
    );
  }

  function svgElement(name, attributes = {}) {
    const element = document.createElementNS("http://www.w3.org/2000/svg", name);
    Object.entries(attributes).forEach(([key, value]) => {
      element.setAttribute(key, String(value));
    });
    return element;
  }

  function scenarioHistoryEntries(row) {
    return history.executions
      .map((execution) => {
        const metric = (execution.scenario_metrics || []).find(
          (candidate) =>
            String(candidate.subject_id || "") === row.subjectId &&
            candidate.scenario_id === row.scenarioId,
        );
        if (!metric) return null;
        const subject = row.subjectId
          ? (execution.subjects || []).find(
              (candidate) => String(candidate.id || "") === row.subjectId,
            )
          : (execution.subjects || []).find((candidate) =>
              (candidate.scenarios || []).some(
                (scenario) => scenario.id === row.scenarioId,
              ),
            );
        const scenario = (subject?.scenarios || []).find(
          (candidate) => candidate.id === row.scenarioId,
        );
        const status =
          execution.status === "cancelled"
            ? "cancelled"
            : !scenario
              ? "incomplete"
              : executionApi.normalizeScenarioStatus(scenario);
        return { execution, metric, scenario, status };
      })
      .filter(Boolean)
      .reverse();
  }

  function renderScenarioHistoryTooltip(svg, point, entry, definition, metricId, bounds) {
    svg.querySelector(".scenario-history-tooltip")?.remove();
    const width = 212;
    const height = 70;
    const x = Math.min(Math.max(point.x - width / 2, 8), bounds.width - width - 8);
    const y = point.y > 92 ? point.y - height - 15 : point.y + 15;
    const meta = statusMeta(entry.status);
    const group = svgElement("g", { class: "scenario-history-tooltip" });
    group.append(
      svgElement("rect", {
        x,
        y,
        width,
        height,
        rx: 8,
        class: "chart-tooltip-box",
      }),
    );
    const heading = svgElement("text", {
      x: x + 12,
      y: y + 18,
      class: "chart-tooltip-heading",
    });
    heading.textContent = `${formatDate(entry.execution.completed_at, true)} · run ${
      entry.execution.run_id || entry.execution.id
    }`;
    const value = svgElement("text", {
      x: x + 12,
      y: y + 40,
      class: "chart-tooltip-value",
    });
    value.textContent = formatScenarioHistoryValue(metricId, point.value);
    const outcome = svgElement("text", {
      x: x + 12,
      y: y + 58,
      class: `chart-tooltip-status chart-tooltip-status-${meta.css}`,
    });
    outcome.textContent = `${meta.label} · v${entry.metric.scenario_version || 1}`;
    group.append(heading, value, outcome);
    svg.append(group);
  }

  function renderScenarioHistoryChart(entries, metricId, row) {
    const definition = scenarioMetricDefinitions[metricId];
    const points = entries
      .map((entry) => ({
        ...entry,
        value: entry.metric?.averages?.[metricId],
      }))
      .filter(
        (entry) =>
          typeof entry.value === "number" && Number.isFinite(entry.value),
      );
    if (!points.length) {
      elements.scenarioHistoryChart.innerHTML =
        '<div class="chart-empty">No values were collected for this metric.</div>';
      return;
    }

    const width = 960;
    const height = 330;
    const margin = { top: 28, right: 24, bottom: 48, left: 74 };
    const plotWidth = width - margin.left - margin.right;
    const plotHeight = height - margin.top - margin.bottom;
    const baseline = row.baseline?.[metricId];
    const domain = points.map((point) => point.value);
    if (typeof baseline === "number") domain.push(baseline);
    let minimum = Math.min(...domain);
    let maximum = Math.max(...domain);
    const padding =
      maximum === minimum
        ? Math.max(Math.abs(maximum) * 0.15, 1)
        : (maximum - minimum) * 0.15;
    minimum = Math.max(0, minimum - padding);
    maximum += padding;
    if (maximum === minimum) maximum = minimum + 1;
    const x = (index) =>
      margin.left +
      (points.length === 1 ? plotWidth / 2 : (index / (points.length - 1)) * plotWidth);
    const y = (value) =>
      margin.top + (1 - (value - minimum) / (maximum - minimum)) * plotHeight;
    const svg = svgElement("svg", {
      viewBox: `0 0 ${width} ${height}`,
      role: "img",
      "aria-label": `${definition.label} history for ${titleCase(row.scenarioId)}`,
    });

    for (let index = 0; index <= 4; index += 1) {
      const value = minimum + ((maximum - minimum) * index) / 4;
      const pointY = y(value);
      svg.append(
        svgElement("line", {
          x1: margin.left,
          y1: pointY,
          x2: width - margin.right,
          y2: pointY,
          class: "chart-grid-line",
        }),
      );
      const label = svgElement("text", {
        x: margin.left - 12,
        y: pointY + 4,
        "text-anchor": "end",
        class: "chart-axis-label",
      });
      label.textContent = formatScenarioHistoryValue(metricId, value);
      svg.append(label);
    }

    if (typeof baseline === "number") {
      const baselineY = y(baseline);
      svg.append(
        svgElement("line", {
          x1: margin.left,
          y1: baselineY,
          x2: width - margin.right,
          y2: baselineY,
          class: "chart-target",
        }),
      );
      const baselineLabel = svgElement("text", {
        x: width - margin.right,
        y: baselineY - 7,
        "text-anchor": "end",
        class: "chart-target-label",
      });
      baselineLabel.textContent = `Comparable baseline ${formatScenarioHistoryValue(
        metricId,
        baseline,
      )}`;
      svg.append(baselineLabel);
    }

    let segment = [];
    const appendSegment = () => {
      if (segment.length > 1) {
        svg.append(
          svgElement("polyline", {
            points: segment.map((point) => `${point.x},${point.y}`).join(" "),
            stroke: chartAccentColor,
            class: "chart-path",
          }),
        );
      }
      segment = [];
    };
    points.forEach((entry, index) => {
      const point = { x: x(index), y: y(entry.value), value: entry.value };
      const previous = points[index - 1];
      if (
        previous &&
        previous.metric.contract_fingerprint !== entry.metric.contract_fingerprint
      ) {
        appendSegment();
        const boundaryX = (x(index - 1) + x(index)) / 2;
        svg.append(
          svgElement("line", {
            x1: boundaryX,
            y1: margin.top,
            x2: boundaryX,
            y2: height - margin.bottom,
            class: "chart-contract-boundary",
          }),
        );
        const label = svgElement("text", {
          x: boundaryX + 5,
          y: margin.top + 10,
          class: "chart-contract-label",
        });
        label.textContent = `v${entry.metric.scenario_version || 1}`;
        svg.append(label);
      }
      segment.push(point);
      if (index === points.length - 1) appendSegment();

      const link = svgElement("a", {
        href: detailUrl(entry.execution, row),
        "aria-label": `${definition.label} ${definition.format(entry.value)}, ${
          statusMeta(entry.status).label
        }, ${formatDate(entry.execution.completed_at, true)}`,
      });
      const circle = svgElement("circle", {
        cx: point.x,
        cy: point.y,
        r: 4.5,
        fill: chartAccentColor,
        class: `chart-point chart-point-${statusMeta(entry.status).css}`,
      });
      link.append(circle);
      const showTooltip = () =>
        renderScenarioHistoryTooltip(
          svg,
          point,
          entry,
          definition,
          metricId,
          { width, height },
        );
      link.addEventListener("mouseenter", showTooltip);
      link.addEventListener("focus", showTooltip);
      link.addEventListener("mouseleave", () =>
        svg.querySelector(".scenario-history-tooltip")?.remove(),
      );
      link.addEventListener("blur", () =>
        svg.querySelector(".scenario-history-tooltip")?.remove(),
      );
      svg.append(link);
    });

    const labelIndexes = [
      ...new Set([0, Math.floor((points.length - 1) / 2), points.length - 1]),
    ];
    labelIndexes.forEach((index) => {
      const label = svgElement("text", {
        x: x(index),
        y: height - 16,
        "text-anchor":
          index === 0
            ? "start"
            : index === points.length - 1
              ? "end"
              : "middle",
        class: "chart-x-label",
      });
      label.textContent = formatDate(points[index].execution.completed_at, true);
      svg.append(label);
    });
    elements.scenarioHistoryChart.replaceChildren(svg);
  }

  function renderScenarioHistoryTable(entries, metricId, row) {
    const definition = scenarioMetricDefinitions[metricId];
    let previousComparable = null;
    const prepared = entries.map((entry, index) => {
      const value = entry.metric?.averages?.[metricId];
      const comparable =
        entry.status === "passed" &&
        previousComparable &&
        previousComparable.metric.contract_fingerprint ===
          entry.metric.contract_fingerprint;
      const delta = comparable
        ? ((value - previousComparable.value) / Math.abs(previousComparable.value)) * 100
        : null;
      const previous = entries[index - 1];
      const contractChanged = Boolean(
        previous &&
          previous.metric.contract_fingerprint !== entry.metric.contract_fingerprint,
      );
      if (entry.status === "passed" && typeof value === "number" && value !== 0) {
        previousComparable = { metric: entry.metric, value };
      }
      return { ...entry, value, delta, contractChanged };
    });
    elements.scenarioHistoryBody.replaceChildren();
    prepared.reverse().forEach((entry) => {
      const meta = statusMeta(entry.status);
      const tableRow = document.createElement("tr");
      const delta =
        typeof entry.delta === "number" && Number.isFinite(entry.delta)
          ? `${entry.delta < 0 ? "↓" : entry.delta > 0 ? "↑" : ""}${compactNumber(
              Math.abs(entry.delta),
              1,
            )}%`
          : "—";
      tableRow.innerHTML = `
        <td><div class="release-cell"><a href="${escapeHtml(
          detailUrl(entry.execution, row),
        )}">${escapeHtml(formatDate(entry.execution.completed_at, true))}</a><span>run ${escapeHtml(
          entry.execution.run_id || entry.execution.id,
        )}</span></div></td>
        <td>${escapeHtml(definition.format(entry.value))}</td>
        <td class="${entry.delta < 0 ? "text-pass" : entry.delta > 0 ? "text-fail" : ""}">${escapeHtml(
          delta,
        )}</td>
        <td><span class="table-status status-${meta.css}">${escapeHtml(meta.label)}</span></td>
        <td><span class="scenario-history-contract">v${escapeHtml(
          entry.metric.scenario_version || 1,
        )}${entry.contractChanged ? " · changed" : ""}</span></td>
      `;
      elements.scenarioHistoryBody.append(tableRow);
    });
  }

  function renderScenarioHistory() {
    const row = state.scenarioHistoryRow;
    if (!row) return;
    const entries = scenarioHistoryEntries(row);
    const metricId = state.scenarioHistoryMetric;
    const definition = scenarioMetricDefinitions[metricId];
    elements.scenarioHistoryTitle.textContent = titleCase(row.scenarioId);
    elements.scenarioHistoryContext.textContent = `${row.subjectId || "default subject"} · ${
      entries.length
    } execution${entries.length === 1 ? "" : "s"} · ${
      row.lifecycle === "retired"
        ? "removed from current suite"
        : `current contract v${row.scenarioVersion}`
    }`;
    elements.scenarioHistoryDescription.textContent =
      `${definition.description} Lines break when the scenario contract changes.`;
    document.querySelectorAll("[data-history-metric]").forEach((button) => {
      const active = button.dataset.historyMetric === metricId;
      button.classList.toggle("active", active);
      button.setAttribute("aria-selected", String(active));
      button.tabIndex = active ? 0 : -1;
    });
    renderScenarioHistoryChart(entries, metricId, row);
    renderScenarioHistoryTable(entries, metricId, row);
  }

  function openScenarioHistory(row) {
    state.scenarioHistoryRow = row;
    state.scenarioHistoryMetric = "cost_usd";
    renderScenarioHistory();
    if (!elements.scenarioHistoryDialog.open) {
      elements.scenarioHistoryDialog.showModal();
    }
  }

  function matrixTooltip(execution, row, cell) {
    const status = statusMeta(cell?.status || execution.status);
    if (!cell) {
      return `${formatDate(execution.completed_at, true)} · ${status.label} · no scenario report`;
    }
    const blocking =
      Number(cell.hard_gate_failures || 0) +
      Number(cell.technical_failures || 0);
    return [
      `${titleCase(row.scenarioId)} · ${formatDate(execution.completed_at, true)}`,
      `${status.label} · score ${compactNumber(cell.median_score, 1)} · pass ${formatPercent(
        typeof cell.pass_rate === "number" ? cell.pass_rate * 100 : null,
      )}`,
      `${formatCurrency(cell.total_cost_usd)} · ${formatDuration(
        cell.wall_time_seconds,
      )} · ${blocking} blocking event${blocking === 1 ? "" : "s"}`,
    ].join("\n");
  }

  function renderMatrix() {
    const executions = history.executions
      .slice(0, state.matrixCount)
      .reverse();
    const rows = executionApi.matrixRows(executions);
    if (!executions.length || !rows.length) {
      elements.matrix.innerHTML =
        '<div class="matrix-empty">No scenario reports are available for this range.</div>';
      return;
    }
    const table = document.createElement("table");
    table.className = "health-matrix";
    const thead = document.createElement("thead");
    const headerRow = document.createElement("tr");
    headerRow.innerHTML = '<th scope="col">Subject / scenario</th>';
    executions.forEach((execution) => {
      const header = document.createElement("th");
      header.scope = "col";
      const status = statusMeta(execution.status);
      header.innerHTML = `
        <a href="${escapeHtml(detailUrl(execution))}" aria-label="${escapeHtml(
          `${formatDate(execution.completed_at, true)}, ${status.label}, run ${execution.run_id}`,
        )}">
          <span>${escapeHtml(
            new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric" }).format(
              new Date(execution.completed_at || execution.started_at),
            ),
          )}</span>
          <i class="matrix-run-status status-${status.css}" aria-hidden="true"></i>
        </a>
      `;
      headerRow.append(header);
    });
    thead.append(headerRow);
    table.append(thead);

    const tbody = document.createElement("tbody");
    rows.forEach((row) => {
      const tableRow = document.createElement("tr");
      const label = document.createElement("th");
      label.scope = "row";
      label.innerHTML = `
        <span>${escapeHtml(row.subjectLabel)}</span>
        <strong>${escapeHtml(titleCase(row.scenarioId))}</strong>
      `;
      tableRow.append(label);
      executions.forEach((execution) => {
        const cell = executionApi.matrixCell(execution, row);
        const cellStatus =
          cell?.status ||
          (["cancelled", "running", "infra_failed"].includes(execution.status)
            ? execution.status
            : "incomplete");
        const meta = statusMeta(cellStatus);
        const cellLabel = executionApi.matrixCellLabel(cell, cellStatus);
        const data = document.createElement("td");
        const tooltip = matrixTooltip(execution, row, cell);
        data.innerHTML = `
          <a
            class="matrix-cell matrix-${escapeHtml(cellStatus)}"
            href="${escapeHtml(detailUrl(execution, row))}"
            aria-label="${escapeHtml(tooltip.replaceAll("\n", ". "))}"
            data-tooltip="${escapeHtml(tooltip)}"
          >
            <span aria-hidden="true">${escapeHtml(cellLabel || meta.short)}</span>
          </a>
        `;
        tableRow.append(data);
      });
      tbody.append(tableRow);
    });
    table.append(tbody);
    elements.matrix.replaceChildren(table);
  }

  function dataLabel(availability) {
    return {
      full: "Diagnostic detail",
      aggregate: "Aggregate",
      unavailable: "No report",
    }[availability] || "Unknown";
  }

  function renderComparisonBar() {
    elements.comparisonBar.hidden = false;
    const count = state.comparison.length;
    elements.comparisonCount.textContent =
      state.comparisonLimitReached
        ? "Two selected · clear one before adding another"
        : count === 0
        ? "Select two executions"
        : count === 1
          ? "1 of 2 executions selected"
          : "2 executions selected";
    const ready = count === 2;
    elements.comparisonLink.setAttribute("aria-disabled", String(!ready));
    elements.comparisonLink.href = ready
      ? `./compare.html?left=${encodeURIComponent(state.comparison[0])}&right=${encodeURIComponent(state.comparison[1])}`
      : "./compare.html";
  }

  function toggleComparison(executionId, checked) {
    state.comparison = state.comparison.filter((id) => id !== executionId);
    state.comparisonLimitReached = false;
    if (checked) {
      if (state.comparison.length === 2) {
        state.comparisonLimitReached = true;
      } else {
        state.comparison.push(executionId);
      }
    }
    renderTable();
  }

  function renderTable() {
    const filtered = executionApi.filterExecutions(history.executions, state);
    const pageCount = Math.max(1, Math.ceil(filtered.length / state.pageSize));
    state.page = Math.min(state.page, pageCount);
    const start = (state.page - 1) * state.pageSize;
    const page = filtered.slice(start, start + state.pageSize);
    elements.body.replaceChildren();
    page.forEach((execution) => {
      const row = document.createElement("tr");
      const meta = statusMeta(execution.status);
      const commit = execution.source?.sha || "";
      const subjectLabels = (execution.subjects || []).map(
        (subject) => `${subject.provider}/${subject.model}`,
      );
      const failures = failureCount(execution);
      const hasFailureEvidence = [
        execution.totals?.hard_gate_failures,
        execution.totals?.technical_failures,
        execution.totals?.missing_reports,
      ].some((value) => typeof value === "number" && Number.isFinite(value));
      const expectedReports = execution.totals?.expected_reports;
      const receivedReports = execution.totals?.received_reports;
      const scope =
        typeof expectedReports === "number" && typeof receivedReports === "number"
          ? `${compactNumber(receivedReports, 0)}/${compactNumber(expectedReports, 0)}`
          : "—";
      const firstFailure = execution.first_failure?.message || "";
      const evidencePreview = firstFailure
        ? firstFailure
        : hasFailureEvidence
          ? failures
            ? `${failures} blocking event${failures === 1 ? "" : "s"}`
            : "No blocking events"
          : "No structured report signal";
      const trigger =
        execution.event === "workflow_dispatch" ? "manual" : execution.event || "unknown";
      const primaryLabel = execution.label || formatDate(execution.completed_at, true);
      const secondaryLabel = execution.label
        ? `${formatDate(execution.completed_at, true)} · run ${execution.run_id || execution.id}`
        : `run ${execution.run_id || execution.id}`;
      const comparisonControl = `<label class="execution-compare-control">
            <input type="checkbox" data-compare-id="${escapeHtml(execution.id)}" ${
              state.comparison.includes(execution.id) ? "checked" : ""
            }>
            <span class="visually-hidden">Select ${escapeHtml(primaryLabel)} for comparison</span>
          </label>`;
      const commitCell = isLocal
        ? ""
        : `<td>${
            commit
              ? `<a class="commit-link" href="${escapeHtml(
                  safeUrl(
                    `${history.repoUrl.replace(/\/$/, "")}/commit/${encodeURIComponent(commit)}`,
                  ),
                )}">${escapeHtml(commit.slice(0, 7))}</a>`
              : "—"
          }</td>`;
      row.innerHTML = `
        <td>
          <div class="execution-identity-cell">
            ${comparisonControl}
            <div class="release-cell">
            <a href="${escapeHtml(detailUrl(execution))}">${escapeHtml(
              primaryLabel,
            )}</a>
            <span>${escapeHtml(secondaryLabel)} · attempt ${execution.attempt} · ${escapeHtml(trigger)}</span>
            </div>
          </div>
        </td>
        <td><span class="table-status status-${meta.css}">${meta.label}</span></td>
        ${commitCell}
        <td title="${escapeHtml(subjectLabels.join(", "))}">${escapeHtml(
          subjectLabels.length === 1
            ? subjectLabels[0]
            : subjectLabels.length
              ? `${subjectLabels.length} subjects`
              : "—",
        )}</td>
        <td>
          <div class="execution-table-stack">
            <strong>${scope}</strong>
            <small>${formatPercent(execution.totals?.report_coverage)} coverage</small>
          </div>
        </td>
        <td>
          <div class="execution-table-stack">
            <strong>${formatPercent(execution.totals?.scenario_pass_rate)}</strong>
            <small>score ${compactNumber(execution.totals?.average_score, 1)}</small>
          </div>
        </td>
        <td>
          <div class="execution-table-stack">
            <strong>${formatCurrency(execution.totals?.total_cost_usd)}</strong>
            <small>${formatDuration(execution.totals?.wall_time_seconds)} · ${compactNumber(execution.totals?.total_tokens, 0)} tokens</small>
          </div>
        </td>
        <td>
          <div class="execution-evidence-cell ${failures ? "text-fail" : ""}" title="${escapeHtml(evidencePreview)}">
            <span>${escapeHtml(evidencePreview)}</span>
            <span class="data-badge data-${escapeHtml(execution.availability)}">${escapeHtml(dataLabel(execution.availability))}</span>
          </div>
        </td>
      `;
      row.querySelector("[data-compare-id]")?.addEventListener("change", (event) => {
        toggleComparison(execution.id, event.currentTarget.checked);
      });
      elements.body.append(row);
    });
    if (!page.length) {
      const row = document.createElement("tr");
      row.innerHTML =
        `<td class="table-empty" colspan="${isLocal ? 7 : 8}">No executions match these filters.</td>`;
      elements.body.append(row);
    }
    elements.count.textContent =
      `${filtered.length} execution${filtered.length === 1 ? "" : "s"}`;
    elements.pageLabel.textContent = `Page ${state.page} of ${pageCount}`;
    elements.previous.disabled = state.page === 1;
    elements.next.disabled = state.page === pageCount;
    renderComparisonBar();
  }

  function render() {
    const hasData = history.executions.length > 0;
    elements.empty.hidden = hasData;
    elements.content.hidden = !hasData;
    if (!hasData) return;
    renderLatestHealth();
    renderKpis();
    renderCapability();
    renderEfficiency();
    renderMatrix();
    renderTable();
  }

  async function initialize() {
    elements.preview.hidden = !(history.preview || isLocal || isObserved);
    elements.preview.textContent = isLocal
      ? "Local data"
      : isObserved
        ? "Observed reports"
        : "Preview data";
    if (isLocal) {
      elements.syncLabel.textContent = "Last completed";
      elements.emptyTitle.textContent = "No local executions yet";
      elements.emptyDescription.textContent =
        "Use New execution to create the first local experiment.";
      elements.actionsLink.textContent = "View repository ↗";
      elements.footerSummary.textContent = "Harness E2E · local execution history";
      elements.localRunner.hidden = false;
      elements.localRunnerOpen.hidden = false;
      elements.commitHeading.hidden = true;
      elements.search.placeholder = "Search label, run, or date";
    } else if (isObserved) {
      elements.syncLabel.textContent = "Last completed";
      elements.actionsLink.textContent = "View repository ↗";
      elements.footerSummary.textContent =
        "Harness E2E · observed control-plane reports";
      elements.search.placeholder = "Search scenario, run, commit, or date";
    }
    const lastUpdate = history.lastUpdate || history.executions[0]?.completed_at;
    if (lastUpdate) {
      elements.lastUpdate.dateTime = new Date(lastUpdate).toISOString();
      elements.lastUpdate.textContent = formatDate(lastUpdate, true);
    }
    if (history.repoUrl) {
      const repo = safeUrl(history.repoUrl);
      if (repo) {
        elements.actionsLink.href = isLocal
          ? repo
          : `${repo.replace(/\/$/, "")}/actions/workflows/harness-e2e-daily.yml`;
      }
    }
    elements.localRunnerOpen.addEventListener("click", () => {
      if (isLocal) elements.localRunnerDialog.showModal();
    });
    elements.localRunnerClose.addEventListener("click", () => {
      elements.localRunnerDialog.close();
    });
    elements.localRunnerDialog.addEventListener("click", (event) => {
      if (event.target === elements.localRunnerDialog) {
        elements.localRunnerDialog.close();
      }
    });
    elements.overviewComparisonLeft.addEventListener(
      "change",
      renderOverviewComparison,
    );
    elements.overviewComparisonRight.addEventListener(
      "change",
      renderOverviewComparison,
    );
    elements.overviewComparisonSwap.addEventListener("click", () => {
      const left = elements.overviewComparisonLeft.value;
      elements.overviewComparisonLeft.value = elements.overviewComparisonRight.value;
      elements.overviewComparisonRight.value = left;
      renderOverviewComparison();
    });
    elements.scenarioHistoryClose.addEventListener("click", () => {
      elements.scenarioHistoryDialog.close();
    });
    elements.scenarioHistoryDialog.addEventListener("click", (event) => {
      if (event.target === elements.scenarioHistoryDialog) {
        elements.scenarioHistoryDialog.close();
      }
    });
    elements.scenarioHistoryDialog.addEventListener("close", () => {
      state.scenarioHistoryRow = null;
    });
    document.querySelectorAll("[data-history-metric]").forEach((button) => {
      button.addEventListener("click", () => {
        const metricId = button.dataset.historyMetric;
        if (!scenarioHistoryMetricIds.includes(metricId)) return;
        state.scenarioHistoryMetric = metricId;
        renderScenarioHistory();
      });
    });
    document.querySelectorAll(".range-button").forEach((button) => {
      button.addEventListener("click", () => {
        state.matrixCount = Number(button.dataset.count);
        document.querySelectorAll(".range-button").forEach((candidate) => {
          candidate.classList.toggle("active", candidate === button);
        });
        renderMatrix();
      });
    });
    elements.search.addEventListener("input", () => {
      state.query = elements.search.value;
      state.page = 1;
      renderTable();
    });
    elements.status.addEventListener("change", () => {
      state.status = elements.status.value;
      state.page = 1;
      renderTable();
    });
    elements.event.addEventListener("change", () => {
      state.event = elements.event.value;
      state.page = 1;
      renderTable();
    });
    elements.previous.addEventListener("click", () => {
      state.page = Math.max(1, state.page - 1);
      renderTable();
    });
    elements.next.addEventListener("click", () => {
      state.page += 1;
      renderTable();
    });
    initializeOverviewComparison();
    if (isLocal) window.HarnessLocalRunner.initialize();
    render();
    await hydrateExecutionMetrics();
    renderEfficiency();
    renderOverviewComparison();
    renderTable();
  }

  initialize();
})();
