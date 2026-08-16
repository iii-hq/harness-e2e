(function renderHarnessExecutionOverview() {
  "use strict";

  const benchmarkApi = window.HarnessBenchmarkData;
  const executionApi = window.HarnessExecutionData;
  const benchmarkData = benchmarkApi.normalizeBenchmarkData(window.BENCHMARK_DATA);
  const history = executionApi.mergeExecutionHistory(window.HARNESS_EXECUTIONS);
  const isLocal = history.mode === "local";
  const isObserved = history.mode === "observed";
  const remotePaging = Boolean(
    isLocal && window.HarnessDashboardData?.remotePaging,
  );
  const state = {
    page: 1,
    pageSize: 25,
    query: "",
    status: "all",
    event: "all",
    tableExecutions: history.executions,
    tableTotal:
      Number(window.HARNESS_EXECUTIONS?.total) || history.executions.length,
    tableLoading: false,
    tableError: "",
  };
  let tableLoadSequence = 0;
  let searchTimer = null;
  const elements = {
    actionsLink: document.querySelector("#actions-link"),
    body: document.querySelector("#execution-body"),
    capabilityBody: document.querySelector("#capability-body"),
    capabilityPolicy: document.querySelector("#capability-policy"),
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
    event: document.querySelector("#event-filter"),
    footerSummary: document.querySelector("#dashboard-footer-summary"),
    commitHeading: document.querySelector("#execution-commit-heading"),
    kpiCost: document.querySelector("#kpi-cost"),
    kpiCoverage: document.querySelector("#kpi-coverage"),
    kpiFailures: document.querySelector("#kpi-failures"),
    kpiPassRate: document.querySelector("#kpi-pass-rate"),
    kpiRuntime: document.querySelector("#kpi-runtime"),
    lastUpdate: document.querySelector("#last-update"),
    latestAvailability: document.querySelector("#latest-health-availability"),
    latestCompleted: document.querySelector("#latest-health-completed"),
    latestDetailLink: document.querySelector("#latest-detail-link"),
    latestFirstFailure: document.querySelector("#latest-first-failure"),
    latestIdentity: document.querySelector("#latest-health-identity"),
    latestLane: document.querySelector("#latest-health-lane"),
    latestStatus: document.querySelector("#latest-health-status"),
    latestSummary: document.querySelector("#latest-health-summary"),
    latestTimeLabel: document.querySelector("#latest-health-time-label"),
    latestTitle: document.querySelector("#latest-health-title"),
    latestWorkflowLink: document.querySelector("#latest-workflow-link"),
    localRunnerClose: document.querySelector("#close-local-runner"),
    localRunnerDialog: document.querySelector("#local-runner-dialog"),
    localRunnerOpen: document.querySelector("#open-local-runner"),
    localRunner: document.querySelector("#local-runner"),
    next: document.querySelector("#next-page"),
    pageLabel: document.querySelector("#page-label"),
    preview: document.querySelector("#preview-badge"),
    previous: document.querySelector("#previous-page"),
    search: document.querySelector("#execution-search"),
    syncLabel: document.querySelector("#sync-label"),
    status: document.querySelector("#status-filter"),
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

  function detailUrl(execution) {
    return window.HarnessDashboardRoutes.execution(execution.id);
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
      cancelling: { label: "Cancelling", short: "•", css: "running" },
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
    const active = ["running", "cancelling"].includes(health.status);

    elements.latestStatus.className = `status-pill status-${meta.css}`;
    elements.latestStatus.textContent = meta.label;
    elements.latestTitle.textContent =
      active
        ? health.status === "cancelling"
          ? "Cancellation is in progress"
          : "Execution is still running"
        : health.status === "passed"
        ? "Latest execution passed"
        : health.availability === "unavailable"
          ? "Execution stopped before report evidence"
          : `${meta.label} needs attention`;
    elements.latestSummary.textContent = active
      ? hasReportDenominator
        ? `${compactNumber(received, 0)} of ${compactNumber(expected, 0)} expected reports received so far. Results will update as scenarios complete.`
        : "Report evidence will appear as scenarios complete. Quality and reliability remain pending while this execution is active."
      : hasReportDenominator
      ? `${compactNumber(received, 0)} of ${compactNumber(expected, 0)} expected reports received. ${dataLabel(health.availability)} is retained for this execution.`
      : health.availability === "unavailable"
        ? "No scenario denominator is available, so quality and reliability metrics remain unknown instead of being shown as zero."
        : `${dataLabel(health.availability)} is retained; report completeness was not published.`;
    elements.latestIdentity.textContent = health.identity;
    elements.latestLane.textContent = titleCase(health.lane);
    elements.latestAvailability.textContent = dataLabel(health.availability);
    elements.latestTimeLabel.textContent = active ? "Started" : "Completed";
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
    } else if (active) {
      signalTitle.textContent = "Waiting for report evidence";
      signalMessage.textContent =
        "No blocking failure has been reported. Open the execution to follow live progress.";
      elements.latestFirstFailure.classList.remove("has-failure");
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

  function dataLabel(availability) {
    return {
      full: "Diagnostic detail",
      aggregate: "Aggregate",
      unavailable: "No report",
    }[availability] || "Unknown";
  }

  function renderTable() {
    const filtered = remotePaging
      ? state.tableExecutions
      : executionApi.filterExecutions(history.executions, state);
    const total = remotePaging ? state.tableTotal : filtered.length;
    const pageCount = Math.max(1, Math.ceil(total / state.pageSize));
    state.page = Math.min(state.page, pageCount);
    const start = remotePaging ? 0 : (state.page - 1) * state.pageSize;
    const page = filtered.slice(start, start + state.pageSize);
    elements.body.replaceChildren();
    page.forEach((execution) => {
      const row = document.createElement("tr");
      const meta = statusMeta(execution.status);
      const executionActive = ["running", "cancelling"].includes(
        execution.status,
      );
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
        : executionActive
          ? "Waiting for report evidence"
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
      const commitCell = isLocal
        ? ""
        : `<td data-label="Commit">${
            commit
              ? `<a class="commit-link" href="${escapeHtml(
                  safeUrl(
                    `${history.repoUrl.replace(/\/$/, "")}/commit/${encodeURIComponent(commit)}`,
                  ),
                )}">${escapeHtml(commit.slice(0, 7))}</a>`
              : "—"
          }</td>`;
      row.innerHTML = `
        <td data-label="Execution">
          <div class="execution-identity-cell">
            <div class="release-cell">
            <a href="${escapeHtml(detailUrl(execution))}">${escapeHtml(
              primaryLabel,
            )}</a>
            <span>${escapeHtml(secondaryLabel)} · attempt ${execution.attempt} · ${escapeHtml(trigger)}</span>
            </div>
          </div>
        </td>
        <td data-label="Result"><span class="table-status status-${meta.css}">${meta.label}</span></td>
        ${commitCell}
        <td data-label="Subject" title="${escapeHtml(subjectLabels.join(", "))}">${escapeHtml(
          subjectLabels.length === 1
            ? subjectLabels[0]
            : subjectLabels.length
              ? `${subjectLabels.length} subjects`
              : "—",
        )}</td>
        <td data-label="Scope">
          <div class="execution-table-stack">
            <strong>${scope}</strong>
            <small>${formatPercent(execution.totals?.report_coverage)} coverage</small>
          </div>
        </td>
        <td data-label="Outcome">
          <div class="execution-table-stack">
            <strong>${formatPercent(execution.totals?.scenario_pass_rate)}</strong>
            <small>${executionActive ? "Pending report evidence" : hasFailureEvidence ? failures ? `${failures} blocking event${failures === 1 ? "" : "s"}` : "No blocking events" : "Reliability unavailable"}</small>
          </div>
        </td>
        <td data-label="Efficiency">
          <div class="execution-table-stack">
            <strong>${formatCurrency(execution.totals?.total_cost_usd)}</strong>
            <small>${formatDuration(execution.totals?.wall_time_seconds)} · ${compactNumber(execution.totals?.total_tokens, 0)} tokens</small>
          </div>
        </td>
        <td data-label="Evidence">
          <div class="execution-evidence-cell ${failures ? "text-fail" : ""}" title="${escapeHtml(evidencePreview)}">
            <span>${escapeHtml(evidencePreview)}</span>
            <span class="data-badge data-${escapeHtml(execution.availability)}">${escapeHtml(dataLabel(execution.availability))}</span>
          </div>
        </td>
      `;
      elements.body.append(row);
    });
    if (!page.length) {
      const row = document.createElement("tr");
      row.innerHTML =
        `<td class="table-empty" colspan="${isLocal ? 7 : 8}">${state.tableLoading ? "Loading executions…" : state.tableError ? escapeHtml(state.tableError) : "No executions match these filters."}</td>`;
      elements.body.append(row);
    }
    elements.count.textContent = state.tableError
      ? `Refresh failed · ${total} cached execution${total === 1 ? "" : "s"}`
      : `${total} execution${total === 1 ? "" : "s"}`;
    elements.pageLabel.textContent = `Page ${state.page} of ${pageCount}`;
    elements.previous.disabled = state.tableLoading || state.page === 1;
    elements.next.disabled = state.tableLoading || state.page === pageCount;
  }

  async function loadRemoteTablePage() {
    if (!remotePaging) {
      renderTable();
      return;
    }
    const sequence = ++tableLoadSequence;
    state.tableLoading = true;
    state.tableError = "";
    renderTable();
    try {
      const manifest = await window.HarnessDashboardData.listExecutions({
        cursor: String((state.page - 1) * state.pageSize),
        limit: state.pageSize,
        query: state.query,
        status: state.status,
        event: state.event,
      });
      if (sequence !== tableLoadSequence) return;
      state.tableExecutions = (manifest.executions || []).map(
        executionApi.normalizeExecution,
      );
      state.tableTotal = Number(manifest.total) || 0;
    } catch (_error) {
      if (sequence !== tableLoadSequence) return;
      state.tableError =
        "Executions could not be refreshed. Showing the last available data.";
    } finally {
      if (sequence === tableLoadSequence) {
        state.tableLoading = false;
        renderTable();
      }
    }
  }

  function render() {
    const hasData = history.executions.length > 0;
    elements.empty.hidden = hasData;
    elements.content.hidden = !hasData;
    if (!hasData) return;
    renderLatestHealth();
    renderKpis();
    renderCapability();
    renderTable();
  }

  async function initialize() {
    const latestActive = ["running", "cancelling"].includes(
      history.executions[0]?.status,
    );
    elements.preview.hidden = !(history.preview || isLocal || isObserved);
    elements.preview.textContent = isLocal
      ? "Local data"
      : isObserved
        ? "Observed reports"
        : "Preview data";
    if (isLocal) {
      elements.syncLabel.textContent = latestActive ? "Last started" : "Last completed";
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
      elements.syncLabel.textContent = latestActive ? "Last started" : "Last completed";
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
      if (isLocal) {
        window.HarnessLocalRunner?.open?.();
        elements.localRunnerDialog.showModal();
      }
    });
    elements.localRunnerClose.addEventListener("click", () => {
      elements.localRunnerDialog.close();
    });
    elements.localRunnerDialog.addEventListener("click", (event) => {
      if (event.target === elements.localRunnerDialog) {
        elements.localRunnerDialog.close();
      }
    });
    elements.search.addEventListener("input", () => {
      state.query = elements.search.value;
      state.page = 1;
      clearTimeout(searchTimer);
      searchTimer = setTimeout(loadRemoteTablePage, remotePaging ? 220 : 0);
    });
    elements.status.addEventListener("change", () => {
      state.status = elements.status.value;
      state.page = 1;
      loadRemoteTablePage();
    });
    elements.event.addEventListener("change", () => {
      state.event = elements.event.value;
      state.page = 1;
      loadRemoteTablePage();
    });
    elements.previous.addEventListener("click", () => {
      state.page = Math.max(1, state.page - 1);
      loadRemoteTablePage();
    });
    elements.next.addEventListener("click", () => {
      state.page += 1;
      loadRemoteTablePage();
    });
    if (isLocal) window.HarnessLocalRunner.initialize();
    render();

    const refreshTableForCurrentRoute = () => {
      const route = window.HarnessDashboardRoutes.current();
      if (
        remotePaging &&
        route.page === "overview" &&
        route.view === "executions"
      ) {
        void loadRemoteTablePage();
      }
    };
    window.addEventListener("hashchange", refreshTableForCurrentRoute);
    refreshTableForCurrentRoute();
  }

  initialize();
})();
