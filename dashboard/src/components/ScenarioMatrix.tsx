import { ChevronDown } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useState } from 'react'
import { AssessmentWorkspace } from '@/components/AssessmentWorkspace'
import { SemanticTestFlow } from '@/components/SemanticTestFlow'
import { type OperationalStatus, StatusBadge } from '@/design-system'
import type { AssessmentRunView } from '@/lib/assessment-view'
import type {
  DashboardExecutionDetail,
  SemanticTestReport,
} from '@/lib/dashboard-data-source'
import { titleCase } from '@/lib/execution-view'
import {
  buildScenarioMatrix,
  detailForScenario,
  formatScenarioDuration,
  type ScenarioMatrixItem,
  stepSignals,
} from '@/lib/scenario-matrix'

export function ScenarioMatrix({
  detail,
  onTranscript,
}: {
  detail: DashboardExecutionDetail
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  const model = useMemo(() => buildScenarioMatrix(detail), [detail])
  const preferredItem =
    model.items.find((item) => item.workflowSteps.length > 0) ??
    model.items.find((item) => item.objective.status !== 'passed') ??
    model.items[0]
  const [expandedKey, setExpandedKey] = useState<string | null>(
    preferredItem?.key ?? null,
  )

  useEffect(() => {
    if (expandedKey && !model.items.some((item) => item.key === expandedKey)) {
      setExpandedKey(preferredItem?.key ?? null)
    }
  }, [expandedKey, model.items, preferredItem?.key])

  if (model.items.length === 0) {
    return (
      <div className="rounded-[var(--ds-radius-sm)] border border-dashed border-[var(--ds-color-line-strong)] bg-[var(--ds-color-surface-raised)] p-5 text-sm text-[var(--ds-color-ink-muted)]">
        No scenario reports were retained for this execution.
      </div>
    )
  }

  return (
    <div className="grid gap-4">
      <ScenarioSummary summary={model.summary} />
      <div className="overflow-hidden rounded-[var(--ds-radius-md)] border border-[var(--ds-color-line-strong)] bg-[var(--ds-color-surface)]">
        <div
          className="hidden grid-cols-[minmax(13rem,1.2fr)_minmax(9rem,0.75fr)_minmax(10rem,1fr)_minmax(7rem,0.55fr)_minmax(8rem,0.7fr)] gap-4 border-b border-[var(--ds-color-line)] bg-[var(--ds-color-surface-raised)] px-5 py-3 font-mono text-[0.61rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)] lg:grid"
          aria-hidden="true"
        >
          <span>Scenario</span>
          <span>Objective result</span>
          <span>Advisory</span>
          <span>Runtime</span>
          <span>Structure</span>
        </div>
        <ol className="m-0 grid list-none p-0">
          {model.items.map((item) => {
            const expanded = expandedKey === item.key
            return (
              <ScenarioRow
                key={item.key}
                item={item}
                detail={detail}
                expanded={expanded}
                onToggle={() => setExpandedKey(expanded ? null : item.key)}
                onTranscript={onTranscript}
              />
            )
          })}
        </ol>
      </div>
    </div>
  )
}

function ScenarioSummary({
  summary,
}: {
  summary: ReturnType<typeof buildScenarioMatrix>['summary']
}) {
  const entries: Array<{
    status: OperationalStatus
    count: number
    label: string
  }> = [
    { status: 'passed', count: summary.passed, label: 'passed' },
    { status: 'hard_gate', count: summary.hardGate, label: 'hard gate' },
    { status: 'failed', count: summary.failed, label: 'failed' },
    {
      status: 'inconclusive',
      count: summary.inconclusive,
      label: 'inconclusive',
    },
    {
      status: 'unavailable',
      count: summary.unavailable,
      label: 'unavailable',
    },
    { status: 'running', count: summary.running, label: 'running' },
    {
      status: 'incomplete',
      count: summary.incomplete,
      label: 'incomplete',
    },
  ]

  return (
    <section
      className="flex flex-wrap items-center gap-x-5 gap-y-2 border-y border-[var(--ds-color-line)] py-3"
      aria-label="Scenario result summary"
    >
      <strong className="font-mono text-xs font-semibold text-[var(--ds-color-ink)]">
        {summary.total} {summary.total === 1 ? 'scenario' : 'scenarios'}
      </strong>
      {entries
        .filter((entry) => entry.count > 0)
        .map((entry) => (
          <StatusBadge
            key={entry.status}
            status={entry.status}
            label={`${entry.count} ${entry.label}`}
          />
        ))}
    </section>
  )
}

function ScenarioRow({
  item,
  detail,
  expanded,
  onToggle,
  onTranscript,
}: {
  item: ScenarioMatrixItem
  detail: DashboardExecutionDetail
  expanded: boolean
  onToggle: () => void
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  const panelId = `${safeId(item.key)}-scenario-panel`
  const structure = item.workflowSteps.length
    ? `Workflow · ${item.workflowSteps.length} ${item.workflowSteps.length === 1 ? 'step' : 'steps'}`
    : item.available
      ? 'Standard'
      : 'No report'

  return (
    <li className="border-t border-[var(--ds-color-line)] first:border-t-0">
      <button
        data-scenario-row={item.key}
        className={`group grid min-h-16 w-full min-w-0 gap-x-4 gap-y-3 border-0 px-4 py-4 text-left transition-colors motion-reduce:transition-none lg:grid-cols-[minmax(13rem,1.2fr)_minmax(9rem,0.75fr)_minmax(10rem,1fr)_minmax(7rem,0.55fr)_minmax(8rem,0.7fr)] lg:items-center lg:px-5 ${
          expanded
            ? 'bg-[var(--ds-color-surface-raised)]'
            : 'bg-[var(--ds-color-surface)] hover:bg-[var(--ds-color-surface-raised)]'
        }`}
        type="button"
        aria-expanded={expanded}
        aria-controls={panelId}
        onClick={onToggle}
      >
        <span className="flex min-w-0 items-start gap-3">
          <ChevronDown
            className={`mt-0.5 size-4 shrink-0 text-[var(--ds-color-ink-muted)] transition-transform duration-[var(--ds-motion-duration-fast)] motion-reduce:transition-none ${expanded ? 'rotate-0' : '-rotate-90'}`}
            aria-hidden="true"
          />
          <span className="min-w-0">
            <strong className="block truncate text-sm font-semibold text-[var(--ds-color-ink)]">
              {titleCase(item.scenarioId)}
              {item.scenarioVersion != null ? ` v${item.scenarioVersion}` : ''}
            </strong>
            <span className="mt-1 block truncate font-mono text-[0.61rem] text-[var(--ds-color-ink-muted)]">
              {item.subjectId} · {item.runCount}{' '}
              {item.runCount === 1 ? 'run' : 'runs'}
            </span>
          </span>
        </span>
        <MatrixCell label="Objective result">
          <StatusBadge
            status={item.objective.status}
            label={item.objective.label}
          />
        </MatrixCell>
        <MatrixCell label="Advisory">
          <StatusBadge
            status={item.advisory.status}
            label={item.advisory.label}
          />
        </MatrixCell>
        <MatrixCell label="Runtime">
          <strong className="font-mono text-xs font-semibold tabular-nums text-[var(--ds-color-ink)]">
            {formatScenarioDuration(item.durationMs)}
          </strong>
          {item.durationKind === 'average' ? (
            <span className="ml-1 font-mono text-[0.58rem] text-[var(--ds-color-ink-muted)]">
              avg
            </span>
          ) : null}
        </MatrixCell>
        <MatrixCell label="Structure">
          <span className="text-xs font-medium text-[var(--ds-color-ink-soft)]">
            {structure}
          </span>
        </MatrixCell>
      </button>
      {expanded ? (
        <ScenarioExpansion
          id={panelId}
          detail={detailForScenario(detail, item)}
          item={item}
          onTranscript={onTranscript}
        />
      ) : null}
    </li>
  )
}

function MatrixCell({
  label,
  children,
}: {
  label: string
  children: ReactNode
}) {
  return (
    <span className="grid min-w-0 grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 lg:block">
      <span className="font-mono text-[0.58rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)] lg:hidden">
        {label}
      </span>
      <span className="min-w-0">{children}</span>
    </span>
  )
}

function ScenarioExpansion({
  id,
  detail,
  item,
  onTranscript,
}: {
  id: string
  detail: DashboardExecutionDetail
  item: ScenarioMatrixItem
  onTranscript: (run: AssessmentRunView, title: string) => void
}) {
  return (
    <section
      id={id}
      className="border-t border-[var(--ds-color-line-strong)] bg-[var(--ds-color-surface-raised)] p-3 sm:p-4 md:p-5"
      aria-label={`${titleCase(item.scenarioId)} scenario result`}
    >
      <div className="overflow-hidden rounded-[var(--ds-radius-sm)] border border-[var(--ds-color-line)] bg-[var(--ds-color-surface)]">
        <ScenarioResultBand item={item} />
        {!item.available ? (
          <div className="border-t border-[var(--ds-color-line)] px-4 py-6 text-sm leading-6 text-[var(--ds-color-ink-muted)] md:px-5">
            The expected report for this scenario is unavailable. Runtime,
            workflow, and advisory data are intentionally not inferred.
          </div>
        ) : item.workflowSteps.length > 0 ? (
          <WorkflowDurationProfile tests={item.workflowSteps} />
        ) : null}
        {item.available ? (
          <details className="group border-t border-[var(--ds-color-line)]">
            <summary className="flex min-h-12 cursor-pointer list-none items-center justify-between gap-4 px-4 py-3 text-sm font-semibold text-[var(--ds-color-ink)] marker:hidden md:px-5">
              <span>Inspect scenario evidence</span>
              <span className="font-mono text-[0.61rem] font-normal text-[var(--ds-color-ink-muted)]">
                assessments, gates, artifacts, and provenance
              </span>
            </summary>
            <div className="grid gap-6 border-t border-[var(--ds-color-line)] p-4 md:p-5">
              <AssessmentWorkspace
                detail={detail}
                onTranscript={onTranscript}
              />
              {item.workflowSteps.length > 0 ? (
                <SemanticTestFlow detail={detail} />
              ) : null}
            </div>
          </details>
        ) : null}
      </div>
    </section>
  )
}

function ScenarioResultBand({ item }: { item: ScenarioMatrixItem }) {
  return (
    <dl
      className="m-0 grid grid-cols-2 gap-px bg-[var(--ds-color-line)] lg:grid-cols-4 xl:grid-cols-7"
      data-scenario-primary-metrics
    >
      <ResultFact
        label="Objective"
        value={item.objective.label}
        status={item.objective.status}
        detail="Authoritative system result"
      />
      <ResultFact
        label="Advisory"
        value={item.advisory.label}
        status={item.advisory.status}
        detail="Does not override objective result"
      />
      {item.primaryMetrics.map((metric) => (
        <ResultFact
          key={metric.label}
          label={metric.label}
          value={metric.value}
          detail={metric.detail}
        />
      ))}
    </dl>
  )
}

function ResultFact({
  label,
  value,
  detail,
  status,
}: {
  label: string
  value: string
  detail: string
  status?: OperationalStatus
}) {
  return (
    <div
      className="min-w-0 bg-[var(--ds-color-surface)] p-3 md:p-4"
      data-primary-metric={label}
    >
      <dt className="font-mono text-[0.58rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)]">
        {label}
      </dt>
      <dd className="m-0 mt-2 min-w-0">
        {status ? (
          <StatusBadge status={status} label={value} />
        ) : (
          <strong className="block truncate font-mono text-sm font-semibold tabular-nums text-[var(--ds-color-ink)]">
            {value}
          </strong>
        )}
        <span className="mt-1 block text-[0.66rem] leading-4 text-[var(--ds-color-ink-muted)]">
          {detail}
        </span>
      </dd>
    </div>
  )
}

function WorkflowDurationProfile({ tests }: { tests: SemanticTestReport[] }) {
  const maxDuration = Math.max(...tests.map((test) => test.duration_ms), 1)
  const totalDuration = tests.reduce(
    (total, test) => total + Math.max(test.duration_ms, 0),
    0,
  )

  return (
    <section
      className="border-t border-[var(--ds-color-line)]"
      aria-labelledby="workflow-duration-heading"
    >
      <header className="grid gap-2 border-b border-[var(--ds-color-line)] px-4 py-4 md:grid-cols-[minmax(0,1fr)_auto] md:items-end md:px-5">
        <div>
          <h3
            id="workflow-duration-heading"
            className="m-0 text-sm font-semibold text-[var(--ds-color-ink)]"
          >
            Workflow duration profile
          </h3>
          <p className="m-0 mt-1 text-xs leading-5 text-[var(--ds-color-ink-muted)]">
            Bars compare recorded step duration. They do not infer parallel
            timing that is absent from the report.
          </p>
        </div>
        <span className="font-mono text-[0.64rem] text-[var(--ds-color-ink-muted)]">
          {formatScenarioDuration(totalDuration)} recorded step time
        </span>
      </header>
      <div className="hidden grid-cols-[minmax(12rem,0.9fr)_7rem_minmax(14rem,1.4fr)_minmax(12rem,1fr)] gap-4 border-b border-[var(--ds-color-line)] bg-[var(--ds-color-surface-raised)] px-5 py-2.5 font-mono text-[0.58rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)] lg:grid">
        <span>Step</span>
        <span>Duration</span>
        <span>Duration profile</span>
        <span>Step metrics</span>
      </div>
      <ol className="m-0 grid list-none p-0">
        {tests.map((test, index) => (
          <WorkflowStepRow
            key={test.node_id}
            test={test}
            index={index}
            maxDuration={maxDuration}
            totalDuration={totalDuration}
          />
        ))}
      </ol>
    </section>
  )
}

function WorkflowStepRow({
  test,
  index,
  maxDuration,
  totalDuration,
}: {
  test: SemanticTestReport
  index: number
  maxDuration: number
  totalDuration: number
}) {
  const status = workflowStepStatus(test.status)
  const metrics = stepSignals(test)
  const width = Math.max(
    1.5,
    (Math.max(test.duration_ms, 0) / maxDuration) * 100,
  )
  const share =
    totalDuration > 0
      ? (Math.max(test.duration_ms, 0) / totalDuration) * 100
      : 0
  const dependencies = test.dependencies.length
    ? `After ${test.dependencies.map(titleCase).join(', ')}`
    : 'Starts workflow'
  const barTone =
    status.status === 'passed'
      ? 'bg-[var(--ds-color-brand)]'
      : status.status === 'hard_gate' || status.status === 'failed'
        ? 'bg-[var(--ds-color-danger)]'
        : 'bg-[var(--ds-color-warning)]'

  return (
    <li
      className="grid gap-3 border-t border-[var(--ds-color-line)] px-4 py-4 first:border-t-0 lg:grid-cols-[minmax(12rem,0.9fr)_7rem_minmax(14rem,1.4fr)_minmax(12rem,1fr)] lg:items-center lg:gap-4 lg:px-5"
      data-workflow-step={test.node_id}
    >
      <div className="flex min-w-0 items-start gap-3">
        <span className="grid size-7 shrink-0 place-items-center rounded-md border border-[var(--ds-color-line)] bg-[var(--ds-color-surface-raised)] font-mono text-[0.61rem] font-semibold text-[var(--ds-color-ink-muted)]">
          {String(index + 1).padStart(2, '0')}
        </span>
        <div className="min-w-0">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <strong className="truncate text-sm text-[var(--ds-color-ink)]">
              {titleCase(test.node_id)}
            </strong>
            <StatusBadge
              className="[&_.ds-status-dot]:size-1.5"
              status={status.status}
              label={status.label}
            />
          </div>
          <span className="mt-1 block truncate text-[0.65rem] text-[var(--ds-color-ink-muted)]">
            {dependencies}
          </span>
        </div>
      </div>
      <div className="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 lg:block">
        <span className="font-mono text-[0.58rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)] lg:hidden">
          Duration
        </span>
        <strong className="font-mono text-xs font-semibold tabular-nums text-[var(--ds-color-ink)]">
          {formatScenarioDuration(test.duration_ms)}
        </strong>
      </div>
      <div className="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 lg:block">
        <span className="font-mono text-[0.58rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)] lg:hidden">
          Profile
        </span>
        <div>
          <div className="h-2 overflow-hidden rounded-full bg-[var(--ds-color-surface-strong)]">
            <div
              className={`h-full rounded-full ${barTone}`}
              style={{ width: `${width}%` }}
            />
          </div>
          <span className="mt-1 block font-mono text-[0.59rem] tabular-nums text-[var(--ds-color-ink-muted)]">
            {share.toFixed(share >= 10 ? 0 : 1)}% of recorded step time
          </span>
        </div>
      </div>
      <div className="grid grid-cols-[7rem_minmax(0,1fr)] items-start gap-3 lg:block">
        <span className="font-mono text-[0.58rem] font-semibold uppercase tracking-[0.06em] text-[var(--ds-color-ink-muted)] lg:hidden">
          Metrics
        </span>
        <div
          className="flex min-w-0 flex-wrap gap-1.5"
          data-step-metrics={test.node_id}
        >
          {metrics.map((metric) => (
            <span
              key={metric.label}
              className="inline-flex min-w-0 items-baseline gap-1 rounded-md border border-[var(--ds-color-line)] bg-[var(--ds-color-surface-raised)] px-2 py-1 font-mono text-[0.6rem] text-[var(--ds-color-ink-muted)]"
              data-step-metric={metric.label}
            >
              <span className="truncate">{metric.label}</span>
              <strong className="shrink-0 text-[var(--ds-color-ink-soft)]">
                {metric.value}
              </strong>
            </span>
          ))}
        </div>
      </div>
    </li>
  )
}

function workflowStepStatus(statusValue: string): {
  status: OperationalStatus
  label: string
} {
  const status = statusValue.toLowerCase()
  if (status === 'succeeded') return { status: 'passed', label: 'Succeeded' }
  if (status === 'hard_gate_failed') {
    return { status: 'hard_gate', label: 'Hard gate failed' }
  }
  if (status === 'failed') return { status: 'failed', label: 'Failed' }
  if (status === 'running') return { status: 'running', label: 'Running' }
  if (status === 'cancelled') {
    return { status: 'cancelled', label: 'Cancelled' }
  }
  if (status === 'pending' || status === 'skipped') {
    return { status: 'incomplete', label: titleCase(status) }
  }
  return { status: 'unavailable', label: titleCase(status) }
}

function safeId(value: string) {
  return value.replace(/[^a-zA-Z0-9_-]/g, '-')
}
