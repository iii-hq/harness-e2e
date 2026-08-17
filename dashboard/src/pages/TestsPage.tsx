import {
  AlertTriangle,
  ArrowLeftRight,
  ChevronDown,
  ExternalLink,
  RefreshCw,
  Search,
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ThemeToggle } from '@/components/ThemeToggle'
import {
  hashForCoverage,
  hashForExecution,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import { summarizeAssessmentContract } from '@/lib/assessment-contract'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
} from '@/lib/dashboard-data-source'
import type {
  CohortDescriptor,
  EvaluatedVersionsResponse,
  TestCatalogRow,
  TestSideSummary,
  TestsListResponse,
  TestVersionResult,
} from '@/lib/test-catalog'
import {
  comparisonUtility,
  comparisonWarnings,
  hasRetainedEvidence,
  isMoreUsefulComparison,
  matchesResultFilter,
  type ResultFilter,
  sortCatalogRows,
} from '@/lib/test-catalog-view'

const inputClass =
  'min-h-11 w-full rounded-lg border border-line bg-panel-raised px-3 text-sm text-ink outline-none transition focus:border-brand focus:ring-2 focus:ring-brand-soft'

function displayTestName(value: string) {
  return value.replaceAll('_', ' ').replaceAll('.', ' / ')
}

function formatNumber(value: number | null, digits = 1) {
  if (value === null || !Number.isFinite(value)) return '—'
  return new Intl.NumberFormat('en-US', {
    maximumFractionDigits: digits,
  }).format(value)
}

function formatCurrency(value: number | null) {
  if (value === null || !Number.isFinite(value)) return '—'
  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    maximumFractionDigits: value < 1 ? 3 : 2,
  }).format(value)
}

function formatDuration(value: number | null) {
  if (value === null || !Number.isFinite(value)) return '—'
  if (value < 60) return `${formatNumber(value)}s`
  return `${Math.floor(value / 60)}m ${String(Math.round(value % 60)).padStart(2, '0')}s`
}

function summaryStatus(summary: TestSideSummary | null) {
  if (!summary) return { label: 'Not run', tone: 'text-ink-muted border-line' }
  if (summary.outcomes.infra_failed > 0) {
    return {
      label: 'Infrastructure failure',
      tone: 'text-danger border-danger/30',
    }
  }
  if (summary.outcomes.technical_failed > 0) {
    return { label: 'Technical failure', tone: 'text-danger border-danger/30' }
  }
  if (summary.outcomes.hard_gate_failed > 0) {
    return { label: 'Hard gate failed', tone: 'text-warning border-warning/30' }
  }
  return { label: 'Passed', tone: 'text-success border-success/30' }
}

export function SideResult({ summary }: { summary: TestSideSummary | null }) {
  const status = summaryStatus(summary)
  if (!summary) {
    return <span className="text-sm text-ink-muted">Not run</span>
  }
  const assessments =
    summary.assessment_summary ?? summarizeAssessmentContract({ runs: [] })
  const aiVerdict =
    assessments.ai_verdicts.fail > 0
      ? 'Fail'
      : assessments.ai_verdicts.pass_with_concerns > 0
        ? 'Pass with concerns'
        : assessments.ai_verdicts.pass > 0
          ? 'Pass'
          : assessments.ai_verdicts.inconclusive > 0
            ? 'Inconclusive'
            : assessments.ai_availability.available > 0
              ? 'Available'
              : 'Unavailable'
  const effectiveStatus =
    assessments.effective_statuses.hard_gate_failed > 0
      ? 'Hard gate failed'
      : assessments.effective_statuses.subject_error > 0 ||
          assessments.effective_statuses.judge_error > 0 ||
          assessments.effective_statuses.infrastructure_error > 0 ||
          assessments.effective_statuses.resource_limit > 0
        ? 'Technical failure'
        : assessments.effective_statuses.passed_with_concerns > 0
          ? 'Passed with concerns'
          : assessments.effective_statuses.passed > 0
            ? 'Passed'
            : 'Unavailable'
  return (
    <div className="grid gap-1.5">
      <span
        className={`w-fit rounded-full border px-2 py-1 text-[0.65rem] font-semibold ${status.tone}`}
      >
        System: {status.label}
      </span>
      <strong className="text-base font-semibold tracking-[-0.03em] text-ink">
        {formatNumber(summary.median_score)}
        <span className="ml-1 text-xs font-normal text-ink-muted">/100</span>
      </strong>
      <span className="font-mono text-[0.64rem] text-ink-muted">
        n={summary.scored_runs} scored · {summary.total_runs} total
      </span>
      <span className="text-[0.66rem] text-ink-muted">AI: {aiVerdict}</span>
      <span className="text-[0.66rem] text-ink-muted">
        Effective: {effectiveStatus}
      </span>
      <span className="text-[0.66rem] text-ink-muted">
        {assessments.evidence_reference_count} evidence references
      </span>
    </div>
  )
}

function compatibilityLabel(result: TestVersionResult | null) {
  if (!result) return 'Choose two system versions'
  return {
    compatible: 'Comparable contract',
    missing_side: 'Missing on one side',
    contract_changed: 'Cases or contract changed',
    contract_conflict: 'Contract conflict',
    assessment_changed: 'Assessment profile changed',
    assessment_conflict: 'Assessment profile conflict',
    analyzer_changed: 'Analyzer profile changed',
    analyzer_conflict: 'Analyzer profile conflict',
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

function RowDetails({ result }: { result: TestVersionResult | null }) {
  if (!result) {
    return (
      <p className="m-0 text-sm text-ink-muted">
        Select two versions to inspect evidence.
      </p>
    )
  }
  const observations = [
    ...result.from_observations.map((item) => ({ ...item, side: 'A' })),
    ...result.to_observations.map((item) => ({ ...item, side: 'B' })),
  ]
  const warnings = comparisonWarnings(result)
  return (
    <div className="grid gap-5">
      {warnings.length > 0 && (
        <div
          className="grid gap-2 rounded-lg border border-warning/30 bg-warning/5 p-4"
          role="alert"
        >
          <div className="flex items-center gap-2 text-sm font-semibold text-warning">
            <AlertTriangle size={16} aria-hidden="true" /> Comparison disabled
          </div>
          <ul className="m-0 grid gap-2 pl-5 text-sm text-ink-soft">
            {warnings.map((warning) => (
              <li key={`${warning.title}:${warning.detail}`}>
                <strong>{warning.title}.</strong> {warning.detail}
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {[
          [
            'Pass rate A',
            result.from?.pass_rate == null
              ? '—'
              : `${formatNumber(result.from.pass_rate * 100)}%`,
          ],
          [
            'Pass rate B',
            result.to?.pass_rate == null
              ? '—'
              : `${formatNumber(result.to.pass_rate * 100)}%`,
          ],
          [
            'Median cost A → B',
            `${formatCurrency(result.from?.median_cost_usd ?? null)} → ${formatCurrency(result.to?.median_cost_usd ?? null)}`,
          ],
          [
            'Median runtime A → B',
            `${formatDuration(result.from?.median_duration_seconds ?? null)} → ${formatDuration(result.to?.median_duration_seconds ?? null)}`,
          ],
        ].map(([label, value]) => (
          <div
            key={label}
            className="grid gap-1 rounded-lg border border-line bg-panel px-3 py-3"
          >
            <span className="text-[0.64rem] font-semibold uppercase tracking-[0.06em] text-ink-muted">
              {label}
            </span>
            <strong className="text-sm text-ink">{value}</strong>
          </div>
        ))}
      </div>
      <div className="grid gap-2">
        <span className="text-[0.64rem] font-semibold uppercase tracking-[0.06em] text-ink-muted">
          Retained evidence
        </span>
        {observations.length === 0 ? (
          <span className="text-sm text-ink-muted">
            No retained observations.
          </span>
        ) : (
          <div className="grid gap-2">
            {observations.map((observation) => (
              <a
                key={`${observation.side}-${observation.execution_id}-${observation.case_id}`}
                className="flex min-h-11 items-center justify-between gap-3 rounded-lg border border-line bg-panel px-3 py-2 text-sm text-ink no-underline hover:border-line-strong hover:bg-panel-raised"
                href={hashForExecution(observation.execution_id)}
              >
                <span className="min-w-0">
                  <strong className="mr-2 text-brand">
                    {observation.side}
                  </strong>
                  <span className="break-all font-mono text-xs">
                    {observation.execution_id}
                  </span>
                  <small className="mt-1 block text-ink-muted">
                    {observation.case_id} · score{' '}
                    {formatNumber(observation.median_score)} · n=
                    {observation.scored_runs}
                  </small>
                  <small className="mt-1 block text-ink-muted">
                    {observation.assessment_summary?.assessment_count ?? 0}{' '}
                    assessments ·{' '}
                    {observation.assessment_summary?.evidence_reference_count ??
                      0}{' '}
                    evidence references
                  </small>
                </span>
                <ExternalLink size={15} aria-hidden="true" />
              </a>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

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
  const [resultFilter, setResultFilter] = useState<ResultFilter>('all')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<Error | null>(null)
  const [rowLoading, setRowLoading] = useState<Set<string>>(new Set())
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({})
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [detailsLoaded, setDetailsLoaded] = useState<Set<string>>(new Set())
  const [comparisonNotice, setComparisonNotice] = useState('')
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
          setComparisonNotice(
            bestUtility.comparable > 0
              ? `Selected the closest retained version with ${bestUtility.comparable} comparable test${bestUtility.comparable === 1 ? '' : 's'}.`
              : 'Selected the retained version with the most shared evidence.',
          )
          setFromVersionId(best.fromVersionId)
          return
        }
        if (bestUtility.comparable === 0) {
          setComparisonNotice(
            'No shared canonical cases were found for the latest system version. One-sided evidence remains available below.',
          )
        }
      }
      if (
        cohortId &&
        automaticSelection.current &&
        versions.length <= 2 &&
        comparisonUtility(response.rows).comparable === 0
      ) {
        setComparisonNotice(
          'No shared canonical cases were found for this cohort. One-sided evidence remains available below.',
        )
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

  const filteredRows = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase()
    return sortCatalogRows(
      rows.filter(
        (row) =>
          (!normalizedQuery ||
            row.test_id.toLowerCase().includes(normalizedQuery)) &&
          matchesResultFilter(row, resultFilter),
      ),
    )
  }, [rows, query, resultFilter])
  const compatibleCount = rows.filter(
    (row) => row.result?.compatibility === 'compatible',
  ).length
  const issueCount = rows.filter((row) =>
    matchesResultFilter(row, 'issues'),
  ).length

  const updateCohort = (nextCohort: string) => {
    automaticSelection.current = true
    setComparisonNotice('')
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
    setComparisonNotice('')
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
    setComparisonNotice('')
    setFromVersionId(versionId)
  }

  const selectToVersion = (versionId: string) => {
    automaticSelection.current = false
    setComparisonNotice('')
    setToVersionId(versionId)
  }

  const compactModel = (model: string) => model.split('/').at(-1) || model

  const cohortLabel = (cohort: CohortDescriptor) => {
    const judge = cohort.judge_model
      ? `judge ${compactModel(cohort.judge_model)}`
      : 'automatic judge'
    return `${compactModel(cohort.subject_model)} · ${cohort.lane} · ${judge}`
  }

  const cohortDetail = (cohort: CohortDescriptor) => {
    const judge = cohort.judge_model
      ? `judge ${cohort.judge_provider}/${cohort.judge_model}${
          cohort.judge_protocol ? ` (${cohort.judge_protocol})` : ''
        }`
      : 'automatic judge'
    return `subject ${cohort.subject_provider}/${cohort.subject_model} · ${cohort.lane} · ${judge}`
  }

  const versionLabel = (versionId: string) => {
    const version = versionById.get(versionId)
    if (!version) return ''
    const duplicate = versions.some(
      (candidate) =>
        candidate.id !== version.id && candidate.label === version.label,
    )
    return duplicate
      ? `${version.label} · ${version.id.slice(-6)}`
      : version.label
  }
  const activeCohort = evaluated?.cohorts.find(
    (cohort) => cohort.id === cohortId,
  )

  return (
    <>
      <a className="skip-link" href="#tests-main">
        Skip to versioned tests
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
        <nav className="topbar-actions" aria-label="Dashboard actions">
          <a
            className="button button-primary"
            href={hashForWorkspace()}
            data-mobile-label="New"
          >
            ＋ New execution
          </a>
          <a
            className="button button-secondary"
            href={hashForCoverage()}
            data-mobile-label="Coverage"
          >
            Coverage
          </a>
          <ThemeToggle />
        </nav>
      </header>

      <main id="tests-main" className="page-shell overview-shell">
        <section className="page-heading" aria-labelledby="tests-title">
          <div>
            <div className="eyebrow">
              <span className="live-dot" aria-hidden="true"></span>
              Versioned evidence
            </div>
            <h1 id="tests-title">Tests across system versions</h1>
            <p>
              Choose one contract version per test and compare immutable system
              versions without collapsing quality into a single number.
            </p>
          </div>
          <div className="sync-block">
            <span>Catalog revision</span>
            <strong className="font-mono text-xs text-ink-muted">
              {evaluated?.revision.slice(-12) ?? 'loading'}
            </strong>
          </div>
        </section>

        <section
          className="panel overflow-visible"
          aria-labelledby="comparison-controls-title"
        >
          <div className="panel-heading">
            <div>
              <div className="section-kicker">Comparison context</div>
              <h2 id="comparison-controls-title">System version A → B</h2>
              <p className="trend-description">
                Deltas are enabled only for the same cohort, test version,
                cases, canonical contracts, assessment profile, and analyzer
                profile. Cross-cohort comparisons are never combined.
              </p>
            </div>
            <span
              className={`comparison-verdict ${
                compatibleCount > 0
                  ? 'comparison-verdict-eligible'
                  : 'comparison-verdict-exploratory'
              }`}
              aria-live="polite"
            >
              {fromVersionId && toVersionId
                ? `${compatibleCount} comparable test${compatibleCount === 1 ? '' : 's'}`
                : 'Waiting for two system versions'}
            </span>
          </div>
          <div className="grid gap-4 border-t border-line p-5 lg:grid-cols-[minmax(180px,1fr)_minmax(220px,1.3fr)_44px_minmax(220px,1.3fr)]">
            <label className="grid content-start gap-2 text-xs font-semibold text-ink-muted">
              Evaluation cohort
              <select
                className={inputClass}
                value={cohortId}
                title={activeCohort ? cohortDetail(activeCohort) : undefined}
                disabled={(evaluated?.cohorts.length ?? 0) === 0}
                onChange={(event) => updateCohort(event.target.value)}
              >
                {(evaluated?.cohorts.length ?? 0) === 0 && (
                  <option value="">No evaluated cohort yet</option>
                )}
                {(evaluated?.cohorts ?? []).map((cohort) => (
                  <option key={cohort.id} value={cohort.id}>
                    {cohortLabel(cohort)}
                  </option>
                ))}
              </select>
              {activeCohort && (
                <span
                  className="truncate font-normal text-ink-muted"
                  title={cohortDetail(activeCohort)}
                >
                  {cohortDetail(activeCohort)}
                </span>
              )}
            </label>
            <label className="grid content-start gap-2 text-xs font-semibold text-ink-muted">
              System version A
              <select
                className={inputClass}
                value={fromVersionId}
                disabled={versions.length < 2}
                onChange={(event) => selectFromVersion(event.target.value)}
              >
                {versions.length < 2 && (
                  <option value="">Waiting for history</option>
                )}
                {versions.map((version) => (
                  <option
                    key={version.id}
                    value={version.id}
                    disabled={version.id === toVersionId}
                  >
                    {versionLabel(version.id)}
                  </option>
                ))}
              </select>
            </label>
            <button
              className="flex h-11 min-w-11 items-center justify-center rounded-lg border border-line bg-panel-raised text-ink hover:border-line-strong disabled:cursor-not-allowed disabled:opacity-40 lg:mt-6"
              type="button"
              aria-label="Swap system versions A and B"
              disabled={!fromVersionId || !toVersionId}
              onClick={swapVersions}
            >
              <ArrowLeftRight size={17} aria-hidden="true" />
            </button>
            <label className="grid content-start gap-2 text-xs font-semibold text-ink-muted">
              System version B
              <select
                className={inputClass}
                value={toVersionId}
                disabled={versions.length === 0}
                onChange={(event) => selectToVersion(event.target.value)}
              >
                {versions.length === 0 && (
                  <option value="">No retained version</option>
                )}
                {versions.map((version) => (
                  <option
                    key={version.id}
                    value={version.id}
                    disabled={version.id === fromVersionId}
                  >
                    {versionLabel(version.id)}
                  </option>
                ))}
              </select>
            </label>
          </div>
          {comparisonNotice && (
            <p
              className="m-0 border-t border-line px-5 py-3 text-xs text-ink-muted"
              role="status"
            >
              {comparisonNotice}
            </p>
          )}
        </section>

        <section className="panel mt-5" aria-labelledby="test-catalog-title">
          <div className="panel-heading items-end gap-5">
            <div>
              <div className="section-kicker">Test catalog</div>
              <h2 id="test-catalog-title">One row per test</h2>
              <p className="trend-description">
                Scores remain attached to their test, contract version, and
                sample.
              </p>
            </div>
            <div
              className="flex flex-wrap gap-2 text-xs text-ink-muted"
              aria-live="polite"
            >
              <span>{rows.length} tests</span>
              <span aria-hidden="true">·</span>
              <span>{compatibleCount} comparable</span>
              <span aria-hidden="true">·</span>
              <span>{issueCount} with issues in B</span>
            </div>
          </div>
          <div className="grid gap-3 border-t border-line p-4 sm:grid-cols-[minmax(0,1fr)_190px]">
            <label className="relative block">
              <span className="sr-only">Search tests</span>
              <Search
                className="absolute left-3 top-1/2 -translate-y-1/2 text-ink-muted"
                size={16}
                aria-hidden="true"
              />
              <input
                className={`${inputClass} pl-10`}
                type="search"
                value={query}
                placeholder="Search test ID"
                onChange={(event) => setQuery(event.target.value)}
              />
            </label>
            <label>
              <span className="sr-only">Filter test results</span>
              <select
                className={inputClass}
                value={resultFilter}
                onChange={(event) =>
                  setResultFilter(event.target.value as ResultFilter)
                }
              >
                <option value="all">All results</option>
                <option value="passed">Passing in B</option>
                <option value="issues">Issues in B</option>
                <option value="missing">Missing side</option>
                <option value="changed">
                  Incompatible contract, assessment, or analyzer
                </option>
              </select>
            </label>
          </div>

          {error && (
            <div
              className="m-4 flex items-start justify-between gap-4 rounded-lg border border-danger/30 bg-danger/5 p-4"
              role="alert"
            >
              <div className="flex gap-3">
                <AlertTriangle
                  className="mt-0.5 text-danger"
                  size={18}
                  aria-hidden="true"
                />
                <div>
                  <strong className="block text-sm text-ink">
                    Versioned evidence could not be loaded
                  </strong>
                  <span className="text-sm text-ink-muted">
                    {error.message}
                  </span>
                </div>
              </div>
              <button
                className="button"
                type="button"
                onClick={() => setReloadKey((value) => value + 1)}
              >
                <RefreshCw size={14} aria-hidden="true" /> Retry
              </button>
            </div>
          )}

          {loading && rows.length === 0 ? (
            <div className="grid gap-2 p-4" aria-busy="true" role="status">
              <span className="sr-only">Loading test catalog</span>
              {['first', 'second', 'third', 'fourth', 'fifth', 'sixth'].map(
                (placeholder) => (
                  <div
                    key={placeholder}
                    className="h-20 animate-pulse rounded-lg border border-line bg-panel-raised"
                  />
                ),
              )}
            </div>
          ) : filteredRows.length === 0 ? (
            <div className="empty-state m-4">
              <h3>
                {rows.length === 0
                  ? 'No tests are registered yet'
                  : 'No tests match this view'}
              </h3>
              <p>
                {rows.length === 0
                  ? 'Create an execution to publish the first test evidence.'
                  : 'Clear the search or result filter to see the full catalog.'}
              </p>
              <a className="button button-primary" href={hashForWorkspace()}>
                New execution
              </a>
            </div>
          ) : (
            <>
              <div className="hidden overflow-x-auto md:block">
                <table className="w-full border-collapse text-left">
                  <thead>
                    <tr className="border-y border-line text-[0.64rem] uppercase tracking-[0.06em] text-ink-muted">
                      <th className="px-4 py-3 font-semibold">Test</th>
                      <th className="px-4 py-3 font-semibold">Test version</th>
                      <th className="px-4 py-3 font-semibold">Version A</th>
                      <th className="px-4 py-3 font-semibold">Version B</th>
                      <th className="px-4 py-3 font-semibold">Δ score</th>
                      <th className="px-4 py-3 font-semibold">Evidence</th>
                    </tr>
                  </thead>
                  <tbody>
                    {filteredRows.map((row) => {
                      const isExpanded = expanded.has(row.test_id)
                      const isLoading = rowLoading.has(row.test_id)
                      const delta = row.result?.delta.score ?? null
                      return (
                        <TestRows
                          key={row.test_id}
                          row={row}
                          expanded={isExpanded}
                          loading={isLoading}
                          error={rowErrors[row.test_id]}
                          delta={delta}
                          onVersion={selectTestVersion}
                          onToggle={toggleDetails}
                        />
                      )
                    })}
                  </tbody>
                </table>
              </div>
              <div className="grid gap-3 p-3 md:hidden">
                {filteredRows.map((row) => (
                  <TestCard
                    key={row.test_id}
                    row={row}
                    expanded={expanded.has(row.test_id)}
                    loading={rowLoading.has(row.test_id)}
                    error={rowErrors[row.test_id]}
                    onVersion={selectTestVersion}
                    onToggle={toggleDetails}
                  />
                ))}
              </div>
            </>
          )}
        </section>
      </main>
      <footer>
        <span>Harness E2E · versioned evidence</span>
        <span>
          {versionLabel(fromVersionId) || 'A unavailable'} →{' '}
          {versionLabel(toVersionId) || 'B unavailable'}
        </span>
      </footer>
    </>
  )
}

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
      <span className="font-mono text-xs text-ink-muted">
        v{row.selected_version ?? row.current_version ?? '—'}
      </span>
    )
  }
  return (
    <select
      className="min-h-11 rounded-lg border border-line bg-panel-raised px-2 text-sm text-ink outline-none focus:border-brand"
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
    </select>
  )
}

function TestRows({
  row,
  expanded,
  loading,
  error,
  delta,
  onVersion,
  onToggle,
}: {
  row: TestCatalogRow
  expanded: boolean
  loading: boolean
  error?: string
  delta: number | null
  onVersion: (row: TestCatalogRow, version: number) => void
  onToggle: (row: TestCatalogRow) => void
}) {
  const evidenceAvailable = hasRetainedEvidence(row)
  const testName = displayTestName(row.test_id)
  return (
    <>
      <tr className="border-b border-line align-middle hover:bg-panel-subtle">
        <th className="px-4 py-4" scope="row">
          <strong className="block text-sm font-semibold text-ink">
            {displayTestName(row.test_id)}
          </strong>
          <span className="mt-1 block font-mono text-[0.62rem] text-ink-muted">
            {row.test_id}
          </span>
        </th>
        <td className="px-4 py-4">
          <TestVersionSelect
            row={row}
            disabled={loading}
            onVersion={onVersion}
          />
        </td>
        <td className="px-4 py-4">
          <SideResult summary={row.result?.from ?? null} />
        </td>
        <td className="px-4 py-4">
          <SideResult summary={row.result?.to ?? null} />
        </td>
        <td className="px-4 py-4">
          {row.result?.compatibility === 'compatible' ? (
            <strong
              className={
                delta === null
                  ? 'text-ink-muted'
                  : delta >= 0
                    ? 'text-success'
                    : 'text-danger'
              }
            >
              {delta !== null && delta > 0 ? '+' : ''}
              {formatNumber(delta)}
            </strong>
          ) : (
            <span className="text-xs text-warning">
              {compatibilityLabel(row.result)}
            </span>
          )}
        </td>
        <td className="px-4 py-4">
          <button
            className="flex min-h-11 items-center gap-2 rounded-lg border border-line px-3 text-xs font-semibold text-ink hover:border-line-strong disabled:cursor-not-allowed disabled:border-line disabled:text-ink-muted disabled:opacity-60"
            type="button"
            aria-expanded={expanded}
            aria-label={
              evidenceAvailable
                ? `${expanded ? 'Hide' : 'Inspect'} evidence for ${testName}`
                : `No retained evidence for ${testName}`
            }
            disabled={!evidenceAvailable || loading}
            onClick={() => onToggle(row)}
          >
            {loading
              ? 'Loading…'
              : !evidenceAvailable
                ? 'No evidence'
                : expanded
                  ? 'Hide'
                  : 'Inspect'}
            {evidenceAvailable && (
              <ChevronDown
                className={`transition ${expanded ? 'rotate-180' : ''}`}
                size={14}
                aria-hidden="true"
              />
            )}
          </button>
        </td>
      </tr>
      {(expanded || error) && (
        <tr className="border-b border-line bg-panel-soft/40">
          <td colSpan={6} className="px-5 py-5">
            {error ? (
              <div
                className="flex items-center justify-between gap-3 text-sm text-danger"
                role="alert"
              >
                <span>{error}</span>
                <button
                  className="button"
                  type="button"
                  onClick={() =>
                    row.selected_version && onVersion(row, row.selected_version)
                  }
                >
                  Retry
                </button>
              </div>
            ) : (
              <RowDetails result={row.result} />
            )}
          </td>
        </tr>
      )}
    </>
  )
}

function TestCard({
  row,
  expanded,
  loading,
  error,
  onVersion,
  onToggle,
}: {
  row: TestCatalogRow
  expanded: boolean
  loading: boolean
  error?: string
  onVersion: (row: TestCatalogRow, version: number) => void
  onToggle: (row: TestCatalogRow) => void
}) {
  const delta = row.result?.delta.score ?? null
  const evidenceAvailable = hasRetainedEvidence(row)
  const testName = displayTestName(row.test_id)
  return (
    <article className="grid gap-4 rounded-xl border border-line bg-panel-raised p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <strong className="block text-sm text-ink">
            {displayTestName(row.test_id)}
          </strong>
          <span className="block truncate font-mono text-[0.62rem] text-ink-muted">
            {row.test_id}
          </span>
        </div>
        <TestVersionSelect row={row} disabled={loading} onVersion={onVersion} />
      </div>
      <div className="grid grid-cols-2 gap-3">
        <div className="rounded-lg border border-line bg-panel p-3">
          <span className="mb-2 block text-[0.62rem] font-semibold uppercase text-ink-muted">
            Version A
          </span>
          <SideResult summary={row.result?.from ?? null} />
        </div>
        <div className="rounded-lg border border-line bg-panel p-3">
          <span className="mb-2 block text-[0.62rem] font-semibold uppercase text-ink-muted">
            Version B
          </span>
          <SideResult summary={row.result?.to ?? null} />
        </div>
      </div>
      <div className="flex items-center justify-between gap-3 border-t border-line pt-3">
        <span className="text-xs text-ink-muted">
          {row.result?.compatibility === 'compatible'
            ? `Δ score ${delta !== null && delta > 0 ? '+' : ''}${formatNumber(delta)}`
            : compatibilityLabel(row.result)}
        </span>
        <button
          className="button"
          type="button"
          aria-expanded={expanded}
          aria-label={
            evidenceAvailable
              ? `${expanded ? 'Hide' : 'Inspect'} evidence for ${testName}`
              : `No retained evidence for ${testName}`
          }
          disabled={!evidenceAvailable || loading}
          onClick={() => onToggle(row)}
        >
          {loading
            ? 'Loading…'
            : !evidenceAvailable
              ? 'No evidence'
              : expanded
                ? 'Hide evidence'
                : 'Inspect evidence'}
        </button>
      </div>
      {error && (
        <div className="text-sm text-danger" role="alert">
          {error}
        </div>
      )}
      {expanded && !error && <RowDetails result={row.result} />}
    </article>
  )
}
