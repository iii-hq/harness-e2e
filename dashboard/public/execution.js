(function renderHarnessExecutionDetail() {
  "use strict";

  const history = window.HarnessExecutionData.mergeExecutionHistory(
    window.HARNESS_EXECUTIONS,
  );
  const routes = window.HarnessDashboardRoutes;
  const initialRoute = routes.current();
  const executionId = initialRoute.page === "execution" ? initialRoute.executionId : "";
  const execution = window.HarnessExecutionData.findExecution(history, executionId);
  let detail = null;

  const elements = {
    actions: document.querySelector("#execution-actions"),
    availability: document.querySelector("#availability-badge"),
    breadcrumb: document.querySelector("#breadcrumb-run"),
    configuration: document.querySelector("#configuration-content"),
    configurationDigest: document.querySelector("#configuration-digest"),
    content: document.querySelector("#detail-content"),
    cost: document.querySelector("#detail-cost"),
    coverage: document.querySelector("#detail-coverage"),
    error: document.querySelector("#detail-error"),
    errorMessage: document.querySelector("#detail-error-message"),
    failureBox: document.querySelector("#detail-failures"),
    failureSummary: document.querySelector("#failure-summary"),
    failureTitle: document.querySelector("#failures-title"),
    loading: document.querySelector("#detail-loading"),
    metadata: document.querySelector("#metadata-grid"),
    overviewDigest: document.querySelector("#overview-digest"),
    passRate: document.querySelector("#detail-pass-rate"),
    rawActions: document.querySelector("#raw-actions"),
    rawJson: document.querySelector("#raw-json"),
    rawPreview: document.querySelector("#raw-preview"),
    runtime: document.querySelector("#detail-runtime"),
    scenarioDetails: document.querySelector("#scenario-details"),
    scenarioIntro: document.querySelector("#scenario-intro"),
    status: document.querySelector("#execution-status"),
    subtitle: document.querySelector("#execution-subtitle"),
    transcriptBody: document.querySelector("#session-transcript-body"),
    transcriptClose: document.querySelector("#session-transcript-close"),
    transcriptContext: document.querySelector("#session-transcript-context"),
    transcriptDialog: document.querySelector("#session-transcript-dialog"),
    transcriptTitle: document.querySelector("#session-transcript-title"),
    title: document.querySelector("#execution-title"),
    workflowLink: document.querySelector("#workflow-link"),
    workflowRuntime: document.querySelector("#workflow-runtime"),
  };
  const ui = {
    configCard:
      "config-card rounded-none border-0 border-t border-line bg-transparent px-0 py-[18px] [&_dl]:grid-cols-2 [&_h3]:mb-4 [&_h3]:[overflow-wrap:anywhere]",
    configFacts: "config-facts mt-0 grid grid-cols-2 gap-x-[18px] max-[560px]:grid-cols-1",
    configGrid: "config-grid mt-0 grid grid-cols-1 gap-0",
    detailMetric:
      "detail-metric rounded-none border-0 border-r border-b border-line bg-transparent",
    metadataItem:
      "metadata-item grid min-h-[68px] min-w-0 gap-[7px] rounded-none border-0 border-t border-line bg-transparent px-0 py-3.5",
    metadataLabel:
      "text-[0.61rem] font-semibold tracking-[0.055em] text-ink-muted uppercase",
    metadataValue:
      "overflow-hidden text-ellipsis text-[0.73rem] leading-[1.45] font-[570] text-ink-soft no-underline",
    metricGrid:
      "grid grid-cols-4 gap-0 overflow-hidden rounded-[7px] border border-line max-[840px]:grid-cols-2 max-[560px]:grid-cols-1",
    scenarioBody: "scenario-detail-body border-t border-line px-[18px] pt-0 pb-[18px]",
    scenarioCard:
      "scenario-detail-card scroll-mt-[84px] rounded-[7px] border border-line bg-transparent transition-[border-color,background,box-shadow] duration-150 open:bg-panel-faint target:border-brand target:border-l-[3px] target:border-l-brand target:bg-brand-soft target:shadow-[inset_0_0_0_1px_rgba(199,255,74,0.035)] [&.is-focused]:border-brand [&.is-focused]:border-l-[3px] [&.is-focused]:border-l-brand [&.is-focused]:bg-brand-soft [&.is-focused]:shadow-[inset_0_0_0_1px_rgba(199,255,74,0.035)]",
    scenarioSummary:
      "scenario-summary-row grid min-h-[72px] list-none grid-cols-[minmax(200px,1fr)_auto_auto_auto_auto_auto] items-center gap-[18px] px-[18px] py-3.5 max-[560px]:grid-cols-[minmax(0,1fr)_auto_auto] max-[560px]:[&>.scenario-summary-stat]:hidden",
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

  function formatDate(timestamp) {
    if (!timestamp || Number.isNaN(Date.parse(timestamp))) return "Unknown date";
    return new Intl.DateTimeFormat("en-US", {
      month: "short",
      day: "numeric",
      year: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    }).format(new Date(timestamp));
  }

  function statusMeta(status) {
    const normalized = String(status || "").toLowerCase();
    if (normalized === "passed" || normalized === "success") {
      return { label: "Passed", css: "pass" };
    }
    if (normalized === "running") return { label: "Running", css: "running" };
    if (normalized === "cancelling") {
      return { label: "Cancelling", css: "running" };
    }
    if (normalized === "cancelled") return { label: "Cancelled", css: "cancelled" };
    if (
      normalized === "failed" ||
      normalized.endsWith("_failed") ||
      normalized.endsWith("_error") ||
      normalized === "failure" ||
      normalized === "resource_limit"
    ) {
      return { label: titleCase(normalized), css: "fail" };
    }
    return { label: titleCase(normalized || "incomplete"), css: "incomplete" };
  }

  function availabilityLabel(availability) {
    return {
      full: "Full report",
      aggregate: "Aggregate only",
      unavailable: "No report",
    }[availability] || "Unknown data";
  }

  function scenarioAnchor(subjectId, scenarioId) {
    return `scenario-${subjectId}-${scenarioId}`.replace(/[^a-zA-Z0-9_-]/g, "-");
  }

  function runAnchor(subjectId, scenarioId, index) {
    return `run-${subjectId}-${scenarioId}-${index + 1}`.replace(
      /[^a-zA-Z0-9_-]/g,
      "-",
    );
  }

  function blockingFailures() {
    const totals = execution.totals || {};
    return (
      Number(totals.hard_gate_failures || 0) +
      Number(totals.technical_failures || 0) +
      Number(totals.missing_reports || 0)
    );
  }

  function metadataItem(label, value, options = {}) {
    const content = options.href
      ? `<a class="${ui.metadataValue}" href="${escapeHtml(options.href)}">${escapeHtml(value)} <span aria-hidden="true">↗</span></a>`
      : `<strong class="${ui.metadataValue}${options.mono ? " mono font-mono" : ""}">${escapeHtml(value)}</strong>`;
    return `<div class="${ui.metadataItem}"><span class="${ui.metadataLabel}">${escapeHtml(label)}</span>${content}</div>`;
  }

  function renderHeader() {
    const meta = statusMeta(execution.status);
    const runLabel = execution.run_id || execution.id;
    elements.breadcrumb.textContent = `Run ${runLabel}`;
    elements.title.textContent = runLabel;
    elements.status.className = `status-pill status-${meta.css}`;
    elements.status.textContent = meta.label;
    elements.availability.className =
      `data-badge data-${execution.availability}`;
    elements.availability.textContent = availabilityLabel(execution.availability);
    elements.subtitle.textContent =
      `${formatDate(execution.completed_at || execution.started_at)} · ` +
      `attempt ${execution.attempt} · ${titleCase(execution.event || "unknown trigger")}`;
    document.title = `Run ${runLabel} · Harness E2E`;

    const workflowUrl = safeUrl(execution.workflow_url);
    elements.workflowLink.hidden = !workflowUrl;
    if (workflowUrl) elements.workflowLink.href = workflowUrl;
    const commit = execution.source?.sha || "";
    const repo = history.repoUrl.replace(/\/$/, "");
    const commitUrl = commit && repo ? safeUrl(`${repo}/commit/${commit}`) : "";
    elements.actions.innerHTML = `
      ${commitUrl ? `<a class="button" href="${escapeHtml(commitUrl)}">Commit ${escapeHtml(commit.slice(0, 7))} <span aria-hidden="true">↗</span></a>` : ""}
      ${workflowUrl ? `<a class="button" href="${escapeHtml(workflowUrl)}">Open in Actions <span aria-hidden="true">↗</span></a>` : ""}
    `;
  }

  function renderKpis() {
    const totals = execution.totals || {};
    elements.passRate.textContent = formatPercent(totals.scenario_pass_rate);
    elements.coverage.textContent =
      `${formatPercent(totals.report_coverage)} report coverage`;
    elements.cost.textContent = formatCurrency(totals.total_cost_usd);
    elements.runtime.textContent = formatDuration(totals.wall_time_seconds);
    elements.workflowRuntime.textContent =
      `${formatDuration(execution.workflow_duration_seconds)} workflow elapsed`;
  }

  function renderMetadata() {
    const source = execution.source || {};
    const release = execution.release || {};
    elements.overviewDigest.textContent = [
      source.sha ? source.sha.slice(0, 7) : "unknown commit",
      titleCase(execution.event || "unknown trigger"),
      execution.actor || "unknown actor",
    ].join(" · ");
    elements.metadata.innerHTML = [
      metadataItem("Workflow run", execution.run_id || "Unknown", { mono: true }),
      metadataItem("Attempt", execution.attempt),
      metadataItem("Workflow conclusion", titleCase(execution.conclusion || "unknown")),
      metadataItem("Benchmark result", titleCase(execution.status)),
      metadataItem("Trigger", titleCase(execution.event || "unknown")),
      metadataItem("Actor", execution.actor || "Unknown"),
      metadataItem("Started", formatDate(execution.started_at)),
      metadataItem("Completed", formatDate(execution.completed_at)),
      metadataItem("Commit", source.sha ? source.sha.slice(0, 12) : "Unknown", {
        mono: true,
      }),
      metadataItem("Source ref", source.ref || "Unknown", { mono: true }),
      metadataItem("Lane", execution.lane || "Unknown"),
      metadataItem("Release", release.tag || release.version || "Daily"),
    ].join("");
  }

  function capability(value) {
    if (value === true) return "Yes";
    if (value === false) return "No";
    return "Unknown";
  }

  function renderConfiguration() {
    const reports = (detail?.reports || [])
      .filter((record) => record?.available && record.report)
      .map((record) => record.report);
    const fullSubjects = new Map();
    const judges = new Map();
    reports.forEach((report) => {
      if (report.subject) {
        fullSubjects.set(
          `${report.subject.provider}/${report.subject.model}`,
          report.subject,
        );
      }
      if (report.judge) {
        judges.set(`${report.judge.provider}/${report.judge.model}`, report.judge);
      }
    });
    const subjectCards = (
      fullSubjects.size
        ? [...fullSubjects.values()]
        : execution.subjects.map((subject) => ({
            model: subject.model,
            provider: subject.provider,
          }))
    )
      .map(
        (subject) => `
          <article class="${ui.configCard}">
            <span>Subject</span>
            <h3>${escapeHtml(subject.provider)}/${escapeHtml(subject.model)}</h3>
            <dl>
              <div><dt>Context</dt><dd>${compactNumber(subject.context_window, 0)}</dd></div>
              <div><dt>Max output</dt><dd>${compactNumber(subject.max_output_tokens, 0)}</dd></div>
              <div><dt>Tools</dt><dd>${capability(subject.supports_tools)}</dd></div>
              <div><dt>Vision</dt><dd>${capability(subject.supports_vision)}</dd></div>
            </dl>
          </article>
        `,
      )
      .join("");
    const judgeCards = [...judges.values()]
      .map(
        (judge) => `
          <article class="${ui.configCard}">
            <span>Judge</span>
            <h3>${escapeHtml(judge.provider)}/${escapeHtml(judge.model)}</h3>
            <dl>
              <div><dt>Context</dt><dd>${compactNumber(judge.context_window, 0)}</dd></div>
              <div><dt>Max output</dt><dd>${compactNumber(judge.max_output_tokens, 0)}</dd></div>
              <div><dt>Tools</dt><dd>${capability(judge.supports_tools)}</dd></div>
              <div><dt>Vision</dt><dd>${capability(judge.supports_vision)}</dd></div>
            </dl>
          </article>
        `,
      )
      .join("");
    const revisions = [
      ...new Set(
        reports
          .map((report) => report.engine_revision)
          .concat(execution.subjects.map((subject) => subject.engine_revision))
          .filter(Boolean),
      ),
    ];
    const protocols = [
      ...new Set(reports.map((report) => report.judge_protocol).filter(Boolean)),
    ];
    const digestSubject =
      [...fullSubjects.values()][0] || execution.subjects[0] || null;
    const digestJudge = [...judges.values()][0] || null;
    elements.configurationDigest.textContent = [
      digestSubject
        ? `${digestSubject.provider}/${digestSubject.model}`
        : "unknown subject",
      digestJudge ? `judge ${digestJudge.provider}/${digestJudge.model}` : "",
      execution.requested_runs != null
        ? `${execution.requested_runs} runs requested`
        : "",
    ]
      .filter(Boolean)
      .join(" · ");
    elements.configuration.innerHTML = `
      <div class="${ui.configGrid}">${subjectCards}${judgeCards}</div>
      <div class="${ui.configFacts}">
        ${metadataItem("Engine revision", revisions.join(", ") || "Unknown", { mono: true })}
        ${metadataItem("Judge protocol", protocols.join(", ") || "Unknown")}
        ${metadataItem("Requested runs", execution.requested_runs ?? "Unknown")}
        ${metadataItem("Report availability", availabilityLabel(execution.availability))}
      </div>
    `;
  }

  function scenarioSummaryRow(options) {
    return `
      <summary class="${ui.scenarioSummary}">
        <span class="scenario-summary-copy">
          <strong>${escapeHtml(options.title)}</strong>
          <small>${escapeHtml(options.subjectLabel)}</small>
        </span>
        <span class="scenario-summary-stat">
          <small>Median</small>
          <strong>${escapeHtml(options.median)}</strong>
        </span>
        <span class="scenario-summary-stat">
          <small>Pass rate</small>
          <strong>${escapeHtml(options.passRate)}</strong>
        </span>
        <span class="scenario-summary-stat">
          <small>Cost</small>
          <strong>${escapeHtml(options.cost)}</strong>
        </span>
        <span class="table-status status-${options.statusCss}">${options.statusLabel}</span>
        <span class="scenario-chevron" aria-hidden="true">⌄</span>
      </summary>
    `;
  }

  function renderAggregateScenario(subject, scenario, options = {}) {
    const meta = statusMeta(
      window.HarnessExecutionData.normalizeScenarioStatus(scenario),
    );
    const anchor = scenarioAnchor(subject.id, scenario.id);
    const open = scenario.passed === false || options.soleScenario;
    return `
      <details id="${escapeHtml(anchor)}" class="${ui.scenarioCard}" ${open ? "open" : ""}>
        ${scenarioSummaryRow({
          title: titleCase(scenario.id),
          subjectLabel: `${subject.provider}/${subject.model}`,
          median: compactNumber(scenario.median_score, 1),
          passRate: formatPercent(
            typeof scenario.pass_rate === "number" ? scenario.pass_rate * 100 : null,
          ),
          cost: formatCurrency(scenario.total_cost_usd),
          statusCss: meta.css,
          statusLabel: meta.label,
        })}
        <div class="${ui.scenarioBody}">
          <div class="scenario-detail-metrics ${ui.metricGrid}">
            ${metricBlock("Median score", compactNumber(scenario.median_score, 1))}
            ${metricBlock("Pass rate", formatPercent(typeof scenario.pass_rate === "number" ? scenario.pass_rate * 100 : null))}
            ${metricBlock("Cost", formatCurrency(scenario.total_cost_usd))}
            ${metricBlock("Runtime", formatDuration(scenario.wall_time_seconds))}
            ${metricBlock("Hard gates", compactNumber(scenario.hard_gate_failures, 0))}
            ${metricBlock("Technical", compactNumber(scenario.technical_failures, 0))}
            ${metricBlock("Retries", compactNumber(scenario.retries, 0))}
            ${metricBlock("Runs", compactNumber(scenario.runs, 0))}
          </div>
          <div class="aggregate-expired">
            Per-run chat is available only while the full execution detail is retained.
          </div>
        </div>
      </details>
    `;
  }

  function metricBlock(label, value, caption = "") {
    return `
      <div class="${ui.detailMetric}">
        <span>${escapeHtml(label)}</span>
        <strong>${escapeHtml(value)}</strong>
        ${caption ? `<small>${escapeHtml(caption)}</small>` : ""}
      </div>
    `;
  }

  function renderFailureList(failures) {
    if (!failures?.length) return '<div class="clean-state">No recorded failures.</div>';
    return `
      <ul class="diagnostic-list">
        ${failures
          .map(
            (failure) => `
              <li>
                <span>${escapeHtml(titleCase(failure.phase || "failure"))}</span>
                <p>${escapeHtml(failure.message || "No failure message")}</p>
              </li>
            `,
          )
          .join("")}
      </ul>
    `;
  }

  function renderGateTable(gates) {
    if (!gates?.length) return '<div class="clean-state">No hard gates recorded.</div>';
    return `
      <div class="diagnostic-table-wrap">
        <table class="diagnostic-table">
          <thead><tr><th>Gate</th><th>Dimension</th><th>Result</th><th>Reason</th></tr></thead>
          <tbody>
            ${gates
              .map((gate) => {
                const meta = statusMeta(gate.passed ? "passed" : "failed");
                return `<tr>
                  <td class="mono">${escapeHtml(gate.id)}</td>
                  <td>${escapeHtml(titleCase(gate.dimension || "structural_integrity"))}</td>
                  <td><span class="table-status status-${meta.css}">${meta.label}</span></td>
                  <td class="wrap-cell">${escapeHtml(gate.reason || "—")}</td>
                </tr>`;
              })
              .join("")}
          </tbody>
        </table>
      </div>
    `;
  }

  function renderDimensions(dimensions) {
    if (!dimensions?.length) return '<div class="clean-state">No dimension report recorded.</div>';
    return `
      <div class="diagnostic-table-wrap">
        <table class="diagnostic-table">
          <thead><tr><th>Dimension</th><th>Result</th><th>Signals</th></tr></thead>
          <tbody>
            ${dimensions.map((dimension) => {
              const result = dimension.passed === null || dimension.passed === undefined
                ? "Measured"
                : dimension.passed ? "Passed" : "Failed";
              return `<tr>
                <td>${escapeHtml(titleCase(dimension.dimension || "unknown"))}</td>
                <td>${escapeHtml(result)}</td>
                <td class="mono wrap-cell">${escapeHtml(formatPayload(dimension.signals))}</td>
              </tr>`;
            }).join("")}
          </tbody>
        </table>
      </div>
    `;
  }

  function renderDeliverables(deliverables) {
    if (!deliverables?.length) return '<div class="clean-state">No deliverable contract for this run.</div>';
    return `
      <div class="diagnostic-table-wrap">
        <table class="diagnostic-table">
          <thead><tr><th>Deliverable</th><th>Validation</th><th>Artifact</th><th>Invariants</th></tr></thead>
          <tbody>
            ${deliverables.map((deliverable) => {
              const invariants = (deliverable.invariants || [])
                .map((invariant) => `${invariant.id}: ${invariant.passed ? "pass" : "FAIL"}`)
                .join(" · ");
              const valid = deliverable.schema_valid && deliverable.provenance_valid &&
                (deliverable.invariants || []).every((invariant) => invariant.passed);
              return `<tr>
                <td><span class="mono">${escapeHtml(deliverable.id)}</span><br><small>${escapeHtml(deliverable.kind || "")}</small></td>
                <td>${valid ? "Passed" : "Failed"}<br><small class="mono">${escapeHtml(deliverable.content_sha256 || "")}</small></td>
                <td class="mono wrap-cell">${escapeHtml(deliverable.artifact?.path || "unavailable")}</td>
                <td class="wrap-cell">${escapeHtml(invariants || "No invariants")}</td>
              </tr>`;
            }).join("")}
          </tbody>
        </table>
      </div>
    `;
  }

  function renderCriteriaTable(criteria) {
    if (!criteria?.length) return '<div class="clean-state">No scored criteria recorded.</div>';
    return `
      <div class="diagnostic-table-wrap">
        <table class="diagnostic-table">
          <thead><tr><th>Criterion</th><th>Points</th><th>Reason</th></tr></thead>
          <tbody>
            ${criteria
              .map(
                (criterion) => `<tr>
                  <td class="mono">${escapeHtml(criterion.id)}</td>
                  <td>${escapeHtml(
                    `${criterion.awarded ?? "—"} / ${criterion.possible ?? "—"}`,
                  )}</td>
                  <td class="wrap-cell">${escapeHtml(criterion.reason || "—")}</td>
                </tr>`,
              )
              .join("")}
          </tbody>
        </table>
      </div>
    `;
  }

  function usageBlocks(run) {
    const totals = run.metrics?.totals || {};
    const judge = run.judge_usage || {};
    const cost = run.cost || {};
    return `
      <div class="usage-grid ${ui.metricGrid}">
        ${metricBlock("Sessions", compactNumber(totals.sessions, 0))}
        ${metricBlock("Turns", compactNumber(totals.turns, 0))}
        ${metricBlock("Function calls", compactNumber(totals.function_calls, 0))}
        ${metricBlock("Function errors", compactNumber(totals.function_call_errors, 0))}
        ${metricBlock("Input tokens", compactNumber(totals.input_tokens, 0))}
        ${metricBlock("Output tokens", compactNumber(totals.output_tokens, 0))}
        ${metricBlock("Cache read", compactNumber(totals.cache_read_tokens, 0))}
        ${metricBlock("Reasoning", compactNumber(totals.reasoning_tokens, 0))}
        ${metricBlock("Subject cost", formatCurrency(cost.subject_usd))}
        ${metricBlock("Judge cost", formatCurrency(cost.judge_usd), `${compactNumber(run.judge_attempts, 0)} attempt(s)`)}
        ${metricBlock("Total cost", formatCurrency(cost.total_usd))}
        ${metricBlock("Judge tokens", compactNumber(
          (judge.input_tokens || 0) + (judge.output_tokens || 0),
          0,
        ))}
      </div>
    `;
  }

  function renderRetries(retries) {
    if (!retries?.length) return '<div class="clean-state">No retry attempts.</div>';
    return retries
      .map((retry, index) => {
        const meta = statusMeta(retry.status);
        return `
          <article class="retry-card">
            <header>
              <strong>Retry ${index + 1}</strong>
              <span class="table-status status-${meta.css}">${meta.label}</span>
            </header>
            <div class="retry-meta">
              <span class="mono">${escapeHtml(retry.run_id || "")}</span>
              <span>${formatDuration((retry.wall_time_ms || 0) / 1000)}</span>
              <span>${formatCurrency(retry.cost?.total_usd)}</span>
            </div>
            ${renderFailureList(retry.failures)}
          </article>
        `;
      })
      .join("");
  }

  function formatEventTime(timestamp) {
    if (!timestamp || Number.isNaN(Date.parse(timestamp))) return "";
    return new Intl.DateTimeFormat("en-US", {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    }).format(new Date(timestamp));
  }

  function formatPayload(value) {
    if (typeof value === "string") return value;
    if (value === undefined || value === null) return "No payload recorded.";
    try {
      return JSON.stringify(value, null, 2);
    } catch (_error) {
      return String(value);
    }
  }

  function renderConversationText(value) {
    return escapeHtml(value)
      .replace(/`([^`\n]+)`/g, "<code>$1</code>")
      .replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>")
      .replaceAll("\n", "<br>");
  }

  function conversationMessage(event) {
    const isAssistant = event.role === "assistant";
    const label = isAssistant ? "assistant" : "you";
    const context = [event.provider, event.model].filter(Boolean).join("/");
    const time = formatEventTime(event.timestamp);
    const metadata = [context, time].filter(Boolean).join(" · ");
    return `
      <article
        id="${escapeHtml(event.id)}"
        class="conversation-event conversation-message conversation-${escapeHtml(event.role)}"
        data-kind="message"
      >
        <div class="conversation-card">
          <header>
            <strong><span aria-hidden="true">${isAssistant ? ">" : "$"}</span> ${label}</strong>
            <span>${escapeHtml(metadata)}</span>
          </header>
          <div class="conversation-copy">${renderConversationText(event.text)}</div>
        </div>
      </article>
    `;
  }

  function conversationTool(event) {
    const status = event.isError
      ? "Error"
      : event.status === "pending"
        ? "No result"
        : "Completed";
    const resultPayload =
      event.result?.details !== null && event.result?.details !== undefined
        ? event.result.details
        : event.result?.text;
    const time = formatEventTime(event.timestamp);
    return `
      <details
        id="${escapeHtml(event.id)}"
        class="conversation-event conversation-tool${event.isError ? " conversation-tool-error" : ""}"
        data-kind="tool"
        data-error="${event.isError ? "true" : "false"}"
        ${event.isError ? "open" : ""}
      >
        <summary>
          <span class="conversation-tool-icon" aria-hidden="true">${event.isError ? "×" : "✓"}</span>
          <span class="conversation-tool-name">
            <small>${event.status === "pending" ? "triggering" : "triggered"} <i>ƒ</i></small>
            <strong class="mono">${escapeHtml(event.functionId)}${time ? ` <em>· ${escapeHtml(time)}</em>` : ""}</strong>
          </span>
          <span class="conversation-tool-status">${status}</span>
          <span class="conversation-chevron" aria-hidden="true">⌄</span>
        </summary>
        <div class="conversation-tool-body">
          ${
            event.arguments !== null && event.arguments !== undefined
              ? `<div class="conversation-payload">
                  <span>Request</span>
                  <pre>${escapeHtml(formatPayload(event.arguments))}</pre>
                </div>`
              : ""
          }
          <div class="conversation-payload${event.isError ? " conversation-payload-error" : ""}">
            <span>${event.isError ? "Error result" : "Result"}</span>
            <pre>${escapeHtml(formatPayload(resultPayload))}</pre>
          </div>
        </div>
      </details>
    `;
  }

  function renderConversation(messages) {
    const transcript = window.HarnessExecutionTranscript;
    const events = transcript.normalizeTranscript(messages);
    if (!events.length) {
      return '<div class="clean-state">No readable conversation events were recorded.</div>';
    }
    const summary = transcript.summarizeTranscript(events);
    return `
      <div class="conversation-shell" data-filter="all" data-error-cursor="-1">
        <div class="conversation-toolbar">
          <div class="conversation-stats" aria-label="Conversation summary">
            <span><strong>${summary.messages}</strong> messages</span>
            <span><strong>${summary.calls}</strong> calls</span>
            <span class="${summary.errors ? "has-errors" : ""}"><strong>${summary.errors}</strong> errors</span>
          </div>
          <div class="conversation-actions">
            <div class="conversation-filters" aria-label="Filter conversation">
              <button type="button" class="conversation-filter is-active" data-filter="all" aria-pressed="true">All</button>
              <button type="button" class="conversation-filter" data-filter="messages" aria-pressed="false">Messages</button>
              <button type="button" class="conversation-filter" data-filter="errors" aria-pressed="false">Errors only</button>
            </div>
            <button
              type="button"
              class="conversation-next-error"
              ${summary.errors ? "" : "disabled"}
            >
              Next error
            </button>
          </div>
        </div>
        <div class="conversation-error-position" aria-live="polite"></div>
        <div class="conversation-list">
          ${events
            .map((event) =>
              event.kind === "message"
                ? conversationMessage(event)
                : conversationTool(event),
            )
            .join("")}
          <div class="conversation-empty-filter">No events match this filter.</div>
        </div>
      </div>
    `;
  }

  function renderConversationLaunch(messages, run) {
    const transcript = window.HarnessExecutionTranscript;
    const events = transcript.normalizeTranscript(messages);
    const summary = transcript.summarizeTranscript(events);
    return `
      <section class="conversation-launch">
        <div>
          <span>Session transcript</span>
          <strong>${summary.messages} messages · ${summary.calls} functions</strong>
          <small class="${summary.errors ? "has-errors" : ""}">
            ${summary.errors
              ? `${summary.errors} error${summary.errors === 1 ? "" : "s"} to inspect`
              : `${escapeHtml(run.session_id || "No session id")} · read-only`}
          </small>
        </div>
        <button type="button" class="conversation-open">Open chat</button>
      </section>
    `;
  }

  function openConversationDialog(run, context) {
    const messages = Array.isArray(run.transcript?.messages)
      ? run.transcript.messages
      : [];
    elements.transcriptTitle.textContent =
      `${titleCase(context.scenarioId)} · run ${context.runIndex + 1}`;
    elements.transcriptContext.textContent = [
      context.subjectLabel,
      run.session_id ? `session ${run.session_id}` : "session unavailable",
      run.run_id ? `run ${run.run_id}` : "",
    ]
      .filter(Boolean)
      .join(" · ");
    elements.transcriptBody.innerHTML = renderConversation(messages);
    attachConversationControls(elements.transcriptBody);
    if (!elements.transcriptDialog.open) elements.transcriptDialog.showModal();
  }

  function attachTranscriptDialogControls() {
    elements.transcriptClose.addEventListener("click", () => {
      elements.transcriptDialog.close();
    });
    elements.transcriptDialog.addEventListener("click", (event) => {
      if (event.target === elements.transcriptDialog) {
        elements.transcriptDialog.close();
      }
    });
  }

  function attachConversationControls(container) {
    container.querySelectorAll(".conversation-shell").forEach((shell) => {
      const filters = [...shell.querySelectorAll(".conversation-filter")];
      const position = shell.querySelector(".conversation-error-position");
      filters.forEach((button) => {
        button.addEventListener("click", () => {
          const filter = button.dataset.filter || "all";
          shell.dataset.filter = filter;
          filters.forEach((candidate) => {
            const active = candidate === button;
            candidate.classList.toggle("is-active", active);
            candidate.setAttribute("aria-pressed", String(active));
          });
          const visibleEvents = [
            ...shell.querySelectorAll(".conversation-event"),
          ].filter((event) => {
            if (filter === "messages") return event.dataset.kind === "message";
            if (filter === "errors") return event.dataset.error === "true";
            return true;
          });
          shell.classList.toggle(
            "conversation-filter-empty",
            visibleEvents.length === 0,
          );
          position.textContent = "";
        });
      });

      const nextError = shell.querySelector(".conversation-next-error");
      nextError?.addEventListener("click", () => {
        const errors = [
          ...shell.querySelectorAll('.conversation-event[data-error="true"]'),
        ];
        if (!errors.length) return;
        const nextIndex = (Number(shell.dataset.errorCursor || -1) + 1) % errors.length;
        shell.dataset.errorCursor = String(nextIndex);
        const error = errors[nextIndex];
        error.open = true;
        error.scrollIntoView({ behavior: "smooth", block: "center" });
        error.classList.add("conversation-focus-error");
        position.textContent = `Error ${nextIndex + 1} of ${errors.length}`;
        window.setTimeout(
          () => error.classList.remove("conversation-focus-error"),
          1600,
        );
      });
    });
  }

  function sessionTable(metrics) {
    const sessions = Array.isArray(metrics?.by_session) ? metrics.by_session : [];
    const traces = metrics?.traces || {};
    if (!sessions.length && !Object.keys(traces).length) {
      return '<div class="clean-state">No session metrics or traces recorded.</div>';
    }
    return `
      ${
        sessions.length
          ? `<div class="diagnostic-table-wrap">
              <table class="diagnostic-table">
                <thead><tr><th>Session</th><th>Depth</th><th>Turns</th><th>Calls</th><th>Errors</th><th>Tokens</th><th>Cost</th></tr></thead>
                <tbody>
                  ${sessions
                    .map(
                      (session) => `<tr>
                        <td class="mono wrap-cell">${escapeHtml(session.session_id)}</td>
                        <td>${compactNumber(session.depth, 0)}</td>
                        <td>${compactNumber(session.turns, 0)}</td>
                        <td>${compactNumber(session.function_calls, 0)}</td>
                        <td>${compactNumber(session.function_call_errors, 0)}</td>
                        <td>${compactNumber(
                          Number(session.input_tokens || 0) +
                            Number(session.output_tokens || 0),
                          0,
                        )}</td>
                        <td>${formatCurrency(session.cost_usd)}</td>
                      </tr>`,
                    )
                    .join("")}
                </tbody>
              </table>
            </div>`
          : ""
      }
      <details class="nested-raw" data-trace="true">
        <summary>Trace summary</summary>
        <pre></pre>
      </details>
    `;
  }

  function runSection(title, content, options = {}) {
    return `
      <section class="run-section ${options.wide ? "run-section-wide" : ""}">
        <h5>${escapeHtml(title)}</h5>
        ${content}
      </section>
    `;
  }

  const PROMPT_CLAMP_THRESHOLD = 1200;

  function renderEvaluationTab(panel, run) {
    panel.innerHTML = `
      ${runSection("Failures", renderFailureList(run.failures))}
      ${runSection("Evaluation dimensions", renderDimensions(run.dimensions), { wide: true })}
      ${runSection("Deliverables", renderDeliverables(run.deliverables), { wide: true })}
      ${runSection("Hard gates", renderGateTable(run.hard_gates), { wide: true })}
      ${runSection("Scored criteria", renderCriteriaTable(run.criteria), { wide: true })}
      ${runSection("Retry attempts", renderRetries(run.retry_attempts), { wide: true })}
    `;
  }

  function renderUsageTab(panel, run) {
    panel.innerHTML = runSection("Usage and cost", usageBlocks(run), { wide: true });
  }

  function renderPromptTab(panel, run) {
    const prompt = run.prompt || "No prompt recorded.";
    const clamp = prompt.length > PROMPT_CLAMP_THRESHOLD;
    panel.innerHTML = runSection(
      "Prompt",
      `
        <pre class="prompt-block${clamp ? " is-clamped" : ""}">${escapeHtml(prompt)}</pre>
        ${clamp ? '<button type="button" class="prompt-expand">Show full prompt</button>' : ""}
      `,
      { wide: true },
    );
    panel.querySelector(".prompt-expand")?.addEventListener("click", (event) => {
      panel.querySelector(".prompt-block").classList.remove("is-clamped");
      event.target.remove();
    });
  }

  function renderSessionsTab(panel, run) {
    panel.innerHTML = runSection("Sessions and traces", sessionTable(run.metrics), {
      wide: true,
    });
    const trace = panel.querySelector('details[data-trace="true"]');
    trace?.addEventListener("toggle", () => {
      if (!trace.open || trace.dataset.rendered) return;
      trace.dataset.rendered = "true";
      trace.querySelector("pre").textContent = JSON.stringify(
        run.metrics?.traces || {},
        null,
        2,
      );
    });
  }

  function renderRawTab(panel, run) {
    panel.innerHTML = runSection(
      "Complete run record",
      '<pre class="prompt-block"></pre>',
      { wide: true },
    );
    panel.querySelector("pre").textContent = JSON.stringify(run, null, 2);
  }

  const RUN_TABS = [
    { id: "evaluation", label: "Evaluation", render: renderEvaluationTab },
    { id: "usage", label: "Usage", render: renderUsageTab },
    { id: "prompt", label: "Prompt", render: renderPromptTab },
    { id: "sessions", label: "Sessions", render: renderSessionsTab },
    { id: "raw", label: "Raw", render: renderRawTab },
  ];

  function renderRunContent(container, run, context) {
    const messages = Array.isArray(run.transcript?.messages)
      ? run.transcript.messages
      : [];
    container.innerHTML = `
      <div class="run-overview ${ui.metricGrid}">
        ${metricBlock("Score", compactNumber(run.score, 1))}
        ${metricBlock("Runtime", formatDuration((run.wall_time_ms || 0) / 1000))}
        ${metricBlock("Total cost", formatCurrency(run.cost?.total_usd))}
        ${metricBlock("Judge attempts", compactNumber(run.judge_attempts, 0))}
        ${metricBlock("Retries", compactNumber(run.retry_attempts?.length || 0, 0))}
        ${metricBlock("Session", run.session_id || "Unknown")}
      </div>
      ${renderConversationLaunch(messages, run)}
      <div class="run-tabs" role="tablist" aria-label="Run diagnostics">
        ${RUN_TABS.map(
          (tab) =>
            `<button type="button" role="tab" data-tab="${tab.id}" aria-selected="false">${tab.label}</button>`,
        ).join("")}
      </div>
      ${RUN_TABS.map(
        (tab) =>
          `<div class="run-sections" role="tabpanel" data-panel="${tab.id}" hidden></div>`,
      ).join("")}
    `;
    container.querySelector(".conversation-open")?.addEventListener("click", () => {
      openConversationDialog(run, context);
    });
    const buttons = [...container.querySelectorAll(".run-tabs button")];
    const activateRunTab = (tabId) => {
      RUN_TABS.forEach((tab) => {
        const panel = container.querySelector(`[data-panel="${tab.id}"]`);
        const active = tab.id === tabId;
        if (active && !panel.dataset.rendered) {
          panel.dataset.rendered = "true";
          tab.render(panel, run);
        }
        panel.hidden = !active;
      });
      buttons.forEach((button) => {
        const active = button.dataset.tab === tabId;
        button.classList.toggle("active", active);
        button.setAttribute("aria-selected", String(active));
      });
    };
    buttons.forEach((button) => {
      button.addEventListener("click", () => activateRunTab(button.dataset.tab));
    });
    activateRunTab("evaluation");
  }

  function renderFullScenario(record, options = {}) {
    const report = record.report;
    const scenario = report.scenarios?.find(
      (item) => item.scenario_id === record.scenario_id,
    );
    if (!scenario) return "";
    const subjectId = record.subject_id;
    const aggregate = scenario.aggregate || {};
    const meta = statusMeta(
      window.HarnessExecutionData.normalizeScenarioStatus({
        ...scenario,
        hard_gate_failures: aggregate.hard_gate_failures,
        technical_failures: aggregate.technical_failures,
      }),
    );
    const policy = scenario.execution_policy || {};
    const scenarioCase = scenario.case || {};
    const anchor = scenarioAnchor(subjectId, scenario.scenario_id);
    const failingRun = (scenario.runs || []).some(
      (run) =>
        (run.failures || []).length ||
        (run.hard_gates || []).some((gate) => gate && gate.passed === false),
    );
    const open = !scenario.passed || failingRun || options.soleScenario;
    return `
      <details id="${escapeHtml(anchor)}" class="${ui.scenarioCard}" ${open ? "open" : ""}>
        ${scenarioSummaryRow({
          title: titleCase(scenario.scenario_id),
          subjectLabel: `${report.subject.provider}/${report.subject.model}`,
          median: compactNumber(aggregate.median_score, 1),
          passRate: formatPercent(
            typeof aggregate.pass_rate === "number" ? aggregate.pass_rate * 100 : null,
          ),
          cost: formatCurrency(aggregate.cost?.total_usd),
          statusCss: meta.css,
          statusLabel: meta.label,
        })}
        <div class="${ui.scenarioBody}">
        <div class="scenario-detail-metrics ${ui.metricGrid}">
          ${metricBlock("Median score", compactNumber(aggregate.median_score, 1))}
          ${metricBlock("Pass rate", formatPercent(
            typeof aggregate.pass_rate === "number" ? aggregate.pass_rate * 100 : null,
          ))}
          ${metricBlock("Runs passed", `${aggregate.passed_runs ?? "—"} / ${aggregate.runs ?? "—"}`, `${aggregate.required_passes ?? "—"} required`)}
          ${metricBlock("Cost", formatCurrency(aggregate.cost?.total_usd))}
          ${metricBlock("Hard gates", compactNumber(aggregate.hard_gate_failures, 0))}
          ${metricBlock("Technical", compactNumber(aggregate.technical_failures, 0))}
          ${metricBlock("Complexity", titleCase(scenarioCase.complexity?.tier || "unclassified"))}
          ${metricBlock("Case seed", scenarioCase.seed ?? "—")}
          ${metricBlock("Max turns", compactNumber(policy.max_turns, 0))}
          ${metricBlock("Token budget", compactNumber(policy.max_total_tokens, 0))}
        </div>
        <div class="run-list">
          ${(scenario.runs || [])
            .map((run, index) => {
              const runMeta = statusMeta(run.status);
              const runId = runAnchor(subjectId, scenario.scenario_id, index);
              const functionErrors = Number(
                run.metrics?.totals?.function_call_errors || 0,
              );
              return `
                <details id="${escapeHtml(runId)}" class="run-detail" data-run-index="${index}">
                  <summary>
                    <span class="run-number">${String(index + 1).padStart(2, "0")}</span>
                    <span class="run-summary-copy">
                      <strong>Run ${index + 1}</strong>
                      <small>
                        <span class="mono">${escapeHtml(run.run_id || "unknown")}</span>
                        ${
                          functionErrors
                            ? `<span class="run-summary-error">${functionErrors} function error${functionErrors === 1 ? "" : "s"}</span>`
                            : ""
                        }
                      </small>
                    </span>
                    <span>Score ${compactNumber(run.score, 1)}</span>
                    <span>${formatDuration((run.wall_time_ms || 0) / 1000)}</span>
                    <span class="table-status status-${runMeta.css}">${runMeta.label}</span>
                  </summary>
                  <div class="run-content" data-pending="true">
                    <div class="detail-loading-inline">Open run to render diagnostics.</div>
                  </div>
                </details>
              `;
            })
            .join("")}
        </div>
        </div>
      </details>
    `;
  }

  function attachRunRenderers() {
    (detail?.reports || []).forEach((record) => {
      const scenario = record?.report?.scenarios?.find(
        (item) => item.scenario_id === record.scenario_id,
      );
      (scenario?.runs || []).forEach((run, index) => {
        const id = runAnchor(record.subject_id, record.scenario_id, index);
        const element = document.getElementById(id);
        if (!element) return;
        element.addEventListener("toggle", () => {
          if (!element.open) return;
          const container = element.querySelector(".run-content[data-pending='true']");
          if (!container) return;
          container.removeAttribute("data-pending");
          renderRunContent(container, run, {
            runIndex: index,
            scenarioId: record.scenario_id,
            subjectLabel: [
              record.report?.subject?.provider,
              record.report?.subject?.model,
            ]
              .filter(Boolean)
              .join("/"),
          });
        });
      });
    });
  }

  function renderScenarios() {
    const availableReports = (detail?.reports || []).filter(
      (record) => record?.available && record.report,
    );
    const active = ["running", "cancelling"].includes(execution.status);
    if (availableReports.length) {
      const runCount = availableReports.reduce(
        (total, record) =>
          total +
          (record.report.scenarios || []).reduce(
            (scenarioTotal, scenario) =>
              scenarioTotal + (scenario.runs?.length || 0),
            0,
          ),
        0,
      );
      elements.scenarioIntro.textContent =
        `${availableReports.length} scenario reports and ${runCount} individual runs. ` +
        "Select a scenario to expand its runs.";
      elements.scenarioDetails.innerHTML = availableReports
        .map((record) =>
          renderFullScenario(record, { soleScenario: availableReports.length === 1 }),
        )
        .join("");
      attachRunRenderers();
      return;
    }
    elements.scenarioIntro.textContent = active
      ? "Execution is still running. Scenario evidence will appear as reports complete."
      : execution.availability === "aggregate"
        ? "The complete report expired after 30 executions. Historical aggregate metrics remain available."
        : "This workflow did not produce a complete benchmark report.";
    const aggregateCount = execution.subjects.reduce(
      (total, subject) => total + (subject.scenarios || []).length,
      0,
    );
    elements.scenarioDetails.innerHTML = execution.subjects
      .flatMap((subject) =>
        (subject.scenarios || []).map((scenario) =>
          renderAggregateScenario(subject, scenario, {
            soleScenario: aggregateCount === 1,
          }),
        ),
      )
      .join("");
    if (!elements.scenarioDetails.children.length) {
      elements.scenarioDetails.innerHTML = active
        ? '<div class="clean-state">No scenario report has been published yet. This view will update when evidence arrives.</div>'
        : '<div class="clean-state">No scenario data was produced. Open the workflow for build and infrastructure diagnostics.</div>';
    }
  }

  const MAX_FAILURE_CHIPS = 5;

  function truncateText(value, limit) {
    const text = String(value || "");
    return text.length > limit ? `${text.slice(0, limit - 1)}…` : text;
  }

  function failureChip(group) {
    const anchor = runAnchor(group.subjectId, group.scenarioId, group.runIndex);
    const first = group.items[0];
    const preview =
      first.kind === "gate"
        ? `${first.gateId}: ${first.message}`
        : `${titleCase(first.phase)}: ${first.message}`;
    return `
      <a class="failure-chip" href="${escapeHtml(routes.execution(executionId, anchor))}" data-anchor="${escapeHtml(anchor)}">
        <span class="table-status status-fail">${group.items.length} issue${group.items.length === 1 ? "" : "s"}</span>
        <strong>${escapeHtml(titleCase(group.scenarioId))} · run ${group.runIndex + 1}</strong>
        <span class="failure-chip-message">${escapeHtml(truncateText(preview, 140))}</span>
      </a>
    `;
  }

  function renderFailures() {
    const groups = window.HarnessExecutionData.groupRunFailures(detail?.reports);
    const count = blockingFailures();
    const active = ["running", "cancelling"].includes(execution.status);
    if (
      !groups.length &&
      count === 0 &&
      (execution.status === "passed" || active)
    ) {
      elements.failureBox.hidden = true;
      return;
    }
    elements.failureBox.hidden = false;
    elements.failureTitle.textContent = count
      ? `Execution failures · ${count} blocking event${count === 1 ? "" : "s"}`
      : "Execution failures";
    if (groups.length) {
      const sorted = [...groups].sort((a, b) => b.items.length - a.items.length);
      const visible = sorted.slice(0, MAX_FAILURE_CHIPS);
      const overflow = sorted.slice(MAX_FAILURE_CHIPS);
      elements.failureSummary.innerHTML = `
        <div class="failure-groups">${visible.map(failureChip).join("")}</div>
        ${
          overflow.length
            ? `<details class="failure-overflow">
                <summary>Show all ${sorted.length} failing runs</summary>
                <div class="failure-groups">${overflow.map(failureChip).join("")}</div>
              </details>`
            : ""
        }
      `;
      return;
    }
    elements.failureSummary.innerHTML = `
      <p>
        ${escapeHtml(
          execution.status === "cancelled"
            ? "The workflow was cancelled before a complete result was published."
            : `${count} blocking reliability event${count === 1 ? "" : "s"} recorded in the aggregate result.`,
        )}
      </p>
    `;
  }

  function renderRawData() {
    // The detail JSON can be multi-megabyte: serialize only when the preview
    // is opened or the fallback download is clicked, never at page load.
    elements.rawPreview.addEventListener("toggle", () => {
      if (!elements.rawPreview.open || elements.rawPreview.dataset.rendered) return;
      elements.rawPreview.dataset.rendered = "true";
      elements.rawJson.textContent = JSON.stringify(detail || execution, null, 2);
    });
    elements.rawActions.replaceChildren();
    if (detail && execution.detail_path && !window.HARNESS_BENCHMARK_PREVIEW) {
      const link = document.createElement("a");
      link.className = "button";
      link.href = execution.detail_path;
      link.download = `${execution.id}.json`;
      link.textContent = "Download execution JSON";
      elements.rawActions.append(link);
      return;
    }
    const link = document.createElement("a");
    link.className = "button";
    link.href = routes.execution(executionId, "raw-data");
    link.download = `${execution.id}.json`;
    link.textContent = "Download available JSON";
    link.addEventListener("click", () => {
      if (link.dataset.blobUrl) return;
      const blob = new Blob([JSON.stringify(detail || execution, null, 2)], {
        type: "application/json",
      });
      link.dataset.blobUrl = URL.createObjectURL(blob);
      link.href = link.dataset.blobUrl;
    });
    elements.rawActions.append(link);
  }

  async function loadDetail() {
    if (execution.availability !== "full" || !execution.detail_path) return null;
    const preview = window.HARNESS_EXECUTION_DETAILS?.[execution.id];
    if (preview) return preview;
    if (
      typeof execution.detail_path !== "string" ||
      execution.detail_path.includes("..") ||
      !execution.detail_path.startsWith("runs/")
    ) {
      throw new Error("The retained detail path is invalid.");
    }
    const url = new URL(execution.detail_path, window.location.href);
    const runsRoot = new URL("./runs/", window.location.href);
    if (url.origin !== runsRoot.origin || !url.pathname.startsWith(runsRoot.pathname)) {
      throw new Error("The retained detail path is outside the benchmark site.");
    }
    const response = await fetch(url, { cache: "no-store" });
    if (!response.ok) throw new Error(`Execution report returned HTTP ${response.status}.`);
    const value = await response.json();
    if (!value || typeof value !== "object") {
      throw new Error("Execution report is not an object.");
    }
    return value;
  }

  function revealAnchor(anchorId) {
    if (!anchorId) return;
    const target = document.getElementById(anchorId);
    if (!target) return;
    document
      .querySelectorAll(".scenario-detail-card.is-focused")
      .forEach((scenario) => scenario.classList.remove("is-focused"));
    const focusedScenario = target.closest(".scenario-detail-card");
    focusedScenario?.classList.add("is-focused");
    // Open the target and every collapsed ancestor so deep links land on
    // rendered content; opening fires toggle, which drives the lazy renderers.
    let node = target.closest("details");
    while (node) {
      node.open = true;
      node = node.parentElement?.closest("details");
    }
    const disclosure = target.querySelector?.("details.section-disclosure");
    if (disclosure) disclosure.open = true;
    requestAnimationFrame(() => target.scrollIntoView());
  }

  function attachAnchorNavigation() {
    window.addEventListener("hashchange", () => {
      const route = routes.current();
      if (route.page === "execution" && route.executionId === executionId) {
        revealAnchor(route.anchor);
      }
    });
    // Same-hash re-clicks do not fire hashchange; handle triage links directly.
    elements.failureSummary.addEventListener("click", (event) => {
      const link = event.target.closest('a[href^="#"]');
      if (link) revealAnchor(link.dataset.anchor);
    });
  }

  function reveal() {
    elements.loading.hidden = true;
    elements.content.hidden = false;
    requestAnimationFrame(() => {
      const route = routes.current();
      revealAnchor(route.page === "execution" ? route.anchor : null);
    });
  }

  async function initialize() {
    attachTranscriptDialogControls();
    attachAnchorNavigation();
    if (!execution) {
      elements.loading.hidden = true;
      elements.error.hidden = false;
      elements.errorMessage.textContent = executionId
        ? `Execution ${executionId} is outside the retained 100-run history.`
        : "The execution URL does not include a run id.";
      return;
    }
    renderHeader();
    renderKpis();
    renderMetadata();
    try {
      detail = await loadDetail();
    } catch (error) {
      execution.availability = execution.subjects.length ? "aggregate" : "unavailable";
      elements.availability.className = `data-badge data-${execution.availability}`;
      elements.availability.textContent = "Aggregate fallback";
      elements.scenarioIntro.textContent = error.message;
    }
    renderConfiguration();
    renderScenarios();
    renderFailures();
    renderRawData();
    reveal();
  }

  initialize();
})();
