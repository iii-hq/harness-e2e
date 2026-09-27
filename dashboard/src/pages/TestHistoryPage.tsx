import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  SegmentedControl,
  Select,
} from '@iii-dev/console-ui'
import { Ellipsis, GitCompare, Info } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { useDashboardChrome } from '@/components/DashboardShell'
import { RunsTable, runKey } from '@/components/history/RunsTable'
import { TestContract } from '@/components/history/TestContract'
import { LossesPanel, TrendChart } from '@/components/history/TrendChart'
import {
  buttonClassName,
  EmptyState,
  FactChip,
  FactList,
  PageHeader,
} from '@/design-system'
import {
  hashForTestHistory,
  hashForTests,
  hashForVersionComparison,
  hashForWorkspace,
  replaceRouteParams,
  routeParams,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import { getDashboardDataBridge } from '@/lib/dashboard-data-source'
import { definitionTitle, shortDefinition } from '@/lib/definition-digest'
import { plural } from '@/lib/format'
import { requestQuickExecution } from '@/lib/quick-execution'
import type { TestCatalogRow } from '@/lib/test-catalog'
import {
  ALL_DEFINITIONS,
  type ChartMetric,
  definitionChoices,
  type HistoryObservation,
  type HistoryResponse,
  losses,
  profileText,
  type Result,
  resultOf,
  staleNotice,
  summaryFigures,
  toggleSelection,
} from '@/lib/test-history'
import '@/design-system/styles.css'
import './test-history.css'

/* ---------------------------------------------------------------- state */

export type HistoryFilters = {
  /** A definition digest, or every definition. */
  definition: string
  model: string
  system: string
  profile: string
  result: string
}

/** Filters and the A/B ticks live in the hash, so a link reopens the page
 *  as it was. */
export function historyStateFromParams(params: URLSearchParams) {
  return {
    filters: {
      definition: params.get('definition') || ALL_DEFINITIONS,
      model: params.get('model') ?? '',
      system: params.get('system') ?? '',
      profile: params.get('profile') ?? '',
      result: params.get('result') ?? '',
    } satisfies HistoryFilters,
    selected: ['a', 'b']
      .map((slot) => params.get(slot))
      .filter((value): value is string => Boolean(value)),
  }
}

export function historyStateToParams(
  filters: HistoryFilters,
  selected: string[],
) {
  const params = new URLSearchParams()
  if (filters.definition !== ALL_DEFINITIONS)
    params.set('definition', filters.definition)
  for (const key of ['model', 'system', 'profile', 'result'] as const)
    if (filters[key]) params.set(key, filters[key])
  if (selected[0]) params.set('a', selected[0])
  if (selected[1]) params.set('b', selected[1])
  return params
}

/** The runs the list shows: the result and profile filters apply here, so
 *  the summary and the chart keep every run of the definition. */
export function listedRuns(
  observations: HistoryObservation[],
  filters: Pick<HistoryFilters, 'result' | 'profile'>,
) {
  return observations.filter(
    (item) =>
      (!filters.result ||
        filters.result === 'all' ||
        resultOf(item) === filters.result) &&
      (!filters.profile || profileText(item) === filters.profile),
  )
}

export function resultChoices(observations: HistoryObservation[]) {
  const counts = { all: observations.length, full: 0, lost: 0, none: 0 }
  for (const item of observations) counts[resultOf(item)] += 1
  return (
    [
      ['all', 'All'],
      ['full', 'Full marks'],
      ['lost', 'Lost points'],
      ['none', 'No score'],
    ] as Array<[Result, string]>
  )
    .filter(([id]) => id === 'all' || counts[id] > 0)
    .map(([id, label]) => ({ id, label, count: counts[id] }))
}

export function selectionHint(selected: string[]) {
  if (selected.length === 0) return 'Tick two runs to compare them'
  return selected.length === 1 ? 'A ticked · tick B' : 'A and B ticked'
}

/** `{provider, model}` travels as one select value. */
function parseModel(value: string) {
  const [provider, model] = value.split('\n')
  return provider && model ? { provider, model } : null
}

/* --------------------------------------------------------------- header */

function MoreMenu({
  onCopyLink,
  neighbours,
  definition,
}: {
  onCopyLink: () => void
  neighbours: { previous: string | null; next: string | null }
  definition: string | null
}) {
  const go = (hash: string) => () => {
    window.location.hash = hash
  }
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="ds-button ds-button-quiet ds-button-default th-more"
          aria-label="More actions"
        >
          <Ellipsis size={16} aria-hidden="true" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onSelect={onCopyLink}>Copy link</DropdownMenuItem>
        {definition ? (
          <DropdownMenuItem
            onSelect={() => void navigator.clipboard?.writeText(definition)}
          >
            Copy the current definition digest
          </DropdownMenuItem>
        ) : null}
        <DropdownMenuItem onSelect={go(hashForVersionComparison())}>
          Compare systems
        </DropdownMenuItem>
        {neighbours.previous || neighbours.next ? (
          <DropdownMenuSeparator />
        ) : null}
        {neighbours.previous ? (
          <DropdownMenuItem
            onSelect={go(hashForTestHistory(neighbours.previous))}
          >
            Previous test · {neighbours.previous}
          </DropdownMenuItem>
        ) : null}
        {neighbours.next ? (
          <DropdownMenuItem onSelect={go(hashForTestHistory(neighbours.next))}>
            Next test · {neighbours.next}
          </DropdownMenuItem>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** The header's chips: the current definition, the runs and the contract. */
export function historyFacts(
  history: HistoryResponse | null,
  row: TestCatalogRow | null,
) {
  const current = history?.current_version ?? row?.current_version ?? null
  const choices = history ? definitionChoices(history) : []
  const runs = choices.reduce((total, choice) => total + choice.runs, 0)
  const ran = choices.filter((choice) => choice.runs > 0).length
  const criteria = row?.spec?.criteria ?? []
  return [
    current
      ? {
          label: 'Definition',
          value: `${shortDefinition(current)} · current`,
          full: current,
        }
      : null,
    history
      ? {
          label: 'Runs',
          value: `${runs} in ${plural(ran, 'definition')}`,
        }
      : null,
    criteria.length > 0
      ? {
          label: 'Scored on',
          value: `${plural(criteria.length, 'criterion', 'criteria')} · ${criteria.reduce((total, item) => total + item.weight, 0)} points`,
        }
      : null,
  ].filter((fact) => fact !== null)
}

/* ----------------------------------------------------------------- page */

export function TestHistoryPage({ testId }: { testId: string }) {
  const initial = useMemo(
    () =>
      historyStateFromParams(
        typeof window === 'undefined'
          ? new URLSearchParams()
          : routeParams(window.location.hash),
      ),
    [],
  )
  const [filters, setFilters] = useState<HistoryFilters>(initial.filters)
  const [selected, setSelected] = useState<string[]>(initial.selected)
  const [history, setHistory] = useState<HistoryResponse | null>(null)
  const [row, setRow] = useState<TestCatalogRow | null>(null)
  const [neighbours, setNeighbours] = useState<{
    previous: string | null
    next: string | null
  }>({ previous: null, next: null })
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [metric, setMetric] = useState<ChartMetric>('score')
  const [open, setOpen] = useState<Set<string>>(new Set())
  const narrow = useDashboardChrome()?.narrow ?? false
  const beginRequest = useLatestRequest()

  useEffect(() => {
    const request = beginRequest()
    setLoading(true)
    setError(null)
    const model = parseModel(filters.model)
    void getDashboardDataBridge()
      .then((bridge) =>
        bridge.getTestHistory({
          test_id: testId,
          test_version: filters.definition,
          subject_provider: model?.provider,
          subject_model: model?.model,
          system_version_id: filters.system || undefined,
          limit: 100,
        }),
      )
      .then((data) => {
        if (request.isCurrent()) setHistory(data as HistoryResponse)
      })
      .catch((cause) => {
        if (request.isCurrent())
          setError(cause instanceof Error ? cause.message : String(cause))
      })
      .finally(() => {
        if (request.isCurrent()) setLoading(false)
      })
  }, [beginRequest, filters.definition, filters.model, filters.system, testId])

  // The contract and the previous and next tests come from the catalog.
  useEffect(() => {
    let cancelled = false
    void getDashboardDataBridge()
      .then((bridge) => bridge.listTests({ limit: 100 }))
      .then((list) => {
        if (cancelled) return
        const ids = list.rows.map((item) => item.test_id)
        const index = ids.indexOf(testId)
        setRow(list.rows[index] ?? null)
        setNeighbours({
          previous: index > 0 ? ids[index - 1] : null,
          next: index >= 0 && index < ids.length - 1 ? ids[index + 1] : null,
        })
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
    }
  }, [testId])

  useEffect(() => {
    replaceRouteParams(historyStateToParams(filters, selected))
  }, [filters, selected])

  // A tick on a run the filters no longer load is dropped.
  useEffect(() => {
    if (!history) return
    const loaded = new Set(history.observations.map(runKey))
    setSelected((keys) =>
      keys.every((key) => loaded.has(key))
        ? keys
        : keys.filter((key) => loaded.has(key)),
    )
  }, [history])

  const setFilter = <K extends keyof HistoryFilters>(
    key: K,
    value: HistoryFilters[K],
  ) => setFilters((current) => ({ ...current, [key]: value }))

  const spec = row?.spec ?? null
  const observations = history?.observations ?? []
  const listed = listedRuns(observations, filters)
  const profiles = [...new Set(observations.map(profileText))].sort()
  const current = history?.current_version ?? row?.current_version ?? null
  const toggleOpen = (key: string) =>
    setOpen((keys) => {
      const next = new Set(keys)
      if (!next.delete(key)) next.add(key)
      return next
    })
  const choices = history ? definitionChoices(history) : []
  const stale = history ? staleNotice(history) : null
  const total = choices.reduce((sum, choice) => sum + choice.runs, 0)
  const runThisTest = () => requestQuickExecution([testId])

  const copyLink = () => {
    void navigator.clipboard?.writeText(window.location.href).then(() => {
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1500)
    })
  }

  return (
    <div className="ds-root page-shell th-page">
      <DashboardPageActions active="tests" context={testId} />
      <PageHeader
        variant="detail"
        mono
        className="th-header"
        back={{
          label: 'Back to Tests',
          href: hashForTests(new URLSearchParams({ highlight: testId })),
        }}
        title={testId}
        headingId="test-history-title"
        summary={
          spec?.summary ??
          (loading && !history
            ? 'Loading the history…'
            : 'Every run of this test, newest first.')
        }
        actions={
          <>
            {copied ? (
              <span className="th-faint" role="status">
                Link copied
              </span>
            ) : null}
            <button
              type="button"
              className={buttonClassName({ variant: 'secondary' })}
              disabled={selected.length !== 2}
              title={
                selected.length === 2
                  ? undefined
                  : 'Tick two runs in the list to compare them'
              }
              data-compare-runs
            >
              <GitCompare size={16} aria-hidden="true" />
              {selected.length === 2 ? 'Compare A and B' : 'Compare two runs'}
            </button>
            <a
              className={buttonClassName({
                variant: 'primary',
                className: 'no-underline',
              })}
              href={hashForWorkspace()}
              onClick={runThisTest}
            >
              Run this test
            </a>
            <MoreMenu
              onCopyLink={copyLink}
              neighbours={neighbours}
              definition={
                history?.current_version ?? row?.current_version ?? null
              }
            />
          </>
        }
      />
      <FactList className="th-facts" aria-label="About this test">
        {historyFacts(history, row).map((fact) => (
          <FactChip key={fact.label} {...fact} />
        ))}
      </FactList>

      {stale ? (
        <div className="th-notice" role="status" data-stale-definition>
          <Info size={16} aria-hidden="true" className="th-notice-icon" />
          <p>{stale}</p>
          <a
            className="th-act th-act-filled"
            href={hashForWorkspace()}
            onClick={runThisTest}
          >
            Run on the current definition
          </a>
        </div>
      ) : null}

      {error ? (
        <EmptyState
          tone="error"
          title="History unavailable"
          description={error}
        />
      ) : null}

      {history && choices.length > 1 ? (
        <section className="th-definitions" aria-labelledby="th-defs">
          <h2 id="th-defs" className="th-h2">
            Definition
          </h2>
          <SegmentedControl
            variant="radio"
            aria-label="Definition"
            className="th-segments"
            value={filters.definition}
            onChange={(value) => {
              setSelected([])
              setFilter('definition', value)
            }}
            options={[
              {
                value: ALL_DEFINITIONS,
                label: (
                  <>
                    All <span className="th-count">{total}</span>
                  </>
                ),
              },
              ...choices.map((choice) => ({
                value: choice.version,
                title: definitionTitle(choice.version),
                label: (
                  <>
                    <span className="th-mono">{choice.label}</span>
                    {choice.current ? (
                      <span className="th-tag">current</span>
                    ) : null}
                    <span className="th-count">{choice.runs}</span>
                  </>
                ),
              })),
            ]}
          />
          {filters.definition === ALL_DEFINITIONS ? (
            <span className="th-faint">
              Runs on different definitions answer different contracts; compare
              within one.
            </span>
          ) : null}
        </section>
      ) : null}

      {observations.length > 0 ? (
        <>
          <section className="th-kpis" aria-label="Summary">
            {summaryFigures(observations).map((figure) => (
              <div className="th-kpi" key={figure.label}>
                <span className="th-kpi-label">{figure.label}</span>
                <span className="th-kpi-value">{figure.value}</span>
                <span className="th-kpi-sub">{figure.sub}</span>
              </div>
            ))}
          </section>
          <div className="th-two">
            <TrendChart
              observations={observations}
              metric={metric}
              onMetric={setMetric}
            />
            <LossesPanel losses={losses(observations, spec)} />
          </div>
        </>
      ) : null}

      {history || loading ? (
        <section
          className="th-runs"
          aria-labelledby="th-runs"
          aria-busy={loading || undefined}
        >
          <div className="th-toolbar">
            <h2 id="th-runs" className="th-h2 th-h2-lg">
              Runs
            </h2>
            <SegmentedControl
              variant="radio"
              aria-label="Result"
              className="th-segments"
              value={filters.result || 'all'}
              onChange={(value) =>
                setFilter('result', value === 'all' ? '' : value)
              }
              options={resultChoices(observations).map((choice) => ({
                value: choice.id,
                label: (
                  <>
                    {choice.label}
                    <span className="th-count">{choice.count}</span>
                  </>
                ),
              }))}
            />
            {(history?.subject_models ?? []).length > 0 ? (
              <Select
                aria-label="Model"
                className="th-select"
                value={filters.model || undefined}
                placeholder={`All models · ${(history?.subject_models ?? []).reduce((total, group) => total + group.models.length, 0)}`}
                allowEmpty
                emptyLabel="All models"
                onClear={() => setFilter('model', '')}
                onChange={(value) => setFilter('model', value)}
                groups={(history?.subject_models ?? []).map((group) => ({
                  label: group.provider,
                  options: group.models.map((model) => ({
                    value: `${group.provider}\n${model}`,
                    label: model,
                  })),
                }))}
              />
            ) : null}
            {profiles.length > 1 || filters.profile ? (
              <Select
                aria-label="Profile"
                className="th-select"
                value={filters.profile || undefined}
                placeholder={`All profiles · ${profiles.length}`}
                allowEmpty
                emptyLabel="All profiles"
                onClear={() => setFilter('profile', '')}
                onChange={(value) => setFilter('profile', value)}
                options={profiles.map((profile) => ({
                  value: profile,
                  label: profile,
                }))}
              />
            ) : null}
            {(history?.systems ?? []).length > 1 || filters.system ? (
              <Select
                aria-label="System revision"
                className="th-select"
                value={filters.system || undefined}
                placeholder="All system revisions"
                allowEmpty
                emptyLabel="All system revisions"
                onClear={() => setFilter('system', '')}
                onChange={(value) => setFilter('system', value)}
                options={(history?.systems ?? []).map((system) => ({
                  value: system.id,
                  label: system.label,
                }))}
              />
            ) : null}
            <span className="th-faint th-push" aria-live="polite">
              {selectionHint(selected)}
            </span>
          </div>
          {loading && !history ? (
            <div className="th-skeleton" role="status">
              <span className="ds-visually-hidden">Loading the runs</span>
              {[0, 1, 2, 3].map((index) => (
                <div key={index} />
              ))}
            </div>
          ) : listed.length > 0 ? (
            <RunsTable
              observations={listed}
              current={current}
              grouped={
                new Set(listed.map((item) => item.behavior_sha256)).size > 1
              }
              selected={selected}
              onToggleSelected={(key) =>
                setSelected((keys) => toggleSelection(keys, key))
              }
              open={open}
              onToggleOpen={toggleOpen}
              narrow={narrow}
            />
          ) : observations.length > 0 ? (
            <div className="th-empty">
              <p className="th-faint">No run matches these filters.</p>
              <button
                type="button"
                className="th-act th-act-filled"
                onClick={() =>
                  setFilters((value) => ({ ...value, result: '', profile: '' }))
                }
              >
                Clear filters
              </button>
            </div>
          ) : (
            <p className="th-empty th-faint">
              No run on this definition yet. Run the test to get its first
              result.
            </p>
          )}
          {history && history.total > observations.length ? (
            <p className="th-note">
              Showing the latest {observations.length} of {history.total} runs.
            </p>
          ) : null}
        </section>
      ) : null}

      {spec ? <TestContract spec={spec} /> : null}
    </div>
  )
}
