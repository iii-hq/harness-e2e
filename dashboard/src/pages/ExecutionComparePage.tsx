import {
  ArrowLeftRight,
  ChevronDown,
  ClipboardCopy,
  Link2,
  RotateCcw,
} from 'lucide-react'
import { Fragment, type ReactNode, useEffect, useMemo, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { requestRunAgain } from '@/components/ExecutionSetup'
import { ScenarioMatrix } from '@/components/ScenarioMatrix'
import { TranscriptDialog } from '@/components/TranscriptDialog'
import {
  buttonClassName,
  Callout,
  DataTable,
  EmptyState,
  numericCellClassName,
  PageHeader,
} from '@/design-system'
import {
  hashForComparison,
  hashForExecution,
  hashForWorkspace,
  hashWithParams,
  replaceRouteParams,
  routeParams,
} from '@/hooks/use-hash-route'
import type { AssessmentRunView } from '@/lib/assessment-view'
import {
  type DashboardExecutionDetail,
  getDashboardDataBridge,
  type JsonObject,
} from '@/lib/dashboard-data-source'
import {
  compareExecutions,
  comparisonMarkdown,
  type ExecutionComparison,
  exclusionPhrase,
  type ScenarioComparison,
} from '@/lib/execution-comparison'
import {
  formatPlanMetricDelta,
  formatPlanMetricValue,
  type PlanMetricComparison,
} from '@/lib/plan-comparison'
import '@/design-system/styles.css'

type Choice = { include: string[]; exclude: string[] }
type Sides = { a: DashboardExecutionDetail; b: DashboardExecutionDetail }

function listParam(params: URLSearchParams, key: string): string[] {
  return (params.get(key) ?? '').split(',').filter(Boolean)
}

/** The reader's choices live in the hash, so a shared link shows the same
 *  totals. */
export function choiceFromParams(params: URLSearchParams): Choice {
  return {
    include: listParam(params, 'include'),
    exclude: listParam(params, 'exclude'),
  }
}

export function choiceToParams(choice: Choice): URLSearchParams {
  const params = new URLSearchParams()
  if (choice.include.length > 0) params.set('include', choice.include.join(','))
  if (choice.exclude.length > 0) params.set('exclude', choice.exclude.join(','))
  return params
}

/** Take a counted scenario out of the totals, or bring an uncounted one back. */
export function toggleCounted(
  choice: Choice,
  scenario: ScenarioComparison,
): Choice {
  const include = new Set(choice.include)
  const exclude = new Set(choice.exclude)
  if (scenario.counted) {
    if (scenario.exclusion) include.delete(scenario.id)
    else exclude.add(scenario.id)
  } else {
    exclude.delete(scenario.id)
    if (scenario.exclusion) include.add(scenario.id)
  }
  return { include: [...include].sort(), exclude: [...exclude].sort() }
}

/** One scenario of an execution, for the evidence views. */
function scenarioDetail(
  detail: DashboardExecutionDetail,
  scenarioId: string,
): DashboardExecutionDetail {
  return {
    ...detail,
    subjects: detail.subjects.map((subject) => ({
      ...subject,
      scenarios: subject.scenarios.filter(
        (scenario) => scenario.id === scenarioId,
      ),
    })),
    reports: detail.reports.filter(
      (record) => record.scenario_id === scenarioId,
    ),
  }
}

function objects(value: unknown): JsonObject[] {
  return Array.isArray(value)
    ? value.filter(
        (item): item is JsonObject =>
          Boolean(item) && typeof item === 'object' && !Array.isArray(item),
      )
    : []
}

/** Screenshots each run's deliverables name, as the report lists them. */
function screenshotsOf(detail: DashboardExecutionDetail, scenarioId: string) {
  return scenarioDetail(detail, scenarioId).reports.flatMap((record) =>
    (record.report?.scenarios ?? []).flatMap((scenario) =>
      scenario.runs.flatMap((run) =>
        objects(run.deliverables).flatMap((deliverable) =>
          objects(deliverable.screenshots).map((screenshot) => ({
            key: `${run.run_id}:${deliverable.id}:${screenshot.pointer}`,
            runId: run.run_id,
            caption: String(screenshot.caption ?? screenshot.pointer ?? ''),
            mediaType: String(screenshot.media_type ?? ''),
          })),
        ),
      ),
    ),
  )
}

function MetricTable({
  caption,
  metrics,
}: {
  caption: string
  metrics: PlanMetricComparison[]
}) {
  return (
    <DataTable caption={caption} collapse data-comparison-metrics>
      <thead>
        <tr>
          <th scope="col">metric</th>
          <th scope="col" className={numericCellClassName}>
            A
          </th>
          <th scope="col" className={numericCellClassName}>
            B
          </th>
          <th scope="col" className={numericCellClassName}>
            difference
          </th>
        </tr>
      </thead>
      <tbody>
        {metrics.map((metric) => (
          <tr key={metric.id} data-metric-id={metric.id}>
            <th scope="row" className="font-mono text-xs font-medium">
              {metric.label}
            </th>
            <td data-label="A" className={numericCellClassName}>
              {formatPlanMetricValue(metric, 'baseline')}
            </td>
            <td data-label="B" className={numericCellClassName}>
              {formatPlanMetricValue(metric, 'candidate')}
            </td>
            <td
              data-label="Difference"
              className={`${numericCellClassName} text-ink-soft`}
            >
              {formatPlanMetricDelta(metric)}
            </td>
          </tr>
        ))}
      </tbody>
    </DataTable>
  )
}

function ScenarioDetail({
  scenario,
  comparison,
  sides,
  onTranscript,
}: {
  scenario: ScenarioComparison
  comparison: ExecutionComparison
  sides: Sides
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  return (
    <div className="grid min-w-0 gap-4">
      <MetricTable
        caption={`${scenario.id} metrics`}
        metrics={scenario.metrics}
      />
      {scenario.criteria.length > 0 ? (
        <section className="grid gap-2" aria-label="Criteria that changed">
          <h4 className="m-0 ds-label">criteria that changed</h4>
          <ul className="m-0 grid list-none gap-3 p-0">
            {scenario.criteria.map((criterion) => (
              <li
                key={criterion.key}
                className="grid gap-1"
                data-criterion={criterion.key}
              >
                <strong className="font-mono text-xs text-ink">
                  {criterion.label} · {formatPoints(criterion.a)} →{' '}
                  {formatPoints(criterion.b)} of {criterion.possible} (
                  {criterion.delta > 0 ? '+' : '−'}
                  {formatPoints(Math.abs(criterion.delta))})
                </strong>
                <span className="text-xs text-ink-muted">
                  A: {criterion.reasons.a.join(' · ') || 'no reason given'}
                </span>
                <span className="text-xs text-ink-muted">
                  B: {criterion.reasons.b.join(' · ') || 'no reason given'}
                </span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      <div className="grid min-w-0 gap-4 @[1000px]/harness:grid-cols-2">
        {(['a', 'b'] as const).map((which) => {
          const screenshots = screenshotsOf(sides[which], scenario.id)
          return (
            <section
              key={which}
              className="grid min-w-0 content-start gap-2"
              aria-label={`${which.toUpperCase()} evidence`}
              data-comparison-evidence={which}
            >
              <h4 className="m-0 ds-label">
                {which === 'a' ? 'A (base)' : 'B'} · {comparison[which].title}
              </h4>
              <ScenarioMatrix
                detail={scenarioDetail(sides[which], scenario.id)}
                onTranscript={onTranscript}
                showContract={false}
              />
              {screenshots.length > 0 ? (
                <ul
                  className="m-0 grid list-none gap-1 p-0 font-mono text-label text-ink-muted"
                  aria-label="Screenshots"
                >
                  {screenshots.map((screenshot) => (
                    <li key={screenshot.key}>
                      screenshot · {screenshot.caption} · {screenshot.mediaType}{' '}
                      ·{' '}
                      <a
                        href={hashForExecution(
                          sides[which].id,
                          null,
                          screenshot.runId,
                        )}
                      >
                        evidence record
                      </a>
                    </li>
                  ))}
                </ul>
              ) : null}
            </section>
          )
        })}
      </div>
    </div>
  )
}

function formatPoints(value: number) {
  return value.toLocaleString('en-US', { maximumFractionDigits: 1 })
}

/** The comparison itself, from two loaded executions. No side is labelled
 *  better or worse. */
export function ComparisonView({
  comparison,
  sides,
  onToggleCounted,
  onTranscript,
}: {
  comparison: ExecutionComparison
  sides: Sides
  onToggleCounted: (scenario: ScenarioComparison) => void
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const changes = [...comparison.parameters, ...comparison.stack]
  const unrecorded = (['a', 'b'] as const).filter(
    (which) => !comparison.stackRecorded[which],
  )
  const out = comparison.scenarios.filter((scenario) => !scenario.counted)
  const toggle = (set: Set<string>, id: string) => {
    const next = new Set(set)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    return next
  }
  return (
    <>
      <dl className="execution-identity" data-comparison-sides>
        {(['a', 'b'] as const).map((which) => (
          <div className="grid min-w-0 content-start gap-1" key={which}>
            <dt className="ds-label">{which === 'a' ? 'A (base)' : 'B'}</dt>
            <dd className="m-0 min-w-0 break-words text-ink">
              <a href={hashForExecution(comparison[which].id)}>
                {comparison[which].title}
              </a>
              <span className="block font-mono text-label text-ink-muted">
                {comparison[which].origin}
              </span>
            </dd>
          </div>
        ))}
      </dl>

      <section
        className="mt-6 grid min-w-0 gap-3"
        aria-labelledby="comparison-changes-heading"
        data-comparison-changes
      >
        <h2
          id="comparison-changes-heading"
          className="m-0 text-base font-semibold text-ink"
        >
          What changed
        </h2>
        {changes.length > 0 ? (
          <DataTable caption="Parameters and stack that differ" collapse>
            <thead>
              <tr>
                <th scope="col">field</th>
                <th scope="col">A</th>
                <th scope="col">B</th>
              </tr>
            </thead>
            <tbody>
              {changes.map((change) => (
                <tr key={change.field} data-change={change.field}>
                  <th scope="row" className="font-mono text-xs font-medium">
                    {change.field}
                  </th>
                  <td data-label="A" className="font-mono text-xs">
                    {change.a}
                  </td>
                  <td data-label="B" className="font-mono text-xs">
                    {change.b}
                  </td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        ) : (
          <p className="m-0 text-sm text-ink-muted">
            Same scenarios, runs, model, provider and profile
            {unrecorded.length === 0 ? ', and the same stack' : ''}.
          </p>
        )}
        {unrecorded.length > 0 ? (
          <p className="m-0 text-sm text-ink-muted">
            {`No stack recorded for ${unrecorded.map((which) => which.toUpperCase()).join(' and ')}; workers are not compared.`}
          </p>
        ) : null}
      </section>

      {out.length > 0 ? (
        <Callout className="mt-6" tone="info" title="Out of the totals">
          {out
            .map((scenario) => `${scenario.id} (${exclusionPhrase(scenario)})`)
            .join(', ')}
          . Their rows keep their values; count them again from the table.
        </Callout>
      ) : null}

      <section
        className="mt-6 grid min-w-0 gap-3"
        aria-labelledby="comparison-totals-heading"
      >
        <h2
          id="comparison-totals-heading"
          className="m-0 text-base font-semibold text-ink"
        >
          Totals
        </h2>
        <MetricTable caption="Totals" metrics={comparison.totals} />
      </section>

      <section
        className="mt-6 grid min-w-0 gap-3"
        aria-labelledby="comparison-scenarios-heading"
      >
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2
            id="comparison-scenarios-heading"
            className="m-0 text-base font-semibold text-ink"
          >
            By scenario
          </h2>
          <button
            type="button"
            className={buttonClassName({
              variant: 'secondary',
              size: 'compact',
            })}
            disabled={selected.size === 0}
            onClick={() =>
              requestRunAgain({
                executionId: comparison.b.id,
                scenarios: [...selected],
              })
            }
          >
            <RotateCcw size={13} aria-hidden="true" />
            rerun selected{selected.size > 0 ? ` (${selected.size})` : ''}
          </button>
        </div>
        <DataTable caption="Scenarios" collapse data-comparison-scenarios>
          <thead>
            <tr>
              <th scope="col">
                <span className="ds-visually-hidden">Select</span>
              </th>
              <th scope="col">scenario</th>
              <th scope="col" className={numericCellClassName}>
                score A
              </th>
              <th scope="col" className={numericCellClassName}>
                score B
              </th>
              <th scope="col" className={numericCellClassName}>
                difference
              </th>
              <th scope="col">criteria that changed</th>
              <th scope="col">totals</th>
            </tr>
          </thead>
          <tbody>
            {comparison.scenarios.map((scenario) => {
              const score = scenario.metrics[0]
              const open = expanded.has(scenario.id)
              const phrase = exclusionPhrase(scenario)
              return (
                <Fragment key={scenario.id}>
                  <tr
                    data-scenario={scenario.id}
                    data-counted={scenario.counted}
                  >
                    <td data-label="Select">
                      <input
                        type="checkbox"
                        aria-label={`Select ${scenario.id} to run again`}
                        checked={selected.has(scenario.id)}
                        onChange={() =>
                          setSelected((current) => toggle(current, scenario.id))
                        }
                      />
                    </td>
                    <th scope="row" className="font-normal">
                      <button
                        type="button"
                        className="inline-flex items-center gap-1 border-0 bg-transparent p-0 font-mono text-xs font-medium text-ink"
                        aria-expanded={open}
                        onClick={() =>
                          setExpanded((current) => toggle(current, scenario.id))
                        }
                      >
                        <ChevronDown
                          size={14}
                          className={open ? '' : '-rotate-90'}
                          aria-hidden="true"
                        />
                        {scenario.id}
                      </button>
                    </th>
                    <td data-label="Score A" className={numericCellClassName}>
                      {formatPlanMetricValue(score, 'baseline')}
                    </td>
                    <td data-label="Score B" className={numericCellClassName}>
                      {formatPlanMetricValue(score, 'candidate')}
                    </td>
                    <td
                      data-label="Difference"
                      className={`${numericCellClassName} text-ink-soft`}
                    >
                      {formatPlanMetricDelta(score)}
                    </td>
                    <td data-label="Criteria" className="text-xs">
                      {scenario.criteria.length > 0
                        ? scenario.criteria
                            .map(
                              (criterion) =>
                                `${criterion.delta > 0 ? '+' : '−'} ${criterion.label}`,
                            )
                            .join('; ')
                        : '—'}
                    </td>
                    <td data-label="Totals" className="text-xs">
                      <span className="block text-ink-muted">
                        {phrase ? `out · ${phrase}` : 'counted'}
                      </span>
                      <button
                        type="button"
                        className={buttonClassName({
                          variant: 'quiet',
                          size: 'compact',
                        })}
                        onClick={() => onToggleCounted(scenario)}
                      >
                        {scenario.counted ? 'leave out' : 'count'}
                      </button>
                    </td>
                  </tr>
                  {open ? (
                    <tr data-scenario-detail={scenario.id}>
                      <td colSpan={7}>
                        <ScenarioDetail
                          scenario={scenario}
                          comparison={comparison}
                          sides={sides}
                          onTranscript={onTranscript}
                        />
                      </td>
                    </tr>
                  ) : null}
                </Fragment>
              )
            })}
          </tbody>
        </DataTable>
      </section>
    </>
  )
}

export function ExecutionComparePage({
  left,
  right,
}: {
  left: string | null
  right: string | null
}) {
  const [sides, setSides] = useState<Sides | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [choice, setChoice] = useState<Choice>(() =>
    typeof window === 'undefined'
      ? { include: [], exclude: [] }
      : choiceFromParams(routeParams(window.location.hash)),
  )
  const [copied, setCopied] = useState<'summary' | 'link' | null>(null)
  const [transcript, setTranscript] = useState<{
    run: AssessmentRunView
    title: string
  } | null>(null)

  useEffect(() => {
    if (!left || !right) return
    let cancelled = false
    void (async () => {
      try {
        const bridge = await getDashboardDataBridge()
        const [a, b] = await Promise.all([
          bridge.getExecution(left),
          bridge.getExecution(right),
        ])
        if (!cancelled) setSides({ a, b })
      } catch (cause) {
        if (!cancelled)
          setError(cause instanceof Error ? cause.message : String(cause))
      }
    })()
    return () => {
      cancelled = true
    }
  }, [left, right])

  useEffect(() => {
    replaceRouteParams(choiceToParams(choice))
  }, [choice])

  const comparison = useMemo(
    () => (sides ? compareExecutions(sides.a, sides.b, choice) : null),
    [sides, choice],
  )

  const copy = (what: 'summary' | 'link', value: string) => {
    void navigator.clipboard?.writeText(value).then(() => {
      setCopied(what)
      window.setTimeout(() => setCopied(null), 1500)
    })
  }

  const shell = (children: ReactNode) => (
    <div className="ds-root min-h-dvh bg-canvas text-ink">
      <DashboardPageActions active="executions" context="compare" />
      <div className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        {children}
      </div>
    </div>
  )

  if (!left || !right)
    return shell(
      <EmptyState
        title="Choose two executions"
        description="Tick two executions in the list, then compare. The first one ticked is A, the base."
        actions={
          <a
            className={buttonClassName({
              variant: 'secondary',
              className: 'no-underline',
            })}
            href={hashForWorkspace('executions')}
          >
            back to executions
          </a>
        }
      />,
    )

  if (error)
    return shell(
      <EmptyState
        tone="error"
        title="The executions could not be loaded"
        description={error}
        actions={
          <a
            className={buttonClassName({
              variant: 'secondary',
              className: 'no-underline',
            })}
            href={hashForWorkspace('executions')}
          >
            back to executions
          </a>
        }
      />,
    )

  if (!sides || !comparison)
    return shell(
      <div className="grid gap-4" aria-busy="true" role="status">
        <span className="ds-visually-hidden">Loading both executions</span>
        {['sides', 'totals', 'scenarios'].map((placeholder) => (
          <div
            key={placeholder}
            className="h-32 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
          />
        ))}
      </div>,
    )

  const counted = comparison.scenarios.filter((scenario) => scenario.counted)
  return shell(
    <>
      <PageHeader
        title="compare executions"
        summary={`${counted.length} of ${comparison.scenarios.length} scenarios counted in the totals · differences are observations, not a verdict`}
        headingId="comparison-title"
        breadcrumb={[
          { label: 'executions', href: hashForWorkspace('executions') },
          { label: 'compare' },
        ]}
        actions={
          <>
            <a
              className={buttonClassName({
                variant: 'secondary',
                className: 'no-underline',
              })}
              href={hashWithParams(
                hashForComparison(right, left),
                choiceToParams(choice),
              )}
            >
              <ArrowLeftRight size={15} aria-hidden="true" />
              swap A and B
            </a>
            <button
              className={buttonClassName({ variant: 'secondary' })}
              type="button"
              onClick={() => copy('summary', comparisonMarkdown(comparison))}
            >
              <ClipboardCopy size={15} aria-hidden="true" />
              {copied === 'summary' ? 'summary copied' : 'copy summary'}
            </button>
            <button
              className={buttonClassName({ variant: 'quiet' })}
              type="button"
              onClick={() => copy('link', window.location.href)}
            >
              <Link2 size={13} aria-hidden="true" />
              {copied === 'link' ? 'link copied' : 'copy link'}
            </button>
          </>
        }
      />
      <ComparisonView
        comparison={comparison}
        sides={sides}
        onToggleCounted={(scenario) =>
          setChoice((current) => toggleCounted(current, scenario))
        }
        onTranscript={(run, title) => setTranscript({ run, title })}
      />
      {transcript ? (
        <TranscriptDialog
          title={transcript.title}
          messages={transcript.run.transcript?.messages}
          open
          onClose={() => setTranscript(null)}
        />
      ) : null}
    </>,
  )
}
