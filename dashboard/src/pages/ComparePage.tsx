import { LegacyLoadError } from '@/components/LegacyLoadError'
import { ThemeToggle } from '@/components/ThemeToggle'
import { hashForWorkspace } from '@/hooks/use-hash-route'
import { useLegacyPage } from '@/hooks/useLegacyPage'

export function ComparePage() {
  const error = useLegacyPage('compare')

  return (
    <>
      <LegacyLoadError error={error} />
      <a className="skip-link" href="#main">
        Skip to comparison
      </a>
      <div className="ambient ambient-one" aria-hidden="true"></div>
      <div className="ambient ambient-two" aria-hidden="true"></div>

      <header className="topbar">
        <a
          className="brand"
          href={hashForWorkspace()}
          aria-label="Harness E2E dashboard"
        >
          <span className="brand-copy">
            <strong>iii</strong>
            <span>Harness benchmarks</span>
          </span>
        </a>
        <nav className="topbar-actions" aria-label="Comparison actions">
          <ThemeToggle />
          <a
            className="button"
            href={hashForWorkspace()}
            data-mobile-label="Back"
          >
            ← All executions
          </a>
        </nav>
      </header>

      <main id="main" className="page-shell compare-shell">
        <section className="page-heading" aria-labelledby="page-title">
          <div>
            <div className="eyebrow">
              <span className="live-dot" aria-hidden="true"></span>Local E2E
            </div>
            <h1 id="page-title">Execution comparison</h1>
            <p>
              Candidate B relative to baseline A. Incompatible evidence stays
              visible without regression labels.
            </p>
          </div>
        </section>

        <section id="compare-empty" className="empty-state" hidden>
          <div className="empty-icon" aria-hidden="true">
            ⌁
          </div>
          <h2>Select two existing executions</h2>
          <p>
            Return to the execution dashboard and mark any two rows for
            comparison.
          </p>
        </section>

        <div id="compare-content" className="compare-content" hidden>
          <section className="panel" aria-labelledby="selected-heading">
            <div className="panel-heading">
              <div>
                <div className="section-kicker">Selection</div>
                <h2 id="selected-heading">Baseline versus candidate</h2>
              </div>
              <span
                id="compare-verdict"
                className="comparison-verdict comparison-verdict-unavailable"
              >
                Checking compatibility
              </span>
            </div>
            <div
              id="compare-selection"
              className="compare-selection-grid"
            ></div>
            <ul id="compare-warnings" className="compare-warning-list"></ul>
          </section>

          <section
            className="panel executions-panel"
            aria-labelledby="overall-heading"
          >
            <div className="panel-heading">
              <div>
                <div className="section-kicker">Whole execution</div>
                <h2 id="overall-heading">Overall delta</h2>
                <p className="trend-description">
                  Positive and negative deltas always mean B minus A.
                </p>
              </div>
            </div>
            <div id="compare-metrics" className="compare-metric-grid"></div>
          </section>

          <section
            className="panel executions-panel"
            aria-labelledby="scenario-heading"
          >
            <div className="panel-heading">
              <div>
                <div className="section-kicker">Per scenario</div>
                <h2 id="scenario-heading">Scenario deltas</h2>
                <p className="trend-description">
                  Changed contracts are compared but explicitly marked.
                </p>
              </div>
            </div>
            <div className="table-wrap">
              <table className="compare-table">
                <thead>
                  <tr>
                    <th scope="col">Scenario</th>
                    <th scope="col">Execution A</th>
                    <th scope="col">Execution B</th>
                    <th scope="col">Δ score</th>
                    <th scope="col">Δ tokens</th>
                    <th scope="col">Δ cost</th>
                    <th scope="col">Δ time</th>
                    <th scope="col">Contract</th>
                  </tr>
                </thead>
                <tbody id="compare-scenarios"></tbody>
              </table>
            </div>
          </section>
        </div>
      </main>
    </>
  )
}
