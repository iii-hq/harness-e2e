import { useEffect, useState } from 'react'
import { LegacyLoadError } from '@/components/LegacyLoadError'
import { SectionNav, type WorkspaceView } from '@/components/SectionNav'
import { ThemeToggle } from '@/components/ThemeToggle'
import { useLegacyPage } from '@/hooks/useLegacyPage'

function initialWorkspaceView(): WorkspaceView {
  const view = new URLSearchParams(window.location.search).get('view')
  if (view === 'scenarios' || view === 'capability' || view === 'executions') {
    return view
  }
  if (window.location.hash === '#scenarios') return 'scenarios'
  if (window.location.hash === '#capability') return 'capability'
  if (window.location.hash === '#executions') return 'executions'
  return 'overview'
}

export function OverviewPage() {
  const error = useLegacyPage('overview')
  const [activeView, setActiveView] =
    useState<WorkspaceView>(initialWorkspaceView)

  useEffect(() => {
    const onPopState = () => setActiveView(initialWorkspaceView())
    window.addEventListener('popstate', onPopState)
    return () => window.removeEventListener('popstate', onPopState)
  }, [])

  const selectView = (view: WorkspaceView) => {
    setActiveView(view)
    const url = new URL(window.location.href)
    if (view === 'overview') url.searchParams.delete('view')
    else url.searchParams.set('view', view)
    url.hash = ''
    window.history.pushState(null, '', url)
    document.querySelector('.section-nav')?.scrollIntoView({
      block: 'start',
      behavior: 'smooth',
    })
  }

  return (
    <>
      <LegacyLoadError error={error} />
      <a className="skip-link" href="#main">
        Skip to execution dashboard
      </a>
      <div className="ambient ambient-one" aria-hidden="true"></div>
      <div className="ambient ambient-two" aria-hidden="true"></div>

      <header className="topbar">
        <a
          className="brand"
          href="https://github.com/iii-hq/harness-e2e"
          aria-label="iii Harness E2E"
        >
          <span className="brand-copy">
            <strong>iii</strong>
            <span>Harness benchmarks</span>
          </span>
        </a>
        <nav className="topbar-actions" aria-label="Dashboard actions">
          <span id="preview-badge" className="badge badge-preview" hidden>
            Preview data
          </span>
          <button
            id="open-local-runner"
            className="button button-primary"
            type="button"
            hidden
          >
            <span aria-hidden="true">＋</span> New execution
          </button>
          <a
            className="button button-secondary"
            href="./coverage/"
            data-mobile-label="Coverage"
          >
            Coverage
          </a>
          <ThemeToggle />
          <a
            id="actions-link"
            className="button button-secondary"
            href="https://github.com/iii-hq/harness-e2e/actions"
            data-mobile-label="Repo"
          >
            View Actions <span aria-hidden="true">↗</span>
          </a>
        </nav>
      </header>

      <main id="main" className="page-shell overview-shell">
        <section className="page-heading" aria-labelledby="page-title">
          <div>
            <div className="eyebrow">
              <span className="live-dot" aria-hidden="true"></span>
              Harness E2E
            </div>
            <h1 id="page-title">Harness evidence</h1>
            <p>
              Know what passed, what changed, and why before trusting a
              benchmark.
            </p>
          </div>
          <div className="sync-block">
            <span id="sync-label">Last published</span>
            <time id="last-update" dateTime="">
              Waiting for data
            </time>
          </div>
        </section>

        <SectionNav activeView={activeView} onViewChange={selectView} />

        <dialog id="local-runner-dialog" className="local-runner-dialog">
          <div className="local-runner-dialog-header">
            <div>
              <div className="section-kicker">New evidence</div>
              <strong>Configure execution</strong>
            </div>
            <button
              id="close-local-runner"
              className="dialog-close"
              type="button"
              aria-label="Close execution form"
            >
              ×
            </button>
          </div>
          <section
            id="local-runner"
            className="panel local-runner"
            aria-labelledby="local-runner-title"
            hidden
          >
            <div className="panel-heading local-runner-heading">
              <div>
                <div className="section-kicker">Local experiment</div>
                <h2 id="local-runner-title">Run E2E scenarios</h2>
                <p className="trend-description">
                  Uses the Harness already running at the selected iii URL. Each
                  run is saved as an independent execution that can be compared
                  with any other.
                </p>
              </div>
              <span id="local-run-status" className="local-run-status">
                Ready
              </span>
            </div>
            <div className="local-connection" aria-live="polite">
              <div>
                <span
                  id="local-catalog-indicator"
                  className="local-connection-dot"
                  aria-hidden="true"
                ></span>
                <span id="local-catalog-status">
                  Discovering the running Harness…
                </span>
                <code id="local-connection-url"></code>
              </div>
              <button
                id="local-catalog-refresh"
                className="button"
                type="button"
              >
                Refresh catalog
              </button>
            </div>
            <form id="local-run-form" className="local-run-form">
              <label className="local-field">
                <span>
                  Execution label <small>optional</small>
                </span>
                <input
                  name="label"
                  maxLength={120}
                  placeholder="Before system prompt change"
                />
              </label>
              <div className="local-field">
                <span>
                  Subject model <small>required</small>
                </span>
                <select
                  id="local-subject"
                  name="subject"
                  disabled
                  hidden
                  aria-hidden="true"
                  tabIndex={-1}
                >
                  <option value="">Loading registered models…</option>
                </select>
                <details
                  id="local-subject-picker"
                  className="local-model-picker local-picker-disabled"
                  aria-disabled="true"
                >
                  <summary>
                    <strong id="local-subject-summary">
                      Loading registered models…
                    </strong>
                    <span>Choose</span>
                  </summary>
                  <div className="local-model-popover">
                    <div className="local-model-search">
                      <input
                        id="local-subject-search"
                        type="search"
                        placeholder="Search provider or model"
                        autoComplete="off"
                        aria-label="Search subject models"
                      />
                    </div>
                    <div
                      id="local-subject-options"
                      className="local-model-options"
                      role="listbox"
                      aria-label="Subject models"
                    ></div>
                  </div>
                </details>
              </div>
              <div className="local-field local-field-wide">
                <span>
                  Scenarios <small>select one or more</small>
                </span>
                <details
                  id="local-scenario-picker"
                  className="local-scenario-picker"
                  open
                >
                  <summary>
                    <strong id="local-scenario-summary">
                      Loading scenarios…
                    </strong>
                    <span>Choose</span>
                  </summary>
                  <div className="local-scenario-toolbar">
                    <button id="local-scenario-all" type="button">
                      Select all
                    </button>
                    <button id="local-scenario-none" type="button">
                      Clear
                    </button>
                  </div>
                  <div
                    id="local-scenario-options"
                    className="local-scenario-options"
                  ></div>
                </details>
              </div>
              <details
                id="local-advanced"
                className="local-advanced local-field-wide"
              >
                <summary>Advanced options</summary>
                <div className="local-advanced-grid">
                  <label className="local-field local-field-wide">
                    <span>
                      iii WebSocket URL <small>refresh after changing</small>
                    </span>
                    <input
                      name="url"
                      required
                      placeholder="ws://127.0.0.1:49134"
                    />
                  </label>
                  <div className="local-field local-field-wide">
                    <span>
                      Judge model <small>automatic when blank</small>
                    </span>
                    <select
                      id="local-judge"
                      name="judge"
                      disabled
                      hidden
                      aria-hidden="true"
                      tabIndex={-1}
                    >
                      <option value="">Use subject model when required</option>
                    </select>
                    <details
                      id="local-judge-picker"
                      className="local-model-picker local-picker-disabled"
                      aria-disabled="true"
                    >
                      <summary>
                        <strong id="local-judge-summary">
                          Use subject model when required
                        </strong>
                        <span>Choose</span>
                      </summary>
                      <div className="local-model-popover">
                        <div className="local-model-search">
                          <input
                            id="local-judge-search"
                            type="search"
                            placeholder="Search provider or model"
                            autoComplete="off"
                            aria-label="Search judge models"
                          />
                        </div>
                        <div
                          id="local-judge-options"
                          className="local-model-options"
                          role="listbox"
                          aria-label="Judge models"
                        ></div>
                      </div>
                    </details>
                  </div>
                  <label className="local-field">
                    <span>Runs</span>
                    <input
                      name="runs"
                      type="number"
                      min="1"
                      max="20"
                      defaultValue="1"
                      required
                    />
                  </label>
                  <label className="local-field">
                    <span>Technical retries</span>
                    <input
                      name="technical_retries"
                      type="number"
                      min="0"
                      max="3"
                      defaultValue="1"
                      required
                    />
                  </label>
                  <label className="local-field">
                    <span>
                      Case seed <small>canonical when blank</small>
                    </span>
                    <input
                      name="seed"
                      type="number"
                      min="0"
                      max="9007199254740991"
                      step="1"
                      placeholder="Deterministic default"
                    />
                  </label>
                </div>
              </details>
              <div className="local-run-actions">
                <button
                  id="local-run-submit"
                  className="button local-run-submit"
                  type="submit"
                >
                  Run selected E2E
                </button>
                <button
                  id="local-run-cancel"
                  className="button"
                  type="button"
                  hidden
                >
                  Cancel
                </button>
              </div>
            </form>
            <p
              id="local-run-error"
              className="local-run-error"
              role="alert"
              hidden
            ></p>
            <details
              id="local-run-log-shell"
              className="local-run-log-shell"
              hidden
            >
              <summary>Live runner output</summary>
              <pre
                id="local-run-log"
                className="local-run-log"
                aria-live="polite"
              ></pre>
            </details>
          </section>
        </dialog>

        <section id="empty-state" className="empty-state" hidden>
          <div className="empty-icon" aria-hidden="true">
            ⌁
          </div>
          <h2 id="empty-title">No executions published</h2>
          <p id="empty-description">
            The next scheduled Harness E2E workflow will appear here.
          </p>
        </section>

        <div id="overview-content" data-active-view={activeView}>
          <section
            id="latest-evidence"
            className="latest-evidence"
            data-workspace-view="overview"
            hidden={activeView !== 'overview'}
            aria-labelledby="latest-health-heading"
          >
            <article className="panel latest-health">
              <div className="latest-health-heading">
                <div>
                  <div className="section-kicker">01 / Current signal</div>
                  <h2 id="latest-health-heading">Latest execution</h2>
                </div>
                <span
                  id="latest-health-status"
                  className="status-pill status-incomplete"
                >
                  Waiting
                </span>
              </div>
              <h3 id="latest-health-title">Waiting for an execution</h3>
              <p id="latest-health-summary" className="trend-description">
                The newest execution will appear here with its report
                completeness and first actionable signal.
              </p>
              <section
                className="latest-health-meta"
                aria-label="Latest execution identity"
              >
                <span>
                  <small>Identity</small>
                  <strong id="latest-health-identity">—</strong>
                </span>
                <span>
                  <small>Lane</small>
                  <strong id="latest-health-lane">—</strong>
                </span>
                <span>
                  <small>Data</small>
                  <strong id="latest-health-availability">—</strong>
                </span>
                <span>
                  <small>Completed</small>
                  <strong id="latest-health-completed">—</strong>
                </span>
              </section>
              <div
                id="latest-first-failure"
                className="latest-first-failure"
                aria-live="polite"
              >
                <span className="latest-signal-icon" aria-hidden="true">
                  i
                </span>
                <div>
                  <strong>No execution selected</strong>
                  <p>
                    First failures and report gaps are surfaced here before
                    efficiency metrics.
                  </p>
                </div>
              </div>
              <div className="latest-health-actions">
                <a
                  id="latest-detail-link"
                  className="button button-primary"
                  href="./execution.html"
                >
                  Open execution
                </a>
                <a
                  id="latest-workflow-link"
                  className="button"
                  href="./index.html"
                  hidden
                >
                  Open workflow ↗
                </a>
              </div>
            </article>

            <section
              className="latest-kpi-grid"
              aria-label="Latest execution summary"
            >
              <article className="kpi-card">
                <div className="kpi-label">Scenario pass rate</div>
                <div id="kpi-pass-rate" className="kpi-value">
                  —
                </div>
                <div id="kpi-coverage" className="kpi-delta">
                  —
                </div>
              </article>
              <article className="kpi-card">
                <div className="kpi-label">Quality score</div>
                <div id="kpi-score" className="kpi-value">
                  —
                </div>
                <div className="kpi-delta">Mean of scenario medians</div>
              </article>
              <article className="kpi-card">
                <div className="kpi-label">Reliability events</div>
                <div id="kpi-failures" className="kpi-value">
                  —
                </div>
                <div className="kpi-delta">
                  Gates, technical failures, and missing reports
                </div>
              </article>
              <article className="kpi-card">
                <div className="kpi-label">Model cost</div>
                <div id="kpi-cost" className="kpi-value">
                  —
                </div>
                <div id="kpi-runtime" className="kpi-delta">
                  —
                </div>
              </article>
            </section>
          </section>

          <section
            id="comparison"
            className="panel overview-comparison"
            data-workspace-view="overview"
            hidden={activeView !== 'overview'}
            aria-labelledby="overview-comparison-heading"
          >
            <div className="panel-heading comparison-heading">
              <div>
                <div className="section-kicker">02 / Regression gate</div>
                <h2 id="overview-comparison-heading">Baseline → candidate</h2>
                <p className="trend-description">
                  Deltas unlock only when stack, subjects, runs, scenarios, and
                  behavioral contracts match.
                </p>
              </div>
              <span
                id="overview-comparison-verdict"
                className="comparison-verdict comparison-verdict-unavailable"
              >
                Waiting for two executions
              </span>
            </div>
            <div className="overview-comparison-controls">
              <label>
                <span>Baseline A</span>
                <select
                  id="overview-comparison-left"
                  aria-label="Baseline execution"
                ></select>
              </label>
              <button
                id="overview-comparison-swap"
                className="button comparison-swap"
                type="button"
                aria-label="Swap baseline and candidate"
              >
                ⇄
              </button>
              <label>
                <span>Candidate B</span>
                <select
                  id="overview-comparison-right"
                  aria-label="Candidate execution"
                ></select>
              </label>
              <a
                id="overview-comparison-open"
                className="button"
                href="./compare.html"
                aria-disabled="true"
              >
                Open full comparison →
              </a>
            </div>
            <p
              id="overview-comparison-summary"
              className="overview-comparison-summary"
            >
              Select two retained executions to compare their outcome guardrails
              and efficiency.
            </p>
            <div
              id="overview-comparison-metrics"
              className="overview-comparison-metrics"
              aria-live="polite"
            ></div>
          </section>

          <section
            id="capability"
            className="panel capability-panel"
            data-workspace-view="capability"
            hidden={activeView !== 'capability'}
            aria-labelledby="capability-heading"
          >
            <div className="panel-heading">
              <div>
                <div className="section-kicker">Evidence frontier</div>
                <h2 id="capability-heading">Proven complexity</h2>
                <p className="trend-description">
                  Confidence, sample depth, cost, and latency determine how far
                  the Harness can be trusted.
                </p>
              </div>
              <div className="capability-revision">
                <span>Subject revision</span>
                <code id="capability-revision">Unknown</code>
              </div>
            </div>

            <section
              className="capability-summary"
              aria-label="Current capability summary"
            >
              <article className="capability-card capability-primary">
                <div className="kpi-label">Reliable tier</div>
                <strong id="capability-reliable-tier">Not established</strong>
                <small id="capability-reliable-reason">
                  Waiting for policy and evidence
                </small>
              </article>
              <article className="capability-card">
                <div className="kpi-label">Statistically eligible</div>
                <strong id="capability-statistical-tier">—</strong>
                <small>Quality and reliability thresholds only</small>
              </article>
              <article className="capability-card">
                <div className="kpi-label">Observed runs</div>
                <strong id="capability-sample-size">—</strong>
                <small id="capability-sample-context">
                  Across materialized tiers
                </small>
              </article>
              <article className="capability-card">
                <div className="kpi-label">Policy</div>
                <strong id="capability-policy-version">—</strong>
                <small id="capability-policy">Independent thresholds</small>
              </article>
            </section>

            <div className="table-wrap capability-table-wrap">
              <table className="capability-table">
                <thead>
                  <tr>
                    <th scope="col">Tier</th>
                    <th scope="col">Runs</th>
                    <th scope="col">Deliverable</th>
                    <th scope="col">Structure</th>
                    <th scope="col">Technical failures</th>
                    <th scope="col">Flaky rate</th>
                    <th scope="col">p95 latency</th>
                    <th scope="col">p95 cost</th>
                    <th scope="col">Cost / success</th>
                    <th scope="col">Work amplification</th>
                    <th scope="col">Status</th>
                  </tr>
                </thead>
                <tbody id="capability-body"></tbody>
              </table>
            </div>
          </section>

          <section
            id="efficiency"
            className="panel efficiency-overview"
            data-workspace-view="overview"
            hidden={activeView !== 'overview'}
            aria-labelledby="efficiency-overview-heading"
          >
            <div className="panel-heading efficiency-heading">
              <div>
                <div className="section-kicker">03 / Comparable cohort</div>
                <h2 id="efficiency-overview-heading">Cost and work</h2>
                <p className="trend-description">
                  Only passed scenarios with unchanged contracts enter this
                  baseline. Drift remains visible in the guardrail.
                </p>
              </div>
              <div className="efficiency-heading-meta">
                <div className="efficiency-result" aria-live="polite">
                  <span>Latest result</span>
                  <strong
                    id="efficiency-status"
                    className="efficiency-result-value text-incomplete"
                  >
                    Waiting
                  </strong>
                  <small id="efficiency-status-caption">No execution</small>
                </div>
                <div className="efficiency-baseline">
                  <span>Baseline</span>
                  <strong>Median of up to 7 previous runs</strong>
                  <small id="efficiency-run-label">Waiting for data</small>
                </div>
              </div>
            </div>

            <section
              className="efficiency-card-grid"
              aria-label="Comparable cohort totals"
            >
              <article
                className="efficiency-card"
                data-efficiency-metric="cost_usd"
              >
                <div className="efficiency-card-label">Cost</div>
                <strong id="efficiency-cost">—</strong>
                <span id="efficiency-cost-delta" className="efficiency-delta">
                  —
                </span>
                <small
                  id="efficiency-cost-baseline"
                  className="efficiency-baseline-value"
                ></small>
                <div
                  id="efficiency-cost-sparkline"
                  className="efficiency-sparkline"
                ></div>
              </article>
              <article
                className="efficiency-card"
                data-efficiency-metric="tokens"
              >
                <div className="efficiency-card-label">Tokens</div>
                <strong id="efficiency-tokens">—</strong>
                <span id="efficiency-tokens-delta" className="efficiency-delta">
                  —
                </span>
                <small
                  id="efficiency-tokens-baseline"
                  className="efficiency-baseline-value"
                ></small>
                <div
                  id="efficiency-tokens-sparkline"
                  className="efficiency-sparkline"
                ></div>
              </article>
              <article
                className="efficiency-card"
                data-efficiency-metric="duration_seconds"
              >
                <div className="efficiency-card-label">Scenario time</div>
                <strong id="efficiency-duration">—</strong>
                <span
                  id="efficiency-duration-delta"
                  className="efficiency-delta"
                >
                  —
                </span>
                <small
                  id="efficiency-duration-baseline"
                  className="efficiency-baseline-value"
                ></small>
                <div
                  id="efficiency-duration-sparkline"
                  className="efficiency-sparkline"
                ></div>
              </article>
              <article
                className="efficiency-card"
                data-efficiency-metric="function_call_errors"
              >
                <div className="efficiency-card-label">Function errors</div>
                <strong id="efficiency-errors">—</strong>
                <span id="efficiency-errors-delta" className="efficiency-delta">
                  —
                </span>
                <small
                  id="efficiency-errors-baseline"
                  className="efficiency-baseline-value"
                ></small>
                <div
                  id="efficiency-errors-sparkline"
                  className="efficiency-sparkline"
                ></div>
              </article>
            </section>

            <div id="efficiency-guardrail" className="efficiency-guardrail">
              <span className="guardrail-status">
                Waiting for comparable scenarios
              </span>
            </div>
          </section>

          <section
            id="scenario-efficiency"
            className="panel scenario-efficiency-panel"
            data-workspace-view="scenarios"
            hidden={activeView !== 'scenarios'}
            aria-labelledby="scenario-efficiency-heading"
          >
            <div className="panel-heading">
              <div>
                <div className="section-kicker">Scenario economics</div>
                <h2 id="scenario-efficiency-heading">Work by scenario</h2>
                <p className="trend-description">
                  Inspect current values, contract drift, and comparable
                  baselines for each scenario.
                </p>
              </div>
            </div>
            <div className="table-wrap efficiency-table-wrap">
              <table className="efficiency-table">
                <thead>
                  <tr>
                    <th scope="col">Scenario</th>
                    <th scope="col">Cost</th>
                    <th scope="col">Tokens</th>
                    <th scope="col">Duration</th>
                    <th scope="col">Calls</th>
                    <th scope="col">Errors</th>
                    <th scope="col">Trend</th>
                  </tr>
                </thead>
                <tbody id="efficiency-body"></tbody>
              </table>
            </div>
          </section>

          <section
            id="scenarios"
            className="panel health-panel"
            data-workspace-view="scenarios"
            hidden={activeView !== 'scenarios'}
            aria-labelledby="health-heading"
          >
            <div className="panel-heading">
              <div>
                <div className="section-kicker">Scenario map</div>
                <h2 id="health-heading">Outcome matrix</h2>
                <p className="trend-description">
                  Read outcomes across attempts; select any cell for evidence.
                </p>
              </div>
              <fieldset className="range-toggle">
                <legend className="visually-hidden">
                  Matrix execution count
                </legend>
                <button
                  className="range-button active"
                  type="button"
                  data-count="14"
                >
                  14 runs
                </button>
                <button className="range-button" type="button" data-count="30">
                  30 runs
                </button>
              </fieldset>
            </div>
            <section className="matrix-legend" aria-label="Result legend">
              <span>
                <i
                  className="matrix-key matrix-passed matrix-key-score"
                  aria-hidden="true"
                >
                  92%
                </i>{' '}
                Score reached
              </span>
              <span>
                <i className="matrix-key matrix-failed" aria-hidden="true">
                  ×
                </i>{' '}
                Blocking failure
              </span>
              <span>
                <i className="matrix-key matrix-incomplete" aria-hidden="true">
                  –
                </i>{' '}
                Incomplete
              </span>
              <span>
                <i className="matrix-key matrix-cancelled" aria-hidden="true">
                  ○
                </i>{' '}
                Cancelled
              </span>
            </section>
            <div
              id="health-matrix"
              className="health-matrix-wrap"
              aria-live="polite"
            ></div>
          </section>

          <section
            id="executions"
            className="panel executions-panel"
            data-workspace-view="executions"
            hidden={activeView !== 'executions'}
            aria-labelledby="executions-heading"
          >
            <div className="panel-heading executions-heading">
              <div>
                <div className="section-kicker">Run ledger</div>
                <h2 id="executions-heading">Execution history</h2>
              </div>
              <span id="execution-count" className="coverage-note">
                0 executions
              </span>
            </div>
            <section className="table-filters" aria-label="Execution filters">
              <label className="search-field">
                <span className="visually-hidden">Search executions</span>
                <input
                  id="execution-search"
                  type="search"
                  placeholder="Search run, commit, or date"
                />
              </label>
              <label>
                <span className="visually-hidden">Filter by status</span>
                <select id="status-filter">
                  <option value="all">All statuses</option>
                  <option value="passed">Passed</option>
                  <option value="hard_gate_failed">Hard gate failed</option>
                  <option value="technical_failed">Technical failure</option>
                  <option value="infra_failed">Infrastructure failure</option>
                  <option value="incomplete">Incomplete</option>
                  <option value="cancelled">Cancelled</option>
                  <option value="running">Running</option>
                </select>
              </label>
              <label>
                <span className="visually-hidden">Filter by trigger</span>
                <select id="event-filter">
                  <option value="all">All triggers</option>
                  <option value="schedule">Scheduled</option>
                  <option value="workflow_dispatch">Manual</option>
                  <option value="local">Local</option>
                </select>
              </label>
            </section>
            <div id="comparison-bar" className="comparison-bar" hidden>
              <div>
                <strong id="comparison-count">Select two executions</strong>
                <span>
                  Selected runs fill explicit baseline and candidate slots;
                  incompatible evidence stays exploratory.
                </span>
              </div>
              <a
                id="comparison-link"
                className="button"
                href="./compare.html"
                aria-disabled="true"
              >
                Compare selected
              </a>
            </div>
            <div className="table-wrap">
              <table className="execution-table">
                <thead>
                  <tr>
                    <th scope="col">Execution</th>
                    <th scope="col">Result</th>
                    <th id="execution-commit-heading" scope="col">
                      Commit
                    </th>
                    <th scope="col">Subject</th>
                    <th scope="col">Scope</th>
                    <th scope="col">Quality</th>
                    <th scope="col">Efficiency</th>
                    <th scope="col">Evidence</th>
                  </tr>
                </thead>
                <tbody id="execution-body"></tbody>
              </table>
            </div>
            <nav className="pagination" aria-label="Execution table pagination">
              <button id="previous-page" type="button">
                Previous
              </button>
              <span id="page-label" aria-live="polite">
                Page 1
              </span>
              <button id="next-page" type="button">
                Next
              </button>
            </nav>
          </section>
        </div>

        <dialog
          id="scenario-history-dialog"
          className="scenario-history-dialog"
        >
          <div className="scenario-history-shell">
            <header className="scenario-history-header">
              <div>
                <div className="section-kicker">Scenario history</div>
                <h2 id="scenario-history-title">Scenario</h2>
                <p id="scenario-history-context">
                  Execution-by-execution efficiency.
                </p>
              </div>
              <button
                id="scenario-history-close"
                className="scenario-history-close"
                type="button"
                aria-label="Close scenario history"
              >
                ×
              </button>
            </header>

            <div
              className="scenario-history-tabs"
              role="tablist"
              aria-label="Scenario metric"
            >
              <button type="button" role="tab" data-history-metric="cost_usd">
                Cost
              </button>
              <button type="button" role="tab" data-history-metric="tokens">
                Tokens
              </button>
              <button
                type="button"
                role="tab"
                data-history-metric="duration_seconds"
              >
                Duration
              </button>
              <button
                type="button"
                role="tab"
                data-history-metric="function_calls"
              >
                Function calls
              </button>
              <button
                type="button"
                role="tab"
                data-history-metric="function_call_errors"
              >
                Function errors
              </button>
              <button
                type="button"
                role="tab"
                data-history-metric="work_amplification"
              >
                Work amplification
              </button>
              <button
                type="button"
                role="tab"
                data-history-metric="effective_fan_out"
              >
                Effective fan-out
              </button>
            </div>
            <p
              id="scenario-history-description"
              className="scenario-history-description"
            ></p>
            <div
              id="scenario-history-chart"
              className="scenario-history-chart"
              aria-live="polite"
            ></div>

            <div className="table-wrap scenario-history-table-wrap">
              <table className="scenario-history-table">
                <thead>
                  <tr>
                    <th scope="col">Execution</th>
                    <th scope="col">Value</th>
                    <th scope="col">Delta</th>
                    <th scope="col">Outcome</th>
                    <th scope="col">Contract</th>
                  </tr>
                </thead>
                <tbody id="scenario-history-body"></tbody>
              </table>
            </div>
          </div>
        </dialog>
      </main>

      <footer>
        <span id="dashboard-footer-summary">
          Harness E2E · 100 summaries · 30 compact diagnostic reports
        </span>
        <a href="https://github.com/iii-hq/harness-e2e">
          Suite documentation <span aria-hidden="true">↗</span>
        </a>
      </footer>
    </>
  )
}
