import {
  ArrowLeftRight,
  ArrowRight,
  ChevronDown,
  Link2,
  RefreshCw,
  Search,
  X,
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
} from '@/components/DashboardPageActions'
import { requestQuickExecution } from '@/components/ExecutionSetup'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import {
  buttonClassName,
  Callout,
  DataTable,
  DeltaValue,
  EmptyState,
  FilterChip,
  FilterChipGroup,
  Input,
  numericCellClassName,
  type OperationalStatus,
  PageHeader,
  Panel,
  Select,
  StatusBadge,
} from '@/design-system'
import {
  hashForComparison,
  hashForExecution,
  hashForTestHistory,
  hashForTests,
  hashForWorkspace,
  replaceDashboardHash,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import type {
  CohortDescriptor,
  EvaluatedVersion,
  EvaluatedVersionsResponse,
  TestCatalogRow,
  TestObservation,
  TestSideSummary,
  TestsListResponse,
  TestVersionResult,
} from '@/lib/test-catalog'
import {
  comparisonUtility,
  comparisonWarnings,
  hasRetainedEvidence,
  isMoreUsefulComparison,
} from '@/lib/test-catalog-view'

/* ---------------------------------------------------------------- format */

function formatNumber(value: number | null | undefined, digits = 1) {
  if (value === null || value === undefined || !Number.isFinite(value))
    return '—'
  return new Intl.NumberFormat('en-US', {
    maximumFractionDigits: digits,
  }).format(value)
}

function formatCurrency(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value))
    return '—'
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    maximumFractionDigits: value < 1 ? 3 : 2,
  }).format(value)
}

function formatDuration(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value))
    return '—'
  if (value < 60) return `${formatNumber(value)}s`
  return `${Math.floor(value / 60)}m ${String(Math.round(value % 60)).padStart(2, '0')}s`
}

function formatTokens(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value))
    return '—'
  if (Math.abs(value) >= 1000)
    return `${(value / 1000).toFixed(1).replace(/\.0$/, '')}k`
  return Math.round(value).toLocaleString()
}

function formatDay(value: string) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
  }).format(date)
}

function formatDate(value: string) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date)
}

function shortId(value: string) {
  return value.replace(/^sha256:/, '').slice(0, 8)
}

function compactModel(model: string) {
  return model.split('/').at(-1) || model
}

/* ----------------------------------------------------------------- sides */

export function summaryStatus(summary: TestSideSummary | null): {
  status: OperationalStatus
  label: string
} {
  if (!summary) return { status: 'unavailable', label: 'no evidence' }
  if (summary.outcomes.infra_failed > 0)
    return { status: 'failed', label: 'infrastructure failure' }
  if (summary.outcomes.technical_failed > 0)
    return { status: 'failed', label: 'technical failure' }
  if (summary.outcomes.hard_gate_failed > 0)
    return { status: 'hard_gate', label: 'hard gate failed' }
  return { status: 'passed', label: 'passed' }
}

/** Audit CP-04: the AI verdict is one short phrase, only when it exists. */
export function aiVerdictLabel(summary: TestSideSummary) {
  const verdicts = summary.assessment_summary?.ai_verdicts
  if (!verdicts) return null
  if (verdicts.fail > 0) return 'ai: fail'
  if (verdicts.pass_with_concerns > 0)
    return `ai: ${verdicts.pass_with_concerns} concern${verdicts.pass_with_concerns === 1 ? '' : 's'}`
  if (verdicts.pass > 0) return 'ai: no concerns'
  if (verdicts.inconclusive > 0) return 'ai: inconclusive'
  return null
}

/** One line per side: status · score/100 · ai note (audit CP-04 / CP-16). */
export function SideResult({ summary }: { summary: TestSideSummary | null }) {
  const status = summaryStatus(summary)
  if (!summary) {
    return <span className="font-mono text-xs text-ink-muted">no evidence</span>
  }
  const ai = aiVerdictLabel(summary)
  return (
    <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
      <StatusBadge status={status.status} label={status.label} />
      <span className="font-mono text-xs text-ink">
        {formatNumber(summary.median_score, 0)}
        <span className="text-ink-muted"> / 100</span>
      </span>
      <span
        className="font-mono text-label text-ink-muted"
        title={`${summary.scored_runs} scored of ${summary.total_runs} runs`}
      >
        n={summary.scored_runs}
      </span>
      {ai ? (
        <span className="font-mono text-label text-ink-soft">{ai}</span>
      ) : null}
    </span>
  )
}

/* ----------------------------------------------------------------- state */

export type RowState =
  | 'regressed'
  | 'improved'
  | 'unchanged'
  | 'changed'
  | 'one_side'
  | 'none'

function hasIssuesInB(result: TestVersionResult) {
  const to = result.to
  if (!to) return false
  return (
    to.outcomes.hard_gate_failed +
      to.outcomes.technical_failed +
      to.outcomes.infra_failed >
    0
  )
}

/** Audit CP-01: every row has exactly one state that decides its group. */
export function rowState(row: TestCatalogRow): RowState {
  const result = row.result
  if (!result || (!result.from && !result.to)) return 'none'
  if (!result.from || !result.to) return 'one_side'
  if (result.compatibility !== 'compatible') return 'changed'
  const score = result.delta.score
  const fromIssues =
    result.from.outcomes.hard_gate_failed +
      result.from.outcomes.technical_failed +
      result.from.outcomes.infra_failed >
    0
  const toIssues = hasIssuesInB(result)
  if ((toIssues && !fromIssues) || (score !== null && score < 0))
    return 'regressed'
  if ((fromIssues && !toIssues) || (score !== null && score > 0))
    return 'improved'
  return 'unchanged'
}

export type CompareFilter =
  | 'evidence'
  | 'comparable'
  | 'regressed'
  | 'improved'
  | 'one_side'
  | 'all'

const STATE_ORDER: RowState[] = [
  'regressed',
  'improved',
  'unchanged',
  'changed',
  'one_side',
  'none',
]

export function matchesCompareFilter(state: RowState, filter: CompareFilter) {
  if (filter === 'all') return true
  if (filter === 'evidence') return state !== 'none'
  if (filter === 'comparable')
    return (
      state === 'regressed' || state === 'improved' || state === 'unchanged'
    )
  return state === filter
}

export function sortCompareRows(rows: TestCatalogRow[]) {
  return [...rows].sort(
    (left, right) =>
      STATE_ORDER.indexOf(rowState(left)) -
        STATE_ORDER.indexOf(rowState(right)) ||
      left.test_id.localeCompare(right.test_id),
  )
}

function compatibilityLabel(result: TestVersionResult | null) {
  if (!result) return 'no comparison'
  return {
    compatible: 'comparable',
    missing_side: 'evidence on one side',
    contract_changed: 'cases or contract changed',
    contract_conflict: 'contract conflict',
    assessment_changed: 'assessment profile changed',
    assessment_conflict: 'assessment profile conflict',
    analyzer_changed: 'analyzer profile changed',
    analyzer_conflict: 'analyzer profile conflict',
  }[result.compatibility]
}

function negateDelta(result: TestVersionResult | null) {
  if (!result) return null
  const negate = (value: number | null) => (value === null ? null : -value)
  return {
    ...result,
    from: result.to,
    to: result.from,
    from_observations: result.to_observations,
    to_observations: result.from_observations,
    delta: {
      score: negate(result.delta.score),
      cost_usd: negate(result.delta.cost_usd),
      tokens: negate(result.delta.tokens),
      duration_seconds: negate(result.delta.duration_seconds),
    },
  }
}

function relativeTokenDelta(result: TestVersionResult | null) {
  const from = result?.from?.median_tokens
  const to = result?.to?.median_tokens
  if (from == null || to == null || from === 0) return null
  return ((to - from) / from) * 100
}

/* --------------------------------------------------------------- details */

function EvidenceRow({
  side,
  observation,
  testId,
}: {
  side: 'a' | 'b'
  observation: TestObservation
  testId: string
}) {
  const status = summaryStatusFromObservation(observation.status)
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1 py-1.5 font-mono text-xs">
      <span className="w-4 text-ink-muted">{side}</span>
      <span className="text-ink">{formatDate(observation.completed_at)}</span>
      {observation.subject_model ? (
        <span className="text-ink-soft">
          {compactModel(observation.subject_model)}
        </span>
      ) : null}
      <StatusBadge status={status.status} label={status.label} />
      <span className="text-ink">
        {formatNumber(observation.median_score, 0)}
        <span className="text-ink-muted">
          {' '}
          / 100 · n={observation.scored_runs}
        </span>
      </span>
      <span
        className="text-label text-ink-muted"
        title={observation.execution_id}
      >
        {shortId(observation.execution_id)}…
      </span>
      <span className="ms-auto inline-flex items-center gap-1">
        <a
          className={buttonClassName({
            variant: 'quiet',
            size: 'compact',
            className: 'no-underline',
          })}
          href={hashForExecution(observation.execution_id)}
        >
          open
          <ArrowRight size={13} aria-hidden="true" />
        </a>
        <ScenarioChatAction
          compact
          executionId={observation.execution_id}
          scenarioId={testId}
        />
      </span>
    </li>
  )
}

function summaryStatusFromObservation(status: string): {
  status: OperationalStatus
  label: string
} {
  if (status === 'passed') return { status: 'passed', label: 'passed' }
  if (status === 'hard_gate_failed')
    return { status: 'hard_gate', label: 'hard gate failed' }
  return { status: 'failed', label: status.replace(/[_-]+/g, ' ') }
}

/** Audit CP-19: tiles only with data on at least one side; evidence as a list. */
export function RowDetails({
  result,
  aLabel,
  bLabel,
}: {
  result: TestVersionResult | null
  aLabel: string
  bLabel: string
}) {
  if (!result) {
    return (
      <p className="m-0 text-xs text-ink-soft">
        Choose two system versions to inspect evidence.
      </p>
    )
  }
  const warnings = comparisonWarnings(result)
  const pair = (
    label: string,
    from: string,
    to: string,
    delta?: {
      value: number | null
      betterWhen: 'higher' | 'lower'
      format: (m: number) => string
    },
  ) => (from === '—' && to === '—' ? null : { label, from, to, delta })
  const tiles = [
    pair(
      'pass rate',
      result.from?.pass_rate == null
        ? '—'
        : `${formatNumber(result.from.pass_rate * 100, 0)}%`,
      result.to?.pass_rate == null
        ? '—'
        : `${formatNumber(result.to.pass_rate * 100, 0)}%`,
    ),
    pair(
      'median cost',
      formatCurrency(result.from?.median_cost_usd),
      formatCurrency(result.to?.median_cost_usd),
      {
        value: result.delta.cost_usd,
        betterWhen: 'lower',
        format: (m) => formatCurrency(m),
      },
    ),
    pair(
      'median runtime',
      formatDuration(result.from?.median_duration_seconds),
      formatDuration(result.to?.median_duration_seconds),
      {
        value: result.delta.duration_seconds,
        betterWhen: 'lower',
        format: (m) => formatDuration(m),
      },
    ),
    pair(
      'median tokens',
      formatTokens(result.from?.median_tokens),
      formatTokens(result.to?.median_tokens),
      {
        value: result.delta.tokens,
        betterWhen: 'lower',
        format: (m) => formatTokens(m),
      },
    ),
  ].filter((tile): tile is NonNullable<typeof tile> => tile !== null)
  const observations = [
    ...result.from_observations.map((item) => ({ item, side: 'a' as const })),
    ...result.to_observations.map((item) => ({ item, side: 'b' as const })),
  ]
  return (
    <div className="grid gap-4" data-compare-details>
      {warnings.length > 0 ? (
        <Callout
          tone="warning"
          title="not comparable · values shown, deltas not interpreted"
        >
          {warnings
            .map((warning) => `${warning.title}: ${warning.detail}`)
            .join(' ')}
        </Callout>
      ) : null}
      {tiles.length > 0 ? (
        <div className="grid gap-2 @[560px]:grid-cols-2 @[840px]:grid-cols-4">
          {tiles.map((tile) => (
            <article
              key={tile.label}
              className="grid gap-1 rounded-[6px] bg-[var(--surface-fill)] p-3"
            >
              <span className="ds-label">{tile.label}</span>
              <span className="flex items-center gap-2 font-mono text-sm text-ink">
                <span title={aLabel}>{tile.from}</span>
                <ArrowRight
                  className="text-ink-muted"
                  size={13}
                  aria-hidden="true"
                />
                <span title={bLabel}>{tile.to}</span>
              </span>
              {tile.delta && result.compatibility === 'compatible' ? (
                <DeltaValue
                  className="text-label"
                  value={tile.delta.value}
                  format={tile.delta.format}
                  betterWhen={tile.delta.betterWhen}
                  unavailableLabel="—"
                  title="b − a"
                />
              ) : null}
            </article>
          ))}
        </div>
      ) : null}
      <div className="grid gap-1">
        <span className="ds-label">retained evidence</span>
        {observations.length === 0 ? (
          <span className="text-xs text-ink-soft">
            No retained observations.
          </span>
        ) : (
          <ul className="m-0 grid list-none divide-y divide-line p-0">
            {observations.map(({ item, side }) => (
              <EvidenceRow
                key={`${side}-${item.execution_id}-${item.case_id}`}
                side={side}
                observation={item}
                testId={result.test_id}
              />
            ))}
          </ul>
        )}
      </div>
      <a
        className={buttonClassName({
          variant: 'quiet',
          size: 'compact',
          className: 'justify-self-start no-underline',
        })}
        href={hashForTestHistory(result.test_id)}
      >
        open history
        <ArrowRight size={13} aria-hidden="true" />
      </a>
    </div>
  )
}

/* ------------------------------------------------------------------ rows */

function TestVersionSelect({
  row,
  disabled,
  onVersion,
}: {
  row: TestCatalogRow
  disabled: boolean
  onVersion: (row: TestCatalogRow, version: number) => void
}) {
  if (row.available_versions.length <= 1) {
    return (
      <span className="font-mono text-label text-ink-muted">
        v{row.selected_version ?? row.current_version ?? '—'}
      </span>
    )
  }
  return (
    <Select
      className="max-w-[9rem] text-label"
      aria-label={`Test version for ${row.test_id}`}
      value={row.selected_version ?? ''}
      disabled={disabled}
      onChange={(event) => onVersion(row, Number(event.target.value))}
    >
      {row.available_versions.map((version) => (
        <option key={version.version} value={version.version}>
          v{version.version} · {version.run_count} runs
        </option>
      ))}
    </Select>
  )
}

function CompareRow({
  row,
  aLabel,
  bLabel,
  local,
  expanded,
  loading,
  error,
  onVersion,
  onToggle,
}: {
  row: TestCatalogRow
  aLabel: string
  bLabel: string
  local: boolean
  expanded: boolean
  loading: boolean
  error?: string
  onVersion: (row: TestCatalogRow, version: number) => void
  onToggle: (row: TestCatalogRow) => void
}) {
  const state = rowState(row)
  const result = row.result
  const evidenceAvailable = hasRetainedEvidence(row)
  const comparable = result?.compatibility === 'compatible'
  const detailsId = `compare-details-${row.test_id}`
  const runsEachSide =
    result?.from && result?.to
      ? result.from.total_runs === result.to.total_runs
        ? `${result.from.total_runs} run${result.from.total_runs === 1 ? '' : 's'} each side`
        : `${result.from.total_runs} vs ${result.to.total_runs} runs`
      : null
  return (
    <>
      <tr data-row-state={state} data-test-id={row.test_id}>
        <td data-label="Test">
          <span className="flex flex-wrap items-center gap-2">
            <a
              className="font-mono text-xs font-medium text-ink no-underline hover:underline"
              href={hashForTestHistory(row.test_id)}
            >
              {row.test_id}
            </a>
            <TestVersionSelect
              row={row}
              disabled={loading}
              onVersion={onVersion}
            />
          </span>
          {runsEachSide ? (
            <span className="block font-mono text-label text-ink-muted">
              {runsEachSide}
            </span>
          ) : null}
        </td>
        <td data-label={`a · ${aLabel}`}>
          <SideResult summary={result?.from ?? null} />
        </td>
        <td data-label={`b · ${bLabel}`}>
          <SideResult summary={result?.to ?? null} />
        </td>
        <td data-label="Δ score" className={numericCellClassName}>
          {comparable ? (
            <DeltaValue
              value={result?.delta.score}
              format={(m) => m.toFixed(0)}
              betterWhen="higher"
              unavailableLabel="—"
              title="b − a"
            />
          ) : (
            <span className="text-ink-muted" title={compatibilityLabel(result)}>
              —
            </span>
          )}
        </td>
        <td data-label="Δ tokens" className={numericCellClassName}>
          {comparable ? (
            <DeltaValue
              value={relativeTokenDelta(result)}
              format={(m) => `${m.toFixed(m >= 10 ? 0 : 1)}%`}
              betterWhen="lower"
              unavailableLabel="—"
              title="b − a, relative to a"
            />
          ) : (
            <span className="text-ink-muted" title={compatibilityLabel(result)}>
              —
            </span>
          )}
        </td>
        <td data-label="Actions" className="text-right">
          <span className="inline-flex flex-wrap items-center justify-end gap-1">
            {state === 'one_side' && local ? (
              <a
                className={buttonClassName({
                  variant: 'quiet',
                  size: 'compact',
                  className: 'no-underline',
                })}
                href={hashForWorkspace()}
                onClick={() => requestQuickExecution([row.test_id])}
              >
                run on {result?.from ? 'b' : 'a'}
                <ArrowRight size={13} aria-hidden="true" />
              </a>
            ) : null}
            {evidenceAvailable ? (
              <button
                className={buttonClassName({
                  variant: 'quiet',
                  size: 'compact',
                })}
                type="button"
                aria-expanded={expanded}
                aria-controls={detailsId}
                disabled={loading}
                onClick={() => onToggle(row)}
              >
                {loading ? 'loading…' : expanded ? 'hide' : 'inspect'}
                <ChevronDown
                  className={`transition-transform duration-[var(--ds-duration-fast)] ${expanded ? 'rotate-180' : ''}`}
                  size={13}
                  aria-hidden="true"
                />
              </button>
            ) : null}
            <a
              className={buttonClassName({
                variant: 'quiet',
                size: 'compact',
                className: 'no-underline',
              })}
              href={hashForTestHistory(row.test_id)}
              aria-label={`History for ${row.test_id}`}
            >
              history
              <ArrowRight size={13} aria-hidden="true" />
            </a>
          </span>
        </td>
      </tr>
      {expanded || error ? (
        <tr id={detailsId} data-compare-details-row>
          <td colSpan={6} className="bg-[var(--surface-fill)]">
            {error ? (
              <Callout tone="danger">
                <span className="flex flex-wrap items-center justify-between gap-3">
                  {error}
                  <button
                    className={buttonClassName({
                      variant: 'secondary',
                      size: 'compact',
                    })}
                    type="button"
                    onClick={() =>
                      row.selected_version &&
                      onVersion(row, row.selected_version)
                    }
                  >
                    retry
                  </button>
                </span>
              </Callout>
            ) : (
              <RowDetails result={result} aLabel={aLabel} bLabel={bLabel} />
            )}
          </td>
        </tr>
      ) : null}
    </>
  )
}

/* ------------------------------------------------------------------ page */

const GROUPS: Array<{ key: RowState[]; label: string; hint: string }> = [
  {
    key: ['regressed', 'improved', 'unchanged'],
    label: 'comparable',
    hint: 'same test version, cases and contracts',
  },
  {
    key: ['changed'],
    label: 'contract or profile changed',
    hint: 'evidence on both sides, deltas not interpreted',
  },
  {
    key: ['one_side'],
    label: 'evidence on one side',
    hint: 'run the other side to compare',
  },
]

export function TestsPage({
  initialFrom = null,
  initialTo = null,
}: {
  initialFrom?: string | null
  initialTo?: string | null
}) {
  const bridgeRef = useRef<DashboardDataBridge | null>(null)
  const versionOverrides = useRef(new Map<string, number>())
  const rowRequestCounter = useRef(0)
  const rowRequestSequences = useRef(new Map<string, number>())
  const prefetchedCatalog = useRef(new Map<string, TestsListResponse>())
  const recommendationAttempts = useRef(new Set<string>())
  const automaticSelection = useRef(!initialFrom && !initialTo)
  const evaluatedRevision = useRef('')
  const comparisonContext = useRef('')
  const selectedCohort = useRef('')
  const selectedFromVersion = useRef('')
  const selectedToVersion = useRef('')
  const [evaluated, setEvaluated] = useState<EvaluatedVersionsResponse | null>(
    null,
  )
  const [cohortId, setCohortId] = useState('')
  const [fromVersionId, setFromVersionId] = useState('')
  const [toVersionId, setToVersionId] = useState('')
  const [rows, setRows] = useState<TestCatalogRow[]>([])
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<CompareFilter>('evidence')
  const [showHidden, setShowHidden] = useState(false)
  const [loading, setLoading] = useState(true)
  const [local, setLocal] = useState(false)
  const [error, setError] = useState<Error | null>(null)
  const [rowLoading, setRowLoading] = useState<Set<string>>(new Set())
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({})
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [detailsLoaded, setDetailsLoaded] = useState<Set<string>>(new Set())
  const [recommendation, setRecommendation] = useState<{
    message: string
    previousFrom: string | null
  } | null>(null)
  const [copied, setCopied] = useState(false)
  const [reloadKey, setReloadKey] = useState(0)
  const comparisonKey = `${cohortId}:${fromVersionId}:${toVersionId}`
  comparisonContext.current = comparisonKey
  selectedCohort.current = cohortId
  selectedFromVersion.current = fromVersionId
  selectedToVersion.current = toVersionId

  useEffect(() => {
    void comparisonKey
    rowRequestSequences.current.clear()
    setRowLoading(new Set())
    setRowErrors({})
    setDetailsLoaded(new Set())
    setExpanded(new Set())
  }, [comparisonKey])

  // Audit CP-10: the pair lives in the hash, without a reload.
  useEffect(() => {
    if (!fromVersionId || !toVersionId) return
    replaceDashboardHash(hashForComparison(fromVersionId, toVersionId))
  }, [fromVersionId, toVersionId])

  const chooseVersions = useCallback(
    (
      data: EvaluatedVersionsResponse,
      requestedCohort?: string,
      requestedFromId?: string,
      requestedToId?: string,
    ) => {
      let cohort = requestedCohort || ''
      const desiredFrom = requestedFromId || initialFrom
      const desiredTo = requestedToId || initialTo
      if (!cohort && desiredFrom && desiredTo) {
        const from = data.versions.find((version) => version.id === desiredFrom)
        const to = data.versions.find((version) => version.id === desiredTo)
        if (from && from.cohort_id === to?.cohort_id) cohort = from.cohort_id
      }
      cohort ||=
        data.cohorts
          .map((candidate, index) => {
            const candidateVersions = data.versions.filter(
              (version) => version.cohort_id === candidate.id,
            )
            return {
              id: candidate.id,
              index,
              versionCount: candidateVersions.length,
              latest: candidateVersions[0]?.completed_at ?? '',
            }
          })
          .sort(
            (left, right) =>
              right.versionCount - left.versionCount ||
              right.latest.localeCompare(left.latest) ||
              left.index - right.index,
          )[0]?.id ?? ''
      const versions = data.versions.filter(
        (version) => version.cohort_id === cohort,
      )
      const requestedFrom = versions.find(
        (version) => version.id === desiredFrom,
      )?.id
      const requestedTo = versions.find(
        (version) => version.id === desiredTo,
      )?.id
      setCohortId(cohort)
      setToVersionId(requestedTo ?? versions[0]?.id ?? '')
      setFromVersionId(requestedFrom ?? versions[1]?.id ?? '')
    },
    [initialFrom, initialTo],
  )

  const loadEvaluated = useCallback(async () => {
    setError(null)
    setLoading(true)
    try {
      const bridge = await getDashboardDataBridge()
      bridgeRef.current = bridge
      setLocal(bridge.mode === 'local')
      const data = await bridge.listEvaluatedVersions()
      if (evaluatedRevision.current !== data.revision) {
        evaluatedRevision.current = data.revision
        prefetchedCatalog.current.clear()
        recommendationAttempts.current.clear()
      }
      setEvaluated(data)
      chooseVersions(
        data,
        selectedCohort.current,
        selectedFromVersion.current,
        selectedToVersion.current,
      )
    } catch (cause) {
      setError(cause instanceof Error ? cause : new Error(String(cause)))
    } finally {
      setLoading(false)
    }
  }, [chooseVersions])

  useEffect(() => {
    void reloadKey
    void loadEvaluated()
  }, [loadEvaluated, reloadKey])

  useEffect(() => {
    void evaluated?.revision
    const bridge = bridgeRef.current
    if (!bridge) return
    let cancelled = false
    let dispose: (() => void) | undefined
    bridge
      .subscribeRunChanges((payload) => {
        if (payload.kind !== 'progress') {
          setReloadKey((value) => value + 1)
        }
      })
      .then((off) => {
        if (cancelled) off()
        else dispose = off
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
      dispose?.()
    }
  }, [evaluated?.revision])

  const loadVersionResult = useCallback(
    async (testId: string, version: number, markDetails = false) => {
      const bridge = bridgeRef.current
      if (!bridge || !cohortId || !fromVersionId || !toVersionId) return null
      const requestSequence = ++rowRequestCounter.current
      rowRequestSequences.current.set(testId, requestSequence)
      const requestContext = comparisonContext.current
      setRowLoading((current) => new Set(current).add(testId))
      setRowErrors((current) => {
        const next = { ...current }
        delete next[testId]
        return next
      })
      try {
        const result = await bridge.getTestVersion({
          test_id: testId,
          test_version: version,
          cohort_id: cohortId,
          from_version_id: fromVersionId,
          to_version_id: toVersionId,
        })
        if (
          rowRequestSequences.current.get(testId) !== requestSequence ||
          requestContext !== comparisonContext.current
        ) {
          return null
        }
        setRows((current) =>
          current.map((row) =>
            row.test_id === testId
              ? { ...row, selected_version: version, result }
              : row,
          ),
        )
        if (markDetails) {
          setDetailsLoaded((current) => new Set(current).add(testId))
        }
        return result
      } catch (cause) {
        if (
          rowRequestSequences.current.get(testId) !== requestSequence ||
          requestContext !== comparisonContext.current
        ) {
          return null
        }
        setRowErrors((current) => ({
          ...current,
          [testId]: cause instanceof Error ? cause.message : String(cause),
        }))
        return null
      } finally {
        if (
          rowRequestSequences.current.get(testId) === requestSequence &&
          requestContext === comparisonContext.current
        ) {
          setRowLoading((current) => {
            const next = new Set(current)
            next.delete(testId)
            return next
          })
        }
      }
    },
    [cohortId, fromVersionId, toVersionId],
  )

  const versions = useMemo(
    () =>
      evaluated?.versions.filter((version) => version.cohort_id === cohortId) ??
      [],
    [evaluated, cohortId],
  )
  const versionById = useMemo(
    () => new Map(versions.map((version) => [version.id, version])),
    [versions],
  )

  useEffect(() => {
    void reloadKey
    const bridge = bridgeRef.current
    if (!bridge) return
    let active = true
    setLoading(true)
    setError(null)

    const listCatalog = (fromId: string, toId: string) =>
      bridge.listTests({
        limit: 100,
        cohort_id: cohortId || undefined,
        from_version_id: fromId || undefined,
        to_version_id: toId || undefined,
      })

    const applyVersionOverrides = async (response: TestsListResponse) => {
      let next = response.rows
      if (!fromVersionId || !toVersionId) return next
      next = await Promise.all(
        next.map(async (row) => {
          const override = versionOverrides.current.get(row.test_id)
          if (
            !override ||
            override === row.selected_version ||
            !row.available_versions.some((item) => item.version === override)
          ) {
            return row
          }
          const result = await bridge.getTestVersion({
            test_id: row.test_id,
            test_version: override,
            cohort_id: cohortId,
            from_version_id: fromVersionId,
            to_version_id: toVersionId,
          })
          return { ...row, selected_version: override, result }
        }),
      )
      return next
    }

    const load = async () => {
      const cached = prefetchedCatalog.current.get(comparisonKey)
      if (cached) prefetchedCatalog.current.delete(comparisonKey)
      const response = cached ?? (await listCatalog(fromVersionId, toVersionId))
      if (!active) return

      // Audit CP-12: the recommendation only ever replaces the automatic
      // default and says so with an undo; an explicit choice is never moved.
      const latestVersionId = versions[0]?.id
      const recommendationKey = `${evaluated?.revision ?? ''}:${cohortId}:${toVersionId}`
      if (
        automaticSelection.current &&
        fromVersionId &&
        toVersionId &&
        toVersionId === latestVersionId &&
        versions.length > 2 &&
        !recommendationAttempts.current.has(recommendationKey)
      ) {
        recommendationAttempts.current.add(recommendationKey)
        const alternatives = versions.filter(
          (version) =>
            version.id !== fromVersionId && version.id !== toVersionId,
        )
        const candidates = await Promise.all(
          alternatives.map(async (version) => {
            try {
              return {
                fromVersionId: version.id,
                response: await listCatalog(version.id, toVersionId),
              }
            } catch {
              return null
            }
          }),
        )
        if (!active || !automaticSelection.current) return
        let best = { fromVersionId, response }
        let bestUtility = comparisonUtility(response.rows)
        for (const candidate of candidates) {
          if (!candidate) continue
          const utility = comparisonUtility(candidate.response.rows)
          if (isMoreUsefulComparison(utility, bestUtility)) {
            best = candidate
            bestUtility = utility
          }
        }
        if (best.fromVersionId !== fromVersionId) {
          const nextKey = `${cohortId}:${best.fromVersionId}:${toVersionId}`
          prefetchedCatalog.current.set(nextKey, best.response)
          setRecommendation({
            message:
              bestUtility.comparable > 0
                ? `a moved to ${versions.find((version) => version.id === best.fromVersionId)?.label.toLowerCase() ?? shortId(best.fromVersionId)}, the closest version with ${bestUtility.comparable} comparable test${bestUtility.comparable === 1 ? '' : 's'}.`
                : `a moved to ${versions.find((version) => version.id === best.fromVersionId)?.label.toLowerCase() ?? shortId(best.fromVersionId)}, the version with the most shared evidence.`,
            previousFrom: fromVersionId,
          })
          setFromVersionId(best.fromVersionId)
          return
        }
      }

      const next = await applyVersionOverrides(response)
      if (active) setRows(next)
    }

    load()
      .catch((cause) => {
        if (active) {
          setError(cause instanceof Error ? cause : new Error(String(cause)))
        }
      })
      .finally(() => {
        if (active) setLoading(false)
      })
    return () => {
      active = false
    }
  }, [
    cohortId,
    comparisonKey,
    evaluated?.revision,
    fromVersionId,
    reloadKey,
    toVersionId,
    versions,
  ])

  const states = useMemo(
    () => new Map(rows.map((row) => [row.test_id, rowState(row)])),
    [rows],
  )
  const counts = useMemo(() => {
    const tally: Record<RowState, number> = {
      regressed: 0,
      improved: 0,
      unchanged: 0,
      changed: 0,
      one_side: 0,
      none: 0,
    }
    for (const state of states.values()) tally[state] += 1
    return tally
  }, [states])
  const comparableCount = counts.regressed + counts.improved + counts.unchanged
  const evidenceCount = rows.length - counts.none
  const normalizedQuery = query.trim().toLowerCase()
  const visibleRows = useMemo(
    () =>
      sortCompareRows(
        rows.filter(
          (row) =>
            (!normalizedQuery ||
              row.test_id.toLowerCase().includes(normalizedQuery)) &&
            matchesCompareFilter(states.get(row.test_id) ?? 'none', filter),
        ),
      ),
    [rows, normalizedQuery, states, filter],
  )
  const hiddenRows = rows.filter(
    (row) =>
      (states.get(row.test_id) ?? 'none') === 'none' &&
      (!normalizedQuery || row.test_id.toLowerCase().includes(normalizedQuery)),
  )

  const updateCohort = (nextCohort: string) => {
    automaticSelection.current = true
    setRecommendation(null)
    setCohortId(nextCohort)
    const nextVersions = evaluated?.versions.filter(
      (version) => version.cohort_id === nextCohort,
    )
    setToVersionId(nextVersions?.[0]?.id ?? '')
    setFromVersionId(nextVersions?.[1]?.id ?? '')
    versionOverrides.current.clear()
    setDetailsLoaded(new Set())
  }

  const swapVersions = () => {
    if (!fromVersionId || !toVersionId) return
    automaticSelection.current = false
    setRecommendation(null)
    setFromVersionId(toVersionId)
    setToVersionId(fromVersionId)
    setRows((current) =>
      current.map((row) => ({ ...row, result: negateDelta(row.result) })),
    )
  }

  const selectTestVersion = (row: TestCatalogRow, version: number) => {
    versionOverrides.current.set(row.test_id, version)
    setDetailsLoaded((current) => {
      const next = new Set(current)
      next.delete(row.test_id)
      return next
    })
    void loadVersionResult(row.test_id, version, true)
  }

  const toggleDetails = (row: TestCatalogRow) => {
    if (!hasRetainedEvidence(row)) return
    const opening = !expanded.has(row.test_id)
    setExpanded((current) => {
      const next = new Set(current)
      if (opening) next.add(row.test_id)
      else next.delete(row.test_id)
      return next
    })
    if (opening && !detailsLoaded.has(row.test_id) && row.selected_version) {
      void loadVersionResult(row.test_id, row.selected_version, true)
    }
  }

  const selectFromVersion = (versionId: string) => {
    automaticSelection.current = false
    setRecommendation(null)
    setFromVersionId(versionId)
  }

  const selectToVersion = (versionId: string) => {
    automaticSelection.current = false
    setRecommendation(null)
    setToVersionId(versionId)
  }

  const cohortLabel = (cohort: CohortDescriptor) => {
    const judge = cohort.judge_model
      ? `judge ${compactModel(cohort.judge_model)}`
      : 'automatic judge'
    return `${compactModel(cohort.subject_model)} · lane ${cohort.lane} · ${judge}`
  }
  const cohortsWithPairs = (evaluated?.cohorts ?? []).filter(
    (cohort) =>
      (evaluated?.versions.filter((version) => version.cohort_id === cohort.id)
        .length ?? 0) >= 2,
  ).length

  // Audit CP-02: a version reads as date · id · executions, never a bare hash.
  const versionOptionLabel = (version: EvaluatedVersion) =>
    `${formatDay(version.completed_at)} · ${version.label.toLowerCase()} · ${version.execution_count} execution${version.execution_count === 1 ? '' : 's'}`
  const sideSummary = (side: 'from' | 'to') => {
    const withEvidence = rows.filter((row) => row.result?.[side]).length
    const shared = rows.filter(
      (row) => row.result?.from && row.result?.to,
    ).length
    const version = versionById.get(
      side === 'from' ? fromVersionId : toVersionId,
    )
    if (!version) return 'choose a version'
    return `${version.execution_count} execution${version.execution_count === 1 ? '' : 's'} · ${withEvidence} test${withEvidence === 1 ? '' : 's'} with evidence · ${shared} shared with ${side === 'from' ? 'b' : 'a'}`
  }
  const activeCohort = evaluated?.cohorts.find(
    (cohort) => cohort.id === cohortId,
  )
  const aVersion = versionById.get(fromVersionId)
  const bVersion = versionById.get(toVersionId)
  const aLabel = aVersion ? aVersion.label.toLowerCase() : 'a'
  const bLabel = bVersion ? bVersion.label.toLowerCase() : 'b'
  const headline =
    activeCohort && aVersion && bVersion
      ? `${compactModel(activeCohort.subject_model)} judged by ${activeCohort.judge_model ? compactModel(activeCohort.judge_model) : 'the automatic judge'} · ${aLabel} → ${bLabel}`
      : 'two system versions, same model and judge'

  const shareLink = () => {
    void navigator.clipboard?.writeText(window.location.href).then(() => {
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1500)
    })
  }

  const groups = GROUPS.map((group) => ({
    ...group,
    rows: visibleRows.filter((row) =>
      group.key.includes(states.get(row.test_id) ?? 'none'),
    ),
  })).filter((group) => group.rows.length > 0)
  const noneRows = filter === 'all' || showHidden ? hiddenRows : []

  return (
    <>
      <DashboardPageActions
        active="tests"
        context="compare"
        actionsLabel="Comparison actions"
        actions={
          <>
            <button
              className={dashboardHeaderActionClassName()}
              type="button"
              onClick={shareLink}
              disabled={!fromVersionId || !toVersionId}
            >
              <Link2 size={13} aria-hidden="true" />
              {copied ? 'link copied' : 'share link'}
            </button>
            {local ? (
              <a
                className={dashboardHeaderActionClassName({ primary: true })}
                href={hashForWorkspace()}
                onClick={() => requestQuickExecution()}
              >
                new run on b
              </a>
            ) : null}
          </>
        }
      />

      <div className="ds-root page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title="compare"
          summary={headline}
          headingId="compare-title"
          breadcrumb={[
            { label: 'tests', href: hashForTests() },
            { label: 'compare' },
          ]}
          actions={
            evaluated?.revision ? (
              <span
                className="font-mono text-label text-ink-muted"
                title={evaluated.revision}
              >
                catalog {evaluated.revision.slice(-12)}
              </span>
            ) : null
          }
        />

        {/* Audit CP-02 / CP-06: the builder names who was judged by whom and
            what each side holds; one sentence explains when deltas exist. */}
        <Panel className="mt-6" aria-labelledby="compare-builder-title">
          <h2 className="ds-label mb-3" id="compare-builder-title">
            cohort · who was judged by whom
          </h2>
          <div className="grid gap-4 @[840px]:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_auto_minmax(0,1fr)] @[840px]:items-start">
            <div className="grid gap-1">
              <label className="ds-visually-hidden" htmlFor="compare-cohort">
                Evaluation cohort
              </label>
              <Select
                id="compare-cohort"
                value={cohortId}
                disabled={(evaluated?.cohorts.length ?? 0) === 0}
                onChange={(event) => updateCohort(event.target.value)}
              >
                {(evaluated?.cohorts.length ?? 0) === 0 ? (
                  <option value="">no evaluated cohort yet</option>
                ) : null}
                {(evaluated?.cohorts ?? []).map((cohort) => (
                  <option key={cohort.id} value={cohort.id}>
                    {cohortLabel(cohort)}
                  </option>
                ))}
              </Select>
              <span className="font-mono text-label text-ink-muted">
                {activeCohort
                  ? `subject ${activeCohort.subject_provider}/${activeCohort.subject_model} · judge ${activeCohort.judge_provider ? `${activeCohort.judge_provider}/${activeCohort.judge_model}` : 'automatic'}${activeCohort.judge_protocol ? ` · ${activeCohort.judge_protocol}` : ''}`
                  : 'system version = the workers under test · cohort = which model was judged by which judge'}
                {evaluated
                  ? ` · ${cohortsWithPairs} of ${evaluated.cohorts.length} cohorts have ≥ 2 versions`
                  : ''}
              </span>
            </div>
            <div className="grid gap-1">
              <label className="ds-label" htmlFor="compare-a">
                a · baseline
              </label>
              <Select
                id="compare-a"
                value={fromVersionId}
                disabled={versions.length < 2}
                onChange={(event) => selectFromVersion(event.target.value)}
              >
                {versions.length < 2 ? (
                  <option value="">waiting for history</option>
                ) : null}
                {versions.map((version) => (
                  <option
                    key={version.id}
                    value={version.id}
                    disabled={version.id === toVersionId}
                  >
                    {versionOptionLabel(version)}
                  </option>
                ))}
              </Select>
              <span className="font-mono text-label text-ink-muted">
                {sideSummary('from')}
              </span>
            </div>
            <button
              className={buttonClassName({
                variant: 'secondary',
                size: 'compact',
                className: '@[840px]:mt-6',
              })}
              type="button"
              aria-label="Swap system versions a and b"
              title="swap a and b"
              disabled={!fromVersionId || !toVersionId}
              onClick={swapVersions}
            >
              <ArrowLeftRight size={14} aria-hidden="true" />
            </button>
            <div className="grid gap-1">
              <label className="ds-label" htmlFor="compare-b">
                b · candidate
              </label>
              <Select
                id="compare-b"
                value={toVersionId}
                disabled={versions.length === 0}
                onChange={(event) => selectToVersion(event.target.value)}
              >
                {versions.length === 0 ? (
                  <option value="">no retained version</option>
                ) : null}
                {versions.map((version) => (
                  <option
                    key={version.id}
                    value={version.id}
                    disabled={version.id === fromVersionId}
                  >
                    {versionOptionLabel(version)}
                  </option>
                ))}
              </Select>
              <span className="font-mono text-label text-ink-muted">
                {sideSummary('to')}
              </span>
            </div>
          </div>
          <p className="mt-3 mb-0 text-xs text-ink-soft">
            Deltas only between runs of the same model, judge and test contract.
          </p>
          {recommendation ? (
            <Callout tone="info" className="mt-3" data-recommendation>
              <span className="flex flex-wrap items-center justify-between gap-3">
                {recommendation.message}
                {recommendation.previousFrom ? (
                  <button
                    className={buttonClassName({
                      variant: 'secondary',
                      size: 'compact',
                    })}
                    type="button"
                    onClick={() => {
                      const previous = recommendation.previousFrom
                      setRecommendation(null)
                      automaticSelection.current = false
                      if (previous) setFromVersionId(previous)
                    }}
                  >
                    undo
                  </button>
                ) : null}
              </span>
            </Callout>
          ) : null}
        </Panel>

        {error ? (
          <Callout
            tone="danger"
            title="Versioned evidence could not be loaded"
            className="mt-6"
          >
            <span className="flex flex-wrap items-center justify-between gap-3">
              {error.message}
              <button
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                })}
                type="button"
                onClick={() => setReloadKey((value) => value + 1)}
              >
                <RefreshCw size={13} aria-hidden="true" />
                retry
              </button>
            </span>
          </Callout>
        ) : null}

        {/* Audit CP-01 / CP-11: the KPIs are the filters and they count what
            the table shows. */}
        <section className="mt-6 grid gap-3" aria-label="Comparison filters">
          <div className="flex flex-wrap items-center gap-2">
            <FilterChipGroup label="Comparison state">
              <FilterChip
                active={filter === 'evidence'}
                count={evidenceCount}
                onClick={() => setFilter('evidence')}
                title="tests with evidence on at least one side"
              >
                with evidence
              </FilterChip>
              <FilterChip
                active={filter === 'comparable'}
                count={comparableCount}
                onClick={() => setFilter('comparable')}
                title="same test version, cases and contracts on both sides"
              >
                comparable
              </FilterChip>
              <FilterChip
                active={filter === 'regressed'}
                count={counts.regressed}
                onClick={() => setFilter('regressed')}
                title="objective result or score dropped in b"
              >
                regressed in b
              </FilterChip>
              <FilterChip
                active={filter === 'improved'}
                count={counts.improved}
                onClick={() => setFilter('improved')}
                title="score up in b, no gate lost"
              >
                improved in b
              </FilterChip>
              <FilterChip
                active={filter === 'one_side'}
                count={counts.one_side}
                onClick={() => setFilter('one_side')}
                title="run the other side to compare"
              >
                one side
              </FilterChip>
              <FilterChip
                active={filter === 'all'}
                count={rows.length}
                onClick={() => setFilter('all')}
              >
                all
              </FilterChip>
            </FilterChipGroup>
            <div className="relative ms-auto w-full max-w-[18rem]">
              <Search
                className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-ink-muted"
                size={14}
                aria-hidden="true"
              />
              <Input
                className="pr-9 pl-9"
                type="text"
                value={query}
                placeholder="Search test id"
                aria-label="Search tests"
                onChange={(event) => setQuery(event.target.value)}
              />
              {query ? (
                <button
                  className="absolute top-1/2 right-1 inline-grid size-7 -translate-y-1/2 place-items-center rounded-[6px] border-0 bg-transparent text-ink-muted hover:bg-[var(--surface-soft)] hover:text-ink"
                  type="button"
                  onClick={() => setQuery('')}
                  aria-label="Clear search"
                >
                  <X size={13} aria-hidden="true" />
                </button>
              ) : null}
            </div>
          </div>
          <output
            className="font-mono text-label text-ink-muted"
            aria-live="polite"
          >
            {loading && rows.length === 0
              ? 'loading evidence…'
              : `${visibleRows.length} of ${rows.length} tests · ${comparableCount} comparable · ${counts.regressed} regressed · ${counts.improved} improved · ${counts.one_side} on one side · ${counts.none} without evidence`}
          </output>
        </section>

        {loading && rows.length === 0 ? (
          <div className="mt-4 grid gap-px" aria-busy="true" role="status">
            <span className="ds-visually-hidden">Loading test catalog</span>
            {Array.from({ length: 6 }, (_, index) => (
              <div
                // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
                key={index}
                className="h-12 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
              />
            ))}
          </div>
        ) : !error && groups.length === 0 && noneRows.length === 0 ? (
          <EmptyState
            className="mt-6"
            title={
              rows.length === 0
                ? 'No tests are registered yet'
                : counts.none === rows.length
                  ? 'No evidence in this cohort for these two versions'
                  : 'No tests match this view'
            }
            description={
              rows.length === 0
                ? 'Create an execution to publish the first test evidence.'
                : counts.none === rows.length
                  ? 'Pick another pair of versions, or run tests on both sides.'
                  : 'Widen the state filter or clear the search.'
            }
            actions={
              rows.length > 0 && counts.none !== rows.length ? (
                <button
                  className={buttonClassName({ variant: 'secondary' })}
                  type="button"
                  onClick={() => {
                    setFilter('evidence')
                    setQuery('')
                  }}
                >
                  clear filters
                </button>
              ) : local ? (
                <a
                  className={buttonClassName({
                    variant: 'primary',
                    className: 'no-underline',
                  })}
                  href={hashForWorkspace()}
                  onClick={() => requestQuickExecution()}
                >
                  run tests
                </a>
              ) : null
            }
          />
        ) : (
          <div className="mt-4 grid gap-6" data-compare-groups>
            {groups.map((group) => (
              <section
                key={group.label}
                aria-labelledby={`compare-group-${group.key[0]}`}
                data-compare-group={group.key[0]}
              >
                <h2
                  className="ds-label mb-2 flex flex-wrap items-baseline gap-2"
                  id={`compare-group-${group.key[0]}`}
                >
                  {group.label} · {group.rows.length}
                  <span className="font-normal normal-case tracking-normal text-ink-muted">
                    {group.hint}
                  </span>
                </h2>
                <DataTable
                  caption={`${group.label} tests, ${group.rows.length}`}
                  collapse
                  minWidth="64rem"
                  sticky
                >
                  <thead>
                    <tr>
                      <th scope="col">test</th>
                      <th
                        scope="col"
                        title={
                          aVersion ? versionOptionLabel(aVersion) : undefined
                        }
                      >
                        a · {aLabel}
                      </th>
                      <th
                        scope="col"
                        title={
                          bVersion ? versionOptionLabel(bVersion) : undefined
                        }
                      >
                        b · {bLabel}
                      </th>
                      <th
                        scope="col"
                        className={numericCellClassName}
                        title="b − a, in score points"
                      >
                        Δ score
                      </th>
                      <th
                        scope="col"
                        className={numericCellClassName}
                        title="b − a, relative to a"
                      >
                        Δ tokens
                      </th>
                      <th scope="col">
                        <span className="ds-visually-hidden">Actions</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {group.rows.map((row) => (
                      <CompareRow
                        key={row.test_id}
                        row={row}
                        aLabel={aLabel}
                        bLabel={bLabel}
                        local={local}
                        expanded={expanded.has(row.test_id)}
                        loading={rowLoading.has(row.test_id)}
                        error={rowErrors[row.test_id]}
                        onVersion={selectTestVersion}
                        onToggle={toggleDetails}
                      />
                    ))}
                  </tbody>
                </DataTable>
              </section>
            ))}
            {hiddenRows.length > 0 && filter === 'evidence' ? (
              // Audit CP-01: the rows with no evidence are one collapsed line.
              <div data-compare-group="none">
                <button
                  className={buttonClassName({
                    variant: 'quiet',
                    size: 'compact',
                  })}
                  type="button"
                  aria-expanded={showHidden}
                  onClick={() => setShowHidden((value) => !value)}
                >
                  {hiddenRows.length} test{hiddenRows.length === 1 ? '' : 's'}{' '}
                  without evidence in this cohort
                  <ChevronDown
                    className={`transition-transform duration-[var(--ds-duration-fast)] ${showHidden ? 'rotate-180' : ''}`}
                    size={13}
                    aria-hidden="true"
                  />
                </button>
                {showHidden ? (
                  <ul className="mt-2 grid list-none gap-1 p-0 font-mono text-xs text-ink-soft @[560px]:grid-cols-2 @[960px]:grid-cols-4">
                    {hiddenRows.map((row) => (
                      <li key={row.test_id}>
                        <a
                          className="text-ink-soft no-underline hover:text-ink hover:underline"
                          href={hashForTestHistory(row.test_id)}
                        >
                          {row.test_id}
                        </a>
                      </li>
                    ))}
                  </ul>
                ) : null}
              </div>
            ) : null}
          </div>
        )}
      </div>
    </>
  )
}
