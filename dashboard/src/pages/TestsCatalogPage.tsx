import { useEffect, useState } from 'react'
import { ThemeToggle } from '@/components/ThemeToggle'
import {
  hashForComparison,
  hashForNewPlan,
  hashForPlans,
  hashForTestHistory,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import type { TestCatalogRow, TestsListResponse } from '@/lib/test-catalog'

function versionLabel(row: TestCatalogRow) {
  if (!row.current_version) return 'No current version'
  const version = row.available_versions.find(
    (item) => item.version === row.current_version,
  )
  return `v${row.current_version} · ${version?.execution_count ?? 0} executions`
}

function lifecycleStatusClass(lifecycle: TestCatalogRow['lifecycle']) {
  if (lifecycle === 'active') return 'status-catalog-active'
  if (lifecycle === 'never_run') return 'status-catalog-never-run'
  return 'status-catalog-retired'
}

export function TestsCatalogPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [data, setData] = useState<TestsListResponse | null>(null)
  const [query, setQuery] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void getDashboardDataBridge()
      .then(async (next) => {
        if (cancelled) return
        setBridge(next)
        setData(await next.listTests({ limit: 100 }))
      })
      .catch((cause) => {
        if (!cancelled)
          setError(cause instanceof Error ? cause.message : String(cause))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const rows = (data?.rows ?? []).filter((row) =>
    row.test_id.toLowerCase().includes(query.trim().toLowerCase()),
  )
  const local = bridge?.mode === 'local'

  return (
    <>
      <a className="skip-link" href="#tests-catalog-main">
        Skip to test catalog
      </a>
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
        <nav className="topbar-actions" aria-label="Test catalog actions">
          <a
            className="button button-secondary"
            href={hashForWorkspace()}
            data-mobile-label="Overview"
          >
            Overview
          </a>
          {local && (
            <a
              className="button button-secondary"
              href={hashForPlans()}
              data-mobile-label="Plans"
            >
              Plans
            </a>
          )}
          {local && (
            <a className="button button-primary" href={hashForNewPlan()}>
              ＋ New local plan
            </a>
          )}
          <a className="button button-secondary" href={hashForComparison()}>
            System comparison
          </a>
          <ThemeToggle />
        </nav>
      </header>
      <main id="tests-catalog-main" className="page-shell overview-shell">
        <section className="page-heading" aria-labelledby="tests-catalog-title">
          <div>
            <div className="eyebrow">
              <span className="live-dot" aria-hidden="true" />
              Test catalog
            </div>
            <h1 id="tests-catalog-title">Choose a test to inspect</h1>
            <p>
              History describes one test over time. It is intentionally separate
              from system-version comparison.
            </p>
          </div>
          <div className="sync-block">
            <span>Catalog revision</span>
            <strong className="font-mono text-xs text-ink-muted">
              {data?.revision.slice(-12) ?? 'loading'}
            </strong>
          </div>
        </section>
        {error && (
          <section className="empty-state" role="alert">
            <h2>Test catalog unavailable</h2>
            <p>{error}</p>
          </section>
        )}
        {!error && (
          <section
            className="panel test-catalog-panel"
            aria-labelledby="test-catalog-heading"
          >
            <div className="panel-heading">
              <div>
                <div className="section-kicker">Independent history</div>
                <h2 id="test-catalog-heading">Tests</h2>
              </div>
              <label className="search-field">
                <span className="visually-hidden">Search tests</span>
                <input
                  type="search"
                  placeholder="Search test id"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                />
              </label>
            </div>
            {loading ? (
              <p className="table-empty">Loading tests…</p>
            ) : rows.length === 0 ? (
              <p className="table-empty">No tests match this filter.</p>
            ) : (
              <div className="table-wrap">
                <table className="execution-table test-catalog-table">
                  <thead>
                    <tr>
                      <th scope="col">Test</th>
                      <th scope="col">Version</th>
                      <th scope="col">Lifecycle</th>
                      <th scope="col">History</th>
                    </tr>
                  </thead>
                  <tbody>
                    {rows.map((row) => (
                      <tr key={row.test_id}>
                        <td data-label="Test">
                          <strong>{row.test_id}</strong>
                          <small className="block text-ink-muted">
                            {row.available_versions.length} contract version
                            {row.available_versions.length === 1 ? '' : 's'}
                          </small>
                        </td>
                        <td data-label="Version">{versionLabel(row)}</td>
                        <td data-label="Lifecycle">
                          <span
                            className={`status-pill ${lifecycleStatusClass(row.lifecycle)}`}
                          >
                            {row.lifecycle.replace('_', ' ')}
                          </span>
                        </td>
                        <td data-label="History">
                          {local ? (
                            <a
                              className="button button-secondary"
                              href={hashForTestHistory(row.test_id)}
                            >
                              View metrics
                            </a>
                          ) : (
                            <span className="text-sm text-ink-muted">
                              Local dashboard only
                            </span>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>
        )}
      </main>
      <footer>
        <span>Harness E2E · test catalog</span>
        <a href={hashForWorkspace()}>Back to home</a>
      </footer>
    </>
  )
}
