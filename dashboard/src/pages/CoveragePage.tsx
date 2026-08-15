import { LegacyLoadError } from '@/components/LegacyLoadError'
import { ThemeToggle } from '@/components/ThemeToggle'
import { useLegacyPage } from '@/hooks/useLegacyPage'

export function CoveragePage() {
  const error = useLegacyPage('coverage')

  return (
    <>
      <LegacyLoadError error={error} />
      <a className="skip-link" href="#main">
        Skip to coverage reports
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
        <nav className="topbar-actions" aria-label="Dashboard actions">
          <ThemeToggle />
          <a
            className="button button-secondary"
            href="../index.html"
            data-mobile-label="Runs"
          >
            Executions
          </a>
          <a
            className="button button-secondary"
            href="../coverage-trend/"
            data-mobile-label="Trend"
          >
            Trend
          </a>
        </nav>
      </header>

      <main id="main" className="page-shell overview-shell">
        <section className="page-heading" aria-labelledby="page-title">
          <div>
            <div className="eyebrow">
              <span className="live-dot" aria-hidden="true"></span>
              Harness stack
            </div>
            <h1 id="page-title">Code coverage</h1>
            <p>
              LLVM line coverage of the instrumented stack, refreshed by the
              daily E2E cycle.
            </p>
          </div>
          <div className="sync-block">
            <span id="sync-label">Last published</span>
            <time id="last-update" dateTime="">
              Waiting for data
            </time>
          </div>
        </section>

        <section id="empty-state" className="empty-state" hidden>
          <div className="empty-icon" aria-hidden="true">
            ⌁
          </div>
          <h2>No coverage published yet</h2>
          <p>
            The next scheduled Harness E2E Daily workflow will publish the first
            reports here.
          </p>
        </section>

        <section id="coverage-content" className="coverage-grid" hidden>
          <article className="panel coverage-card" id="card-integration">
            <h2>Integration suite</h2>
            <div className="coverage-percent" data-percent>
              –
            </div>
            <div className="coverage-meta" data-meta>
              Not published yet.
            </div>
            <div className="coverage-links">
              <a
                className="button button-secondary"
                href="./integration/index.html"
                data-report
                hidden
              >
                Browse report
              </a>
            </div>
          </article>
          <article className="panel coverage-card" id="card-e2e">
            <h2>E2E suite</h2>
            <div className="coverage-percent" data-percent>
              –
            </div>
            <div className="coverage-meta" data-meta>
              Not published yet.
            </div>
            <div className="coverage-links">
              <a
                className="button button-secondary"
                href="./e2e/index.html"
                data-report
                hidden
              >
                Browse report
              </a>
            </div>
          </article>
        </section>
      </main>
    </>
  )
}
