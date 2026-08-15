import { LegacyLoadError } from '@/components/LegacyLoadError'
import { ThemeToggle } from '@/components/ThemeToggle'
import { useLegacyPage } from '@/hooks/useLegacyPage'

export function ExecutionPage() {
  const error = useLegacyPage('execution')

  return (
    <>
      <LegacyLoadError error={error} />
      <a className="skip-link" href="#main">
        Skip to execution details
      </a>
      <div className="ambient ambient-one" aria-hidden="true"></div>
      <div className="ambient ambient-two" aria-hidden="true"></div>

      <header className="topbar">
        <a
          className="brand"
          href="https://github.com/iii-hq/workers"
          aria-label="iii workers"
        >
          <span className="brand-copy">
            <strong>iii</strong>
            <span>Harness benchmarks</span>
          </span>
        </a>
        <nav className="topbar-actions" aria-label="Execution actions">
          <ThemeToggle />
          <a
            id="workflow-link"
            className="button button-secondary"
            href="https://github.com/iii-hq/workers/actions"
            data-mobile-label="Workflow"
          >
            Open workflow <span aria-hidden="true">↗</span>
          </a>
        </nav>
      </header>

      <main id="main" className="page-shell detail-shell">
        <nav className="breadcrumbs" aria-label="Breadcrumb">
          <a href="./index.html">Executions</a>
          <span aria-hidden="true">/</span>
          <span id="breadcrumb-run">Run</span>
        </nav>

        <section
          id="detail-loading"
          className="detail-loading"
          aria-live="polite"
        >
          Loading execution data…
        </section>

        <section id="detail-error" className="empty-state" hidden>
          <div className="empty-icon" aria-hidden="true">
            !
          </div>
          <h1>Execution not found</h1>
          <p id="detail-error-message">
            This execution is not present in the retained history.
          </p>
          <a className="button" href="./index.html">
            Back to executions
          </a>
        </section>

        <div id="detail-content" hidden>
          <section
            className="execution-header"
            aria-labelledby="execution-title"
          >
            <div>
              <div className="eyebrow">Workflow execution</div>
              <div className="execution-title-row">
                <h1 id="execution-title">Run</h1>
                <span
                  id="execution-status"
                  className="status-pill status-incomplete"
                >
                  Loading
                </span>
                <span id="availability-badge" className="data-badge">
                  Unknown data
                </span>
              </div>
              <p id="execution-subtitle"></p>
            </div>
            <div id="execution-actions" className="execution-actions"></div>
          </section>

          <section
            id="detail-failures"
            className="detail-alert"
            hidden
            aria-labelledby="failures-title"
          >
            <div>
              <div className="section-kicker">Needs attention</div>
              <h2 id="failures-title">Execution failures</h2>
            </div>
            <div id="failure-summary"></div>
          </section>

          <section
            className="kpi-grid detail-kpis"
            aria-label="Execution metrics"
          >
            <article className="kpi-card kpi-primary">
              <div className="kpi-label">Scenario pass rate</div>
              <div id="detail-pass-rate" className="kpi-value">
                —
              </div>
              <div id="detail-coverage" className="kpi-delta">
                —
              </div>
              <div className="kpi-orbit" aria-hidden="true"></div>
            </article>
            <article className="kpi-card">
              <div className="kpi-label">Quality score</div>
              <div id="detail-score" className="kpi-value">
                —
              </div>
              <div className="kpi-delta">Mean of scenario medians</div>
            </article>
            <article className="kpi-card">
              <div className="kpi-label">Model cost</div>
              <div id="detail-cost" className="kpi-value">
                —
              </div>
              <div className="kpi-delta">Subject and judge</div>
            </article>
            <article className="kpi-card">
              <div className="kpi-label">Model runtime</div>
              <div id="detail-runtime" className="kpi-value">
                —
              </div>
              <div id="workflow-runtime" className="kpi-delta">
                —
              </div>
            </article>
          </section>

          <div className="detail-layout">
            <aside
              className="detail-index"
              aria-label="Execution detail sections"
            >
              <span>On this page</span>
              <a href="#detail-failures">Failures</a>
              <a href="#overview">Overview</a>
              <a href="#configuration">Configuration</a>
              <a href="#scenarios">Scenarios and runs</a>
              <a href="#raw-data">Raw data</a>
            </aside>

            <div className="detail-main">
              <section
                id="overview"
                className="detail-section"
                aria-labelledby="overview-heading"
              >
                <div className="section-kicker">Execution context</div>
                <h2 id="overview-heading">Overview</h2>
                <details className="section-disclosure">
                  <summary>
                    <span id="overview-digest" className="section-digest">
                      Loading…
                    </span>
                    <span className="section-chevron" aria-hidden="true">
                      ⌄
                    </span>
                  </summary>
                  <div id="metadata-grid" className="metadata-grid"></div>
                </details>
              </section>

              <section
                id="configuration"
                className="detail-section"
                aria-labelledby="configuration-heading"
              >
                <div className="section-kicker">Resolved stack</div>
                <h2 id="configuration-heading">Configuration</h2>
                <details className="section-disclosure">
                  <summary>
                    <span id="configuration-digest" className="section-digest">
                      Loading…
                    </span>
                    <span className="section-chevron" aria-hidden="true">
                      ⌄
                    </span>
                  </summary>
                  <div id="configuration-content"></div>
                </details>
              </section>

              <section
                id="scenarios"
                className="detail-section"
                aria-labelledby="scenarios-heading"
              >
                <div className="section-kicker">Complete result set</div>
                <h2 id="scenarios-heading">Scenarios and runs</h2>
                <p id="scenario-intro" className="section-description"></p>
                <div id="scenario-details" className="scenario-details"></div>
              </section>

              <section
                id="raw-data"
                className="detail-section"
                aria-labelledby="raw-heading"
              >
                <div className="section-kicker">Source record</div>
                <h2 id="raw-heading">Raw data</h2>
                <p className="section-description">
                  The structured views above preserve the source report. Use the
                  raw record for fields that do not yet have a dedicated
                  visualization.
                </p>
                <div id="raw-actions" className="raw-actions"></div>
                <details id="raw-preview" className="raw-details">
                  <summary>Preview JSON</summary>
                  <pre id="raw-json"></pre>
                </details>
              </section>
            </div>
          </div>
        </div>

        <dialog
          id="session-transcript-dialog"
          className="session-transcript-dialog"
          aria-labelledby="session-transcript-title"
        >
          <div className="session-transcript-shell">
            <header className="session-transcript-header">
              <div>
                <div className="section-kicker">Session transcript</div>
                <h2 id="session-transcript-title">Execution conversation</h2>
                <p id="session-transcript-context"></p>
              </div>
              <button
                id="session-transcript-close"
                className="session-transcript-close"
                type="button"
                aria-label="Close session transcript"
              >
                ×
              </button>
            </header>
            <div
              id="session-transcript-body"
              className="session-transcript-body"
            ></div>
          </div>
        </dialog>
      </main>

      <footer>
        <span>Harness E2E · public execution report</span>
        <a href="./index.html">Back to all executions</a>
      </footer>
    </>
  )
}
