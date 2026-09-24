import {
  ArrowLeftRight,
  ChevronDown,
  ClipboardCopy,
  Link2,
  RotateCcw,
} from 'lucide-react'
import { Fragment, type ReactNode, useEffect, useMemo, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { DisclosureLayer } from '@/components/DisclosureLayer'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
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
  type DashboardDataBridge,
  type DashboardExecutionDetail,
  type ExecutionParameters,
  getDashboardDataBridge,
  type JsonObject,
} from '@/lib/dashboard-data-source'
import {
  type ComparedMetric,
  comparedValue,
  compareExecutions,
  comparisonMarkdown,
  type ExecutionComparison,
  exclusionPhrase,
  rerunPhrase,
  runnerWarning,
  type ScenarioComparison,
  type StackComparison,
  scenarioScore,
  stackSummary,
  yourCodeWorkers,
} from '@/lib/execution-comparison'
import { buildExecutionPresentation } from '@/lib/execution-view'
import { formatMetricDelta } from '@/lib/metric-comparison'
import { rerunParameters } from '@/pages/ExecutionPage'
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

export type ScreenshotEntry = {
  key: string
  /** The native execution whose report declares the deliverable. */
  executionId: string
  runId: string
  path: string
  pointer: string
  caption: string
}

/** Screenshots each run's deliverables name, as the report lists them. */
export function screenshotsOf(
  detail: DashboardExecutionDetail,
  scenarioId: string,
): ScreenshotEntry[] {
  return scenarioDetail(detail, scenarioId).reports.flatMap((record) => {
    const executionId =
      typeof record.native_execution_id === 'string' &&
      record.native_execution_id
        ? record.native_execution_id
        : detail.id
    return (record.report?.scenarios ?? []).flatMap((scenario) =>
      scenario.runs.flatMap((run) =>
        objects(run.deliverables).flatMap((deliverable) => {
          const path = objects([deliverable.artifact])[0]?.path
          if (typeof path !== 'string') return []
          return objects(deliverable.screenshots).map((screenshot) => ({
            key: `${executionId}:${run.run_id}:${path}:${screenshot.pointer}`,
            executionId,
            runId: run.run_id,
            path,
            pointer: String(screenshot.pointer ?? ''),
            caption: String(screenshot.caption ?? screenshot.pointer ?? ''),
          }))
        }),
      ),
    )
  })
}

/** A screenshot's bytes as a data URL, read through `e2e::dashboard::evidence-read`. */
export async function screenshotSource(
  read: DashboardDataBridge['readEvidence'],
  screenshot: ScreenshotEntry,
): Promise<string> {
  const file = await read({
    execution_id: screenshot.executionId,
    path: screenshot.path,
    pointer: screenshot.pointer,
  })
  return `data:${file.media_type};base64,${file.base64}`
}

type ScreenshotImage = { source: string } | { error: string } | undefined

export function ScreenshotFigure({
  screenshot,
  image,
  evidenceHref,
}: {
  screenshot: ScreenshotEntry
  image: ScreenshotImage
  evidenceHref: string
}) {
  return (
    <figure className="m-0 grid min-w-0 gap-1" data-screenshot={screenshot.key}>
      {image && 'source' in image ? (
        <img
          className="block h-auto w-full"
          src={image.source}
          alt={screenshot.caption}
          loading="lazy"
        />
      ) : (
        <span className="text-xs text-ink-muted" role="status">
          {image ? image.error : 'loading screenshot…'}
        </span>
      )}
      <figcaption className="font-mono text-label text-ink-muted">
        {screenshot.caption} · <a href={evidenceHref}>evidence record</a>
      </figcaption>
    </figure>
  )
}

/** The screenshots of one side, read when the scenario opens. */
function Screenshots({
  screenshots,
  hrefFor,
}: {
  screenshots: ScreenshotEntry[]
  hrefFor: (screenshot: ScreenshotEntry) => string
}) {
  const [images, setImages] = useState<Record<string, ScreenshotImage>>({})
  useEffect(() => {
    let cancelled = false
    void getDashboardDataBridge().then((bridge) => {
      for (const screenshot of screenshots)
        screenshotSource(bridge.readEvidence, screenshot)
          .then((source) => ({ source }))
          .catch((cause: unknown) => ({
            error: cause instanceof Error ? cause.message : String(cause),
          }))
          .then((image) => {
            if (!cancelled)
              setImages((current) => ({ ...current, [screenshot.key]: image }))
          })
    })
    return () => {
      cancelled = true
    }
  }, [screenshots])
  if (screenshots.length === 0) return null
  return (
    <section className="grid min-w-0 gap-3" aria-label="Screenshots">
      {screenshots.map((screenshot) => (
        <ScreenshotFigure
          key={screenshot.key}
          screenshot={screenshot}
          image={images[screenshot.key]}
          evidenceHref={hrefFor(screenshot)}
        />
      ))}
    </section>
  )
}

function MetricTable({
  caption,
  metrics,
}: {
  caption: string
  metrics: ComparedMetric[]
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
            <td className="font-mono text-xs font-medium text-ink">
              {metric.label}
            </td>
            <td data-label="A" className={numericCellClassName}>
              {comparedValue(metric, 'baseline')}
            </td>
            <td data-label="B" className={numericCellClassName}>
              {comparedValue(metric, 'candidate')}
            </td>
            <td
              data-label="Difference"
              className={`${numericCellClassName} text-ink-soft`}
            >
              {differenceText(metric)}
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
  // Stable per open scenario, so the screenshots are read once.
  const screenshots = useMemo(
    () => ({
      a: screenshotsOf(sides.a, scenario.id),
      b: screenshotsOf(sides.b, scenario.id),
    }),
    [sides, scenario.id],
  )
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
        {(['a', 'b'] as const).map((which) => (
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
            <Screenshots
              screenshots={screenshots[which]}
              hrefFor={(screenshot) =>
                hashForExecution(sides[which].id, null, screenshot.runId)
              }
            />
          </section>
        ))}
      </div>
    </div>
  )
}

/** A difference, or a dash where one side has no figure to take it from. */
function differenceText(metric: ComparedMetric) {
  return metric.delta === null ? '—' : formatMetricDelta(metric)
}

function formatPoints(value: number) {
  return value.toLocaleString('en-US', { maximumFractionDigits: 1 })
}

/** The stack in groups: your code, version differences, one side only. */
function StackDetail({ stack }: { stack: StackComparison }) {
  const rows: Array<[string, string]> = [
    ...stack.yourCode.map((group): [string, string] => [
      `your code in ${group.side.toUpperCase()} ${group.commit ? `@${group.commit}` : '(commit not recorded)'}${group.dirty ? ' (uncommitted changes)' : ''}`,
      yourCodeWorkers(group),
    ]),
    ...stack.versions.map((change): [string, string] => [
      `version · ${change.field}`,
      `${change.a} → ${change.b}`,
    ]),
    ...(stack.onlyA.length > 0
      ? [['only in A', stack.onlyA.join(', ')] as [string, string]]
      : []),
    ...(stack.onlyB.length > 0
      ? [['only in B', stack.onlyB.join(', ')] as [string, string]]
      : []),
  ]
  if (rows.length === 0)
    return <p className="m-0 text-sm text-ink-muted">Same stack.</p>
  return (
    <dl className="m-0 grid min-w-0 grid-cols-[max-content_minmax(0,1fr)] gap-x-6 gap-y-2 font-mono text-xs">
      {rows.map(([label, value]) => (
        <div key={label} className="contents" data-stack-group={label}>
          <dt className="ds-label">{label}</dt>
          <dd className="m-0 break-words text-ink">{value}</dd>
        </div>
      ))}
    </dl>
  )
}

/** The comparison itself, from two loaded executions. No side is labelled
 *  better or worse. */
export function ComparisonView({
  comparison,
  sides,
  onToggleCounted,
  onRunAgain,
  onTranscript,
}: {
  comparison: ExecutionComparison
  sides: Sides
  onToggleCounted: (scenario: ScenarioComparison) => void
  /** Run execution B again with only these scenarios. */
  onRunAgain: (scenarios: string[]) => void
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const { stack } = comparison
  const warning = runnerWarning(comparison.runner)
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

      {warning ? (
        <Callout className="mt-6" tone="warning" data-runner-warning>
          {warning}
        </Callout>
      ) : null}

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
        {comparison.parameters.length > 0 ? (
          <DataTable caption="Parameters that differ" collapse>
            <thead>
              <tr>
                <th scope="col">parameter</th>
                <th scope="col">A</th>
                <th scope="col">B</th>
              </tr>
            </thead>
            <tbody>
              {comparison.parameters.map((change) => (
                <tr key={change.field} data-change={change.field}>
                  <td className="font-mono text-xs font-medium text-ink">
                    {change.field}
                  </td>
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
            Same scenarios, runs, model, provider and profile.
          </p>
        )}
        <p className="m-0 font-mono text-xs text-ink" data-stack-summary>
          stack · {stackSummary(stack)}
        </p>
      </section>

      {out.length > 0 ? (
        <Callout className="mt-6" tone="info" title="Out of the totals">
          <ul className="m-0 grid list-none gap-1 p-0" data-out-of-totals>
            {out.map((scenario) => (
              <li key={scenario.id} className="break-words">
                <strong className="font-mono text-xs">{scenario.id}</strong> ·{' '}
                {exclusionPhrase(scenario)}
              </li>
            ))}
          </ul>
          Their rows keep their values; count them again from the table.
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

      {stack.recorded.a && stack.recorded.b ? (
        <div className="mt-6">
          <DisclosureLayer
            id="comparison-stack"
            label="stack"
            scent={stackSummary(stack)}
            open={false}
          >
            <StackDetail stack={stack} />
          </DisclosureLayer>
        </div>
      ) : null}

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
            onClick={() => onRunAgain([...selected].sort())}
          >
            <RotateCcw size={13} aria-hidden="true" />
            rerun selected{selected.size > 0 ? ` (${selected.size})` : ''}
          </button>
        </div>
        <DataTable caption="Scenarios" collapse data-comparison-scenarios>
          <thead>
            <tr>
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
              const reran = rerunPhrase(scenario)
              return (
                <Fragment key={scenario.id}>
                  <tr
                    data-scenario={scenario.id}
                    data-counted={scenario.counted}
                  >
                    <td>
                      {/* Checkbox and name share the cell, so they share the line. */}
                      <span className="flex items-center gap-2">
                        <input
                          type="checkbox"
                          aria-label={`Select ${scenario.id} to run again`}
                          checked={selected.has(scenario.id)}
                          onChange={() =>
                            setSelected((current) =>
                              toggle(current, scenario.id),
                            )
                          }
                        />
                        <button
                          type="button"
                          className="inline-flex items-center gap-1 border-0 bg-transparent p-0 font-mono text-xs font-medium text-ink"
                          aria-expanded={open}
                          onClick={() =>
                            setExpanded((current) =>
                              toggle(current, scenario.id),
                            )
                          }
                        >
                          <ChevronDown
                            size={14}
                            className={open ? '' : '-rotate-90'}
                            aria-hidden="true"
                          />
                          {scenario.id}
                        </button>
                        {reran ? (
                          <span
                            className="font-mono text-label text-warning"
                            data-reruns={reran}
                            title="Only the last attempt is compared"
                          >
                            {reran}
                          </span>
                        ) : null}
                      </span>
                    </td>
                    <td data-label="Score A" className={numericCellClassName}>
                      {scenarioScore(scenario, 'a')}
                    </td>
                    <td data-label="Score B" className={numericCellClassName}>
                      {scenarioScore(scenario, 'b')}
                    </td>
                    <td
                      data-label="Difference"
                      className={`${numericCellClassName} text-ink-soft`}
                    >
                      {differenceText(score)}
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
                      <span className="block break-words text-ink-muted">
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
                      <td colSpan={6}>
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

/** Both executions, or an error that names the side that failed. */
export async function loadExecutionPair(
  getExecution: (id: string) => Promise<DashboardExecutionDetail>,
  left: string,
  right: string,
): Promise<Sides> {
  const [a, b] = await Promise.allSettled([
    getExecution(left),
    getExecution(right),
  ])
  if (a.status === 'fulfilled' && b.status === 'fulfilled')
    return { a: a.value, b: b.value }
  const reason = (result: PromiseSettledResult<unknown>) =>
    result.status === 'rejected'
      ? result.reason instanceof Error
        ? result.reason.message
        : String(result.reason)
      : null
  throw new Error(
    (
      [
        ['A', left, reason(a)],
        ['B', right, reason(b)],
      ] as const
    )
      .flatMap(([side, id, message]) =>
        message === null
          ? []
          : [`${side} (${id}) could not be loaded: ${message}`],
      )
      .join(' · '),
  )
}

/** What the page shows before a comparison: a choice to make, an error or
 *  the loading skeleton. */
export function ComparisonPlaceholder({
  missing,
  error,
}: {
  missing: boolean
  error: string | null
}) {
  const back = (
    <a
      className={buttonClassName({
        variant: 'secondary',
        className: 'no-underline',
      })}
      href={hashForWorkspace('executions')}
    >
      back to executions
    </a>
  )
  if (missing)
    return (
      <EmptyState
        title="Choose two executions"
        description="Tick two executions in the list, then compare. The first one ticked is A, the base."
        actions={back}
      />
    )
  if (error)
    return (
      <EmptyState
        tone="error"
        title="The comparison could not be loaded"
        description={error}
        actions={back}
      />
    )
  return (
    <div className="grid gap-4" aria-busy="true" role="status">
      <span className="ds-visually-hidden">Loading both executions</span>
      {['sides', 'totals', 'scenarios'].map((placeholder) => (
        <div
          key={placeholder}
          className="h-32 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
        />
      ))}
    </div>
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
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  // Run again for B: its parameters with only the ticked scenarios.
  const [rerun, setRerun] = useState<{
    parameters: ExecutionParameters
    scenarios: string[]
  } | null>(null)

  useEffect(() => {
    if (!left || !right) return
    let cancelled = false
    void (async () => {
      try {
        const bridge = await getDashboardDataBridge()
        if (!cancelled) setBridge(bridge)
        const pair = await loadExecutionPair(
          (id) => bridge.getExecution(id),
          left,
          right,
        )
        if (!cancelled) setSides(pair)
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

  if (!left || !right || error || !sides || !comparison)
    return shell(
      <ComparisonPlaceholder missing={!left || !right} error={error} />,
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
        onRunAgain={(scenarios) =>
          setRerun({
            parameters: rerunParameters(
              sides.b,
              sides.b.reports.map((record) => record.scenario_id),
              buildExecutionPresentation(sides.b).subjects[0],
            ),
            scenarios,
          })
        }
        onTranscript={(run, title) => setTranscript({ run, title })}
      />
      <LocalRunnerDialog
        bridge={bridge}
        open={rerun !== null}
        parameters={rerun?.parameters ?? null}
        initialScenarios={rerun?.scenarios}
        label={sides.b.plan_execution?.label ?? sides.b.label ?? ''}
        onClose={() => setRerun(null)}
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
