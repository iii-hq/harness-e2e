import {
  Button,
  Checkbox,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  Input as HostInput,
  SegmentedControl,
} from '@iii-dev/console-ui'
import { Check, ChevronRight, Copy, Info, Search, X } from 'lucide-react'
import {
  type CSSProperties,
  type FormEvent,
  type MouseEvent,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
  type HeaderAction,
} from '@/components/DashboardPageActions'
import { useDashboardChrome } from '@/components/DashboardShell'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import {
  buttonClassName,
  Callout,
  EmptyState,
  Input,
  isInteractiveTarget,
  Select,
  StatusLabel,
} from '@/design-system'
import {
  hashForSuites,
  hashForTestHistory,
  hashForVersionComparison,
  replaceRouteParams,
  routeParams,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
  type Suite,
} from '@/lib/dashboard-data-source'
import { plural } from '@/lib/format'
import type { TestCatalogRow } from '@/lib/test-catalog'
import {
  CATALOG_DEFAULT_FILTERS,
  CATALOG_SORTS,
  type CatalogFilter,
  type CatalogFilters,
  type CatalogRowView,
  type CatalogSort,
  catalogFiltersFromParams,
  catalogFiltersToParams,
  catalogRowView,
  catalogSegments,
  catalogSummary,
  filterCatalog,
  groupCatalog,
  suitesByTest,
} from '@/lib/test-catalog-view'
import {
  shownSelection,
  toggleSelection,
  toggleShown,
} from '@/pages/ExecutionsPage'
import '@/design-system/styles.css'
import './executions-page.css'
import './tests-catalog.css'

/** The worker lists at most this many tests per request. */
const PAGE_SIZE = 100

/** Every test, a page at a time: the families and the counts need them all. */
export async function listAllTests(bridge: DashboardDataBridge) {
  const rows: TestCatalogRow[] = []
  let cursor: string | undefined
  let revision = ''
  for (;;) {
    const page = await bridge.listTests({
      limit: PAGE_SIZE,
      ...(cursor ? { cursor } : {}),
    })
    rows.push(...page.rows)
    revision = page.revision
    cursor = page.next_cursor ?? undefined
    if (!cursor) return { rows, revision }
  }
}

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** The selection bar: how many, and the Run button's label. */
export function catalogSelection(count: number) {
  return {
    text: `${plural(count, 'test')} selected`,
    hint: 'Runs open the Run tests dialog with these ticked.',
    run: `Run ${plural(count, 'test')}`,
  }
}

/** The section's actions in the Console header: the version comparison and,
 *  once the worker answers, Run tests. */
export function catalogHeaderActions(onRun?: () => void): HeaderAction[] {
  return [
    {
      id: 'compare',
      label: 'Compare system versions',
      href: hashForVersionComparison(),
    },
    ...(onRun
      ? [{ id: 'run', label: 'Run tests', primary: true, onSelect: onRun }]
      : []),
  ]
}

function CatalogRevision({ revision }: { revision: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <button
      className="tc-revision"
      type="button"
      title={copied ? 'Copied' : 'Copy the full catalog revision'}
      aria-label={`Copy the full catalog revision, ${revision}`}
      onClick={() => {
        void navigator.clipboard?.writeText(revision).then(() => {
          setCopied(true)
          window.setTimeout(() => setCopied(false), 1500)
        })
      }}
    >
      catalog {revision.slice(-12)}
      {copied ? (
        <Check size={16} aria-hidden="true" />
      ) : (
        <Copy size={16} aria-hidden="true" />
      )}
    </button>
  )
}

/** A test's last result: its label and score over when it ran. Faded when
 *  it ran an earlier definition. Suites shows it too. */
export function LastResult({ view }: { view: CatalogRowView }) {
  return (
    <td className="tc-stack" data-older={view.older || undefined}>
      <span className="tc-result">
        <StatusLabel state={view.result} />
        {view.score === null ? null : (
          <span className="tc-score">{view.score}</span>
        )}
      </span>
      <span className="tc-sub" title={view.whenTitle ?? undefined}>
        {view.when}
      </span>
    </td>
  )
}

/** The last scores as bars, oldest first; a run without a score is a stub. */
export function Sparkline({ view }: { view: CatalogRowView }) {
  return (
    <span className="tc-spark" role="img" aria-label={view.sparkLabel}>
      {view.spark.map((score, index) => (
        <span
          // biome-ignore lint/suspicious/noArrayIndexKey: bars in run order
          key={index}
          data-empty={score === null || undefined}
          style={
            score === null
              ? undefined
              : ({ '--tc-score': score } as CSSProperties)
          }
        />
      ))}
    </span>
  )
}

export type CatalogTableProps = {
  /** Every test: a family's note counts all of it. */
  all: CatalogRowView[]
  shown: CatalogRowView[]
  selected: string[]
  onSelect: (ids: string[]) => void
  /** A narrow pane keeps the box, the test, its last result and the link. */
  narrow?: boolean
  highlightId?: string | null
}

/** The tests, one block per family: a heading with its box, note and count,
 *  then its rows. The row opens the test's history; the id link and the
 *  chevron are the keyboard path. */
export function CatalogTable({
  all,
  shown,
  selected,
  onSelect,
  narrow = false,
  highlightId = null,
}: CatalogTableProps) {
  const groups = groupCatalog(all, shown)
  const selectable = (views: CatalogRowView[]) =>
    views.filter((view) => view.selectable).map((view) => view.id)
  const shownIds = selectable(shown)
  const every = shownSelection(selected, shownIds)
  const open = (id: string) => (event: MouseEvent<HTMLTableRowElement>) => {
    if (!isInteractiveTarget(event.target))
      window.location.hash = hashForTestHistory(id)
  }
  return (
    <table
      className="tc-table"
      aria-label={`Tests, ${shown.length} of ${all.length}`}
      data-narrow={narrow || undefined}
      data-catalog-rows
    >
      <thead>
        <tr className="tc-row tc-columns">
          <th scope="col">
            <Checkbox
              aria-label="Select every test shown"
              checked={every === 'all'}
              indeterminate={every === 'some'}
              onChange={() => onSelect(toggleShown(selected, shownIds))}
            />
          </th>
          <th scope="col">Test</th>
          <th scope="col">Last result</th>
          {narrow ? null : (
            <>
              <th scope="col">Recent scores</th>
              <th scope="col" className="tc-num">
                Runs
              </th>
              <th scope="col">Suites</th>
            </>
          )}
          <th scope="col">
            <span className="ds-visually-hidden">History</span>
          </th>
        </tr>
      </thead>
      {groups.map((group) => {
        const ids = selectable(group.rows)
        const state = shownSelection(selected, ids)
        return (
          <tbody
            key={group.key}
            className="tc-group"
            aria-label={group.label}
            data-catalog-group={group.key}
          >
            <tr className="tc-group-head">
              <th scope="rowgroup" colSpan={narrow ? 4 : 7}>
                <Checkbox
                  aria-label={`Select every test in ${group.label}`}
                  checked={state === 'all'}
                  indeterminate={state === 'some'}
                  disabled={ids.length === 0}
                  onChange={() => onSelect(toggleShown(selected, ids))}
                />
                <span
                  className="tc-group-label"
                  data-mono={group.mono || undefined}
                >
                  {group.label}
                </span>
                <span className="tc-group-note">{group.note}</span>
                <span className="tc-group-count">
                  {group.rows.length}
                  <span className="ds-visually-hidden">
                    {group.rows.length === 1 ? ' test' : ' tests'}
                  </span>
                </span>
              </th>
            </tr>
            {group.rows.map((view) => {
              const ticked = selected.includes(view.id)
              const history = hashForTestHistory(view.id)
              return (
                <tr
                  key={view.id}
                  id={`test-${view.id}`}
                  className="tc-row tc-test"
                  data-test-id={view.id}
                  data-selected={ticked || undefined}
                  data-highlighted={highlightId === view.id || undefined}
                  onClick={open(view.id)}
                >
                  <td>
                    {view.selectable ? (
                      <Checkbox
                        aria-label={`Select ${view.id}`}
                        checked={ticked}
                        onChange={() =>
                          onSelect(toggleSelection(selected, view.id))
                        }
                      />
                    ) : null}
                  </td>
                  <td className="tc-stack">
                    <a className="tc-id" href={history}>
                      {view.id}
                    </a>
                    {view.sub ? (
                      <span className="tc-sub" title={view.sub}>
                        {view.sub}
                      </span>
                    ) : null}
                  </td>
                  <LastResult view={view} />
                  {narrow ? null : (
                    <>
                      <td>
                        <Sparkline view={view} />
                      </td>
                      <td className="tc-num" title={view.runsTitle}>
                        {view.runs}
                      </td>
                      <td className="tc-suites">
                        {view.suites.map((suite) => (
                          <span key={suite.id} title={suite.label}>
                            {suite.label}
                          </span>
                        ))}
                      </td>
                    </>
                  )}
                  <td>
                    <a
                      className="ex-icon-button"
                      href={history}
                      aria-label={`History of ${view.id}`}
                    >
                      <ChevronRight size={16} aria-hidden="true" />
                    </a>
                  </td>
                </tr>
              )
            })}
          </tbody>
        )
      })}
    </table>
  )
}

/** Names the ticked tests as a suite of this Console. */
function SaveSuiteDialog({
  tests,
  open,
  onClose,
  onSave,
}: {
  tests: string[]
  open: boolean
  onClose: () => void
  onSave: (label: string) => Promise<void>
}) {
  const [label, setLabel] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (open) {
      setLabel('')
      setError(null)
    }
  }, [open])
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!label.trim()) {
      setError('Name the suite.')
      return
    }
    setSaving(true)
    setError(null)
    try {
      await onSave(label.trim())
      onClose()
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSaving(false)
    }
  }
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && !saving) onClose()
      }}
    >
      <DialogContent className="ex-dialog">
        <DialogTitle className="ex-dialog-title">Save as suite</DialogTitle>
        <DialogDescription className="ex-dialog-body">
          A suite of this Console with the {plural(tests.length, 'test')}{' '}
          ticked, one run each. A test of a sequential group brings its whole
          group.
        </DialogDescription>
        <form className="ex-rename" onSubmit={(event) => void submit(event)}>
          <HostInput
            aria-label="Suite name"
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? 'tc-save-suite-error' : undefined}
            maxLength={160}
            placeholder="Suite name"
            value={label}
            onChange={setLabel}
          />
          {error ? (
            <p id="tc-save-suite-error" className="ex-error" role="alert">
              {error}
            </p>
          ) : null}
          <div className="ex-dialog-actions">
            <Button
              type="button"
              variant="pill"
              size="sm"
              disabled={saving}
              onClick={onClose}
            >
              Cancel
            </Button>
            <Button type="submit" variant="primary" size="sm" disabled={saving}>
              {saving ? 'Saving…' : 'Save suite'}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  )
}

type Saved = { id: string; label: string; count: number; warnings: string[] }

/** Creates the suite, then lists the suites again for the chips and the
 *  filter. Once it is created, a list that fails to load only leaves the
 *  chips as they were: the save is not reported as failed, which would
 *  invite saving it twice. */
export async function saveTestsAsSuite(
  bridge: Pick<DashboardDataBridge, 'createSuiteOfTests' | 'listSuites'>,
  tests: string[],
  label: string,
): Promise<{ saved: Saved; suites: Suite[] | null }> {
  const suite = await bridge.createSuiteOfTests(tests, label)
  const suites = await bridge
    .listSuites()
    .then((listed) => listed.suites)
    .catch(() => null)
  return {
    saved: {
      id: suite.id,
      label: suite.label,
      count: suite.scenarios.length,
      warnings: suite.warnings ?? [],
    },
    suites,
  }
}

export function TestsCatalogPage() {
  const narrow = useDashboardChrome()?.narrow ?? false
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [rows, setRows] = useState<TestCatalogRow[]>([])
  const [revision, setRevision] = useState('')
  const [suites, setSuites] = useState<Suite[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [filters, setFilters] = useState<CatalogFilters>(() =>
    typeof window === 'undefined'
      ? CATALOG_DEFAULT_FILTERS
      : catalogFiltersFromParams(routeParams(window.location.hash)),
  )
  // Ticked tests, in the order they were ticked.
  const [selected, setSelected] = useState<string[]>([])
  // The tests Run tests opens with, while it is open, and which opening it
  // is: each one mounts the dialog afresh, so the ticks of the last one do
  // not carry over.
  const [runner, setRunner] = useState<string[] | null>(null)
  const [opening, setOpening] = useState(0)
  const openRunner = useCallback((scope: string[]) => {
    setOpening((count) => count + 1)
    setRunner(scope)
  }, [])
  const [naming, setNaming] = useState(false)
  const [saved, setSaved] = useState<Saved | null>(null)
  const [highlightId, setHighlightId] = useState<string | null>(() =>
    typeof window === 'undefined'
      ? null
      : routeParams(window.location.hash).get('highlight'),
  )
  const beginRequest = useLatestRequest()

  const load = useCallback(async () => {
    const request = beginRequest()
    setError(null)
    setLoading(true)
    try {
      const next = await getDashboardDataBridge()
      if (!request.isCurrent()) return
      setBridge(next)
      const [listed, suiteList] = await Promise.all([
        listAllTests(next),
        // The suites are chips and a filter: the tests show without them.
        next.listSuites().catch(() => ({ suites: [] as Suite[] })),
      ])
      if (!request.isCurrent()) return
      setRows(listed.rows)
      setRevision(listed.revision)
      setSuites(suiteList.suites)
    } catch (cause) {
      if (request.isCurrent()) setError(errorText(cause))
    } finally {
      if (request.isCurrent()) setLoading(false)
    }
  }, [beginRequest])

  useEffect(() => {
    void load()
  }, [load])

  // A finished run changes a test's last result: the catalog follows run
  // changes (they also clear the bridge's cached lists) as Executions does.
  useEffect(() => {
    if (!bridge) return
    let cancelled = false
    let dispose: (() => void) | undefined
    let timer: number | undefined
    bridge
      .subscribeRunChanges(() => {
        if (timer) window.clearTimeout(timer)
        timer = window.setTimeout(() => void load(), 400)
      })
      .then((off) => {
        if (cancelled) off()
        else dispose = off
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
      if (timer) window.clearTimeout(timer)
      dispose?.()
    }
  }, [bridge, load])

  // Audit T-08: the filters live in the URL.
  useEffect(() => {
    const params = catalogFiltersToParams(filters)
    if (highlightId) params.set('highlight', highlightId)
    replaceRouteParams(params)
  }, [filters, highlightId])

  // A plain "#/tests" (the section link) clears the filters; the route
  // identity does not change, so the page listens for the hash itself.
  useEffect(() => {
    const sync = () => {
      const next = catalogFiltersFromParams(routeParams(window.location.hash))
      setFilters((current) =>
        catalogFiltersToParams(current).toString() ===
        catalogFiltersToParams(next).toString()
          ? current
          : next,
      )
    }
    window.addEventListener('hashchange', sync)
    return () => window.removeEventListener('hashchange', sync)
  }, [])

  const bySuite = useMemo(() => suitesByTest(suites), [suites])
  const views = useMemo(
    () =>
      rows.map((row) => catalogRowView(row, bySuite.get(row.test_id) ?? [])),
    [rows, bySuite],
  )
  const shown = useMemo(() => filterCatalog(views, filters), [views, filters])

  // Back from a test's history: its row, in view and marked for a moment.
  useEffect(() => {
    if (!highlightId || loading) return
    document
      .getElementById(`test-${highlightId}`)
      ?.scrollIntoView({ block: 'center' })
    const timer = window.setTimeout(() => setHighlightId(null), 2000)
    return () => window.clearTimeout(timer)
  }, [highlightId, loading])

  const headerActions = useMemo(
    () => catalogHeaderActions(bridge ? () => openRunner([]) : undefined),
    [bridge, openRunner],
  )

  const setFilter = <K extends keyof CatalogFilters>(
    key: K,
    value: CatalogFilters[K],
  ) => setFilters((current) => ({ ...current, [key]: value }))
  const filtered = catalogFiltersToParams(filters).toString() !== ''
  // A tick outlives a reload only while its test is still listed.
  const ticked = selected.filter((id) =>
    views.some((view) => view.id === id && view.selectable),
  )
  const bar = catalogSelection(ticked.length)
  const failedFirstLoad = Boolean(error) && rows.length === 0
  const suiteOptions = suites.filter((suite) => suite.scenarios.length > 0)

  const saveSuite = async (label: string) => {
    if (!bridge) return
    const done = await saveTestsAsSuite(bridge, ticked, label)
    setSaved(done.saved)
    if (done.suites) setSuites(done.suites)
  }

  return (
    <div className="ds-root ex-page tc-page">
      <DashboardPageActions
        active="tests"
        actionsLabel="Test catalog actions"
        actions={headerActions}
      />
      <div className="tc-head">
        <header className="ex-header">
          <h1 id="tests-catalog-title">Tests</h1>
          {failedFirstLoad ? null : (
            <p>
              {loading && rows.length === 0
                ? 'Loading the catalog…'
                : catalogSummary(views)}
            </p>
          )}
        </header>
        {revision ? <CatalogRevision revision={revision} /> : null}
      </div>
      <p className="ds-visually-hidden" role="status">
        {loading || failedFirstLoad
          ? ''
          : `Tests, ${shown.length} of ${views.length}.`}
      </p>

      {error && !failedFirstLoad ? (
        <Callout tone="danger" title="The catalog could not be reloaded">
          <span className="ex-callout-line">
            {error}
            <button
              className={buttonClassName({
                variant: 'secondary',
                size: 'compact',
              })}
              type="button"
              onClick={() => void load()}
            >
              try again
            </button>
          </span>
        </Callout>
      ) : null}

      {failedFirstLoad ? null : (
        <section className="ex-toolbar" aria-label="Test catalog filters">
          <div className="ex-search">
            <Search size={16} aria-hidden="true" />
            <Input
              type="text"
              value={filters.query}
              placeholder="Filter by name, summary or suite"
              aria-label="Filter tests"
              onChange={(event) => setFilter('query', event.target.value)}
            />
            {filters.query ? (
              <button
                className="ex-icon-button"
                type="button"
                onClick={() => setFilter('query', '')}
                aria-label="Clear search"
              >
                <X size={14} aria-hidden="true" />
              </button>
            ) : null}
          </div>
          <SegmentedControl
            variant="radio"
            aria-label="Lifecycle"
            className="ex-segments"
            value={filters.filter}
            onChange={(value: CatalogFilter) => setFilter('filter', value)}
            options={catalogSegments(views).map((segment) => ({
              value: segment.value,
              title: segment.help || undefined,
              label: (
                <>
                  {segment.label}{' '}
                  <span className="ex-count">{segment.count}</span>
                </>
              ),
            }))}
          />
          <Select
            aria-label="Filter by suite"
            className="ex-sort"
            value={filters.suite}
            onChange={(event) => setFilter('suite', event.target.value)}
          >
            <option value="all">All suites</option>
            {suiteOptions.map((suite) => (
              <option key={suite.id} value={suite.id}>
                {suite.label}
              </option>
            ))}
            {filters.suite !== 'all' &&
            !suiteOptions.some((suite) => suite.id === filters.suite) ? (
              <option value={filters.suite}>{filters.suite}</option>
            ) : null}
          </Select>
          <Select
            aria-label="Sort tests"
            className="tc-sort"
            value={filters.sort}
            onChange={(event) =>
              setFilter('sort', event.target.value as CatalogSort)
            }
          >
            {CATALOG_SORTS.map((sort) => (
              <option key={sort.value} value={sort.value}>
                {sort.label}
              </option>
            ))}
          </Select>
        </section>
      )}

      {filters.filter === 'changed' ? (
        <Callout tone="warning" icon={<Info size={16} />}>
          These tests ran, but their definition changed since. Their last
          results are shown faded: they belong to an older definition and don’t
          count as current. Run them to get a current result.
        </Callout>
      ) : null}

      {/* One live region, always in the page, so what saving the suite
          did (and what it changed) is read out as it appears. */}
      <div className="tc-saved" aria-live="polite">
        {saved ? (
          <div className="ex-flash">
            <Check size={16} aria-hidden="true" />
            <span>
              Saved “{saved.label}” with {plural(saved.count, 'test')}.{' '}
              <a href={hashForSuites(saved.id)}>Open it in Suites</a>
            </span>
            <button
              className="ex-icon-button"
              type="button"
              aria-label="Dismiss"
              onClick={() => setSaved(null)}
            >
              <X size={16} aria-hidden="true" />
            </button>
          </div>
        ) : null}
        {saved?.warnings.length ? (
          <Callout tone="warning" role="note" title="What the suite changed">
            <ul className="tc-warnings">
              {saved.warnings.map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          </Callout>
        ) : null}
      </div>

      {ticked.length > 0 ? (
        <div
          className="ex-selection tc-selection shadow-floating"
          role="toolbar"
          aria-label="Selected tests"
          data-selection-bar
        >
          <span className="ex-selection-count">{bar.text}</span>
          <span className="ex-selection-hint">{bar.hint}</span>
          <Button
            type="button"
            variant="pill"
            size="sm"
            disabled={!bridge}
            onClick={() => setNaming(true)}
          >
            Save as suite…
          </Button>
          {/* The page's ink primary: the host's primary Button loses its
              text colour to the extension's scoped utilities in the pane. */}
          <button
            type="button"
            className={dashboardHeaderActionClassName({ primary: true })}
            disabled={!bridge}
            onClick={() => openRunner(ticked)}
          >
            {bar.run}
          </button>
          <Button
            type="button"
            variant="icon"
            size="icon"
            aria-label="Clear selection"
            onClick={() => setSelected([])}
          >
            <X aria-hidden="true" />
          </Button>
        </div>
      ) : null}

      {loading && rows.length === 0 ? (
        <div className="ex-loading" aria-busy="true" role="status">
          <span className="ds-visually-hidden">Loading the test catalog</span>
          {Array.from({ length: 8 }, (_, index) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
            <div key={index} />
          ))}
        </div>
      ) : failedFirstLoad ? (
        <EmptyState
          tone="error"
          title="The test catalog could not be loaded"
          description={error}
          actions={
            <button
              className={buttonClassName({ variant: 'secondary' })}
              type="button"
              onClick={() => void load()}
            >
              try again
            </button>
          }
        />
      ) : shown.length === 0 ? (
        <EmptyState
          title={
            views.length === 0
              ? 'No tests are registered yet'
              : 'No test matches.'
          }
          description={
            views.length === 0
              ? 'Register a test in the Harness catalog to start collecting evidence.'
              : undefined
          }
          actions={
            filtered ? (
              <button
                className={buttonClassName({ variant: 'secondary' })}
                type="button"
                onClick={() => setFilters(CATALOG_DEFAULT_FILTERS)}
              >
                Clear filters
              </button>
            ) : null
          }
        />
      ) : (
        <CatalogTable
          all={views}
          shown={shown}
          selected={ticked}
          onSelect={setSelected}
          narrow={narrow}
          highlightId={highlightId}
        />
      )}

      <SaveSuiteDialog
        tests={ticked}
        open={naming}
        onClose={() => setNaming(false)}
        onSave={saveSuite}
      />
      <LocalRunnerDialog
        bridge={bridge}
        key={opening}
        open={runner !== null}
        initialScenarios={runner ?? undefined}
        onClose={() => setRunner(null)}
      />
    </div>
  )
}
