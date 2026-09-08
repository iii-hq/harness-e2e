import {
  AlertTriangle,
  CheckCircle2,
  MessageCircle,
  ShieldCheck,
} from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { TestCriteriaList } from '@/components/AboutTestPanel'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import { SystemOutcomeBadge } from '@/components/SystemOutcome'
import {
  buttonClassName,
  Callout,
  Dialog,
  type OperationalStatus,
} from '@/design-system'
import type {
  AssessmentOutcome,
  EvidenceReference,
} from '@/lib/assessment-contract'
import {
  type AssessmentEntry,
  type AssessmentFilter,
  type AssessmentRunView,
  type AssessmentWorkspaceModel,
  assessmentFilterCounts,
  buildAssessmentWorkspace,
  buildHarnessRecommendation,
  matchesAssessmentFilter,
} from '@/lib/assessment-view'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { formatDuration } from '@/lib/execution-view'
import type { TestCriterion, TestSpec } from '@/lib/test-catalog'

const FILTERS: Array<{ id: AssessmentFilter; label: string }> = [
  { id: 'all', label: 'All' },
  { id: 'failed', label: 'Failed' },
  { id: 'unavailable', label: 'Unavailable' },
  { id: 'asset', label: 'Asset-related' },
]

function titleCase(value: string) {
  return value
    .replaceAll('_', ' ')
    .replaceAll('-', ' ')
    .replace(/\b\w/g, (letter) => letter.toUpperCase())
}

function formatMetricCount(value: number | null) {
  return value == null
    ? 'Not reported'
    : Math.round(value).toLocaleString('en-US')
}

function formatRunDuration(durationMs: number | null) {
  return formatDuration(durationMs == null ? null : durationMs / 1000)
}

function RunMetricCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-line bg-panel-subtle p-3">
      <small className="block text-label font-semibold uppercase tracking-[0.06em] text-ink-muted">
        {label}
      </small>
      <strong className="mt-1 block text-sm font-semibold text-ink">
        {value}
      </strong>
    </div>
  )
}

type PrimaryMetricTone =
  | 'positive'
  | 'warning'
  | 'negative'
  | 'neutral'
  | 'unavailable'

type PrimaryMetric = {
  label: string
  value: string
  detail: string
  context: 'Objective' | 'Signal' | 'Observed'
  tone: PrimaryMetricTone
}

// Audit AW-05: tone lives on the value only. Translucent cell fills let the
// hairline grid bleed through and read as a grey block in the console.
const PRIMARY_METRIC_TONES: Record<PrimaryMetricTone, string> = {
  positive: '[&_[data-metric-value]]:text-success',
  warning: '[&_[data-metric-value]]:text-warning',
  negative: '[&_[data-metric-value]]:text-danger',
  neutral: '[&_[data-metric-value]]:text-ink',
  unavailable: '[&_[data-metric-value]]:text-ink-muted',
}

/**
 * Audit AW-01: evidence chips used to be `#technical` anchors. In the console
 * that rewrote the route hash and left the modal open. The chip now closes any
 * open dialog and brings the technical section into view.
 */
export function revealTechnicalSection() {
  if (typeof document === 'undefined') return
  for (const dialog of document.querySelectorAll<HTMLDialogElement>(
    'dialog[open]',
  )) {
    dialog.close()
  }
  // Audit ED-26: provenance is a closed layer on the execution page; opening
  // it here fires its toggle event, so the page's state follows.
  const technical = document.getElementById('technical')
  if (technical instanceof HTMLDetailsElement) technical.open = true
  technical?.scrollIntoView({ block: 'start' })
}

function metricRatio(entry: AssessmentEntry | undefined) {
  if (!entry) return 'Not reported'
  const ratio = entry.summary.match(/\b(\d+)\s+of\s+(\d+)\b/i)
  if (ratio) return `${ratio[1]}/${ratio[2]}`
  if (entry.score) return `${entry.score.awarded}/${entry.score.possible}`
  return titleCase(entry.outcome)
}

function scorePercent(entry: AssessmentEntry | undefined) {
  if (!entry?.score || entry.score.possible <= 0) return null
  return Math.round((entry.score.awarded / entry.score.possible) * 100)
}

function metricToneForEntry(
  entry: AssessmentEntry | undefined,
): PrimaryMetricTone {
  if (!entry) return 'unavailable'
  if (entry.outcome === 'passed') return 'positive'
  if (entry.outcome === 'partial') return 'warning'
  if (entry.outcome === 'failed' || entry.outcome === 'error') return 'negative'
  return 'neutral'
}

/** An outcome nobody reached is not a verdict. A run that died before
 *  evaluation has to read that way, not as a subject that failed its gates. */
export function isEvaluated(outcome: AssessmentOutcome): boolean {
  return outcome !== 'not_evaluated' && outcome !== 'unavailable'
}

function primaryRunMetrics(run: AssessmentRunView): PrimaryMetric[] {
  const evaluatedAssessments = run.assessments.filter((entry) =>
    isEvaluated(entry.outcome),
  ).length
  const detection = run.assessments.find((entry) =>
    entry.criterionId.includes('seeded_vulnerability_detection'),
  )
  const patchApplicability = run.assessments.find((entry) =>
    entry.criterionId.includes('suggested_patch_applicability'),
  )
  const passedAssessments = run.assessments.filter(
    (entry) => entry.outcome === 'passed',
  ).length
  // Audit AW-04: a run with no retained assessments is unavailable, not
  // "0/0 passed".
  const assessmentTone: PrimaryMetricTone =
    run.assessments.length === 0 || evaluatedAssessments === 0
      ? 'unavailable'
      : passedAssessments === run.assessments.length
        ? 'positive'
        : 'warning'

  return [
    {
      label: 'Objective score',
      value:
        run.objectiveScore === null
          ? 'Not reported'
          : `${run.objectiveScore}/100`,
      detail:
        run.objectiveScore === null
          ? 'No objective score retained'
          : 'Official score from measured criteria',
      context: 'Objective',
      tone:
        run.objectiveScore === null
          ? 'unavailable'
          : run.objectiveScore === 100
            ? 'positive'
            : 'warning',
    },
    detection
      ? {
          label: 'Seeded detection',
          value: metricRatio(detection),
          detail: `${scorePercent(detection) ?? '—'}% of the possible score`,
          context: 'Signal',
          tone: metricToneForEntry(detection),
        }
      : {
          label: 'Assessment outcomes',
          value:
            run.assessments.length > 0
              ? `${passedAssessments}/${run.assessments.length}`
              : 'Not reported',
          // Audit AW-11: same distinction as the gates — "needs review" is a
          // verdict on work that exists, "not evaluated" is the absence of one.
          detail:
            run.assessments.length === 0
              ? 'No assessments retained'
              : evaluatedAssessments === 0
                ? `${run.assessments.length} not evaluated`
                : `${evaluatedAssessments - passedAssessments} need review`,
          context: 'Observed',
          tone: assessmentTone,
        },
    patchApplicability
      ? {
          label: 'Optional patch checks',
          value: metricRatio(patchApplicability),
          detail: `${scorePercent(patchApplicability) ?? '—'}% applied cleanly`,
          context: 'Signal',
          tone: metricToneForEntry(patchApplicability),
        }
      : {
          label: 'Runtime',
          value: formatRunDuration(run.metrics.durationMs),
          detail: 'Subject execution time',
          context: 'Observed',
          tone: run.metrics.durationMs == null ? 'unavailable' : 'neutral',
        },
    {
      label: 'Subject tokens',
      value: formatMetricCount(run.metrics.totalTokens),
      detail: 'Total tokens the subject consumed',
      context: 'Observed',
      tone: run.metrics.totalTokens == null ? 'unavailable' : 'neutral',
    },
  ]
}

function PrimaryMetricBoard({
  run,
  standalone = false,
}: {
  run: AssessmentRunView
  standalone?: boolean
}) {
  return (
    <section
      className={`grid grid-flow-dense grid-cols-1 gap-px overflow-hidden bg-line sm:grid-cols-2 lg:grid-cols-4 ${
        standalone ? 'rounded-lg border border-line' : 'border-y border-line'
      }`}
      aria-label={`${titleCase(run.scenarioId)} primary metrics`}
      data-primary-run-metrics
    >
      {primaryRunMetrics(run).map((metric) => (
        <article
          key={metric.label}
          className={`grid min-h-36 content-between gap-5 bg-panel p-4 ${PRIMARY_METRIC_TONES[metric.tone]}`}
        >
          <div className="flex items-start justify-between gap-3">
            <h5 className="m-0 text-label font-semibold uppercase tracking-[0.06em] text-ink-muted">
              {metric.label}
            </h5>
            <span className="shrink-0 font-mono text-label uppercase tracking-[0.05em] text-ink-muted">
              {metric.context}
            </span>
          </div>
          <div>
            <strong
              className="block font-mono text-[1.375rem] font-semibold leading-tight tracking-[-0.01em] tabular-nums"
              data-metric-value
            >
              {metric.value}
            </strong>
            <p className="mt-2 mb-0 text-xs leading-5 text-ink-muted">
              {metric.detail}
            </p>
          </div>
        </article>
      ))}
    </section>
  )
}

function shortHash(value: string) {
  return value.length > 24 ? `${value.slice(0, 18)}…${value.slice(-6)}` : value
}

function safeId(value: string) {
  return value.replace(/[^a-zA-Z0-9_-]/g, '-')
}

function toneForOutcome(outcome: string) {
  if (outcome === 'passed' || outcome === 'valid') {
    return 'border-success/30 bg-success/5 text-success'
  }
  if (
    outcome === 'failed' ||
    outcome === 'error' ||
    outcome === 'resource_limit' ||
    outcome.endsWith('_failed') ||
    outcome.endsWith('_error')
  ) {
    return 'border-danger/30 bg-danger/5 text-danger'
  }
  if (outcome === 'partial') {
    return 'border-warning/30 bg-warning/5 text-warning'
  }
  return 'border-line bg-panel-subtle text-ink-muted'
}

function EvidenceLinks({
  references,
  label = 'Evidence',
}: {
  references: EvidenceReference[]
  label?: string
}) {
  if (references.length === 0) {
    return <span className="text-xs text-ink-muted">No evidence linked</span>
  }
  return (
    <span className="flex flex-wrap gap-1.5">
      {references.map((reference, index) => (
        <button
          key={`${reference.artifact_id}:${reference.artifact_sha256}:${reference.locator ?? ''}`}
          className="rounded-full border border-line bg-panel px-2 py-1 font-mono text-label text-ink-soft hover:border-brand hover:text-ink"
          type="button"
          data-evidence-target="technical"
          title={`${reference.artifact_id} · ${shortHash(reference.artifact_sha256)}`}
          onClick={revealTechnicalSection}
        >
          {label} {index + 1}
        </button>
      ))}
    </span>
  )
}

function AssessmentMatrix({ entries }: { entries: AssessmentEntry[] }) {
  if (entries.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-line px-4 py-6 text-sm text-ink-muted">
        No assessments were retained for this run.
      </div>
    )
  }

  return (
    <div className="overflow-hidden rounded-lg border border-line">
      <div className="hidden overflow-x-auto md:block">
        <table className="w-full border-collapse text-left text-sm">
          <thead className="bg-panel-subtle text-label uppercase tracking-[0.06em] text-ink-muted">
            <tr>
              <th className="px-3 py-2.5 font-semibold">Assessment</th>
              <th className="px-3 py-2.5 font-semibold">Outcome</th>
              <th className="px-3 py-2.5 font-semibold">Score</th>
              <th className="px-3 py-2.5 font-semibold">Conclusion</th>
            </tr>
          </thead>
          <tbody>
            {entries.map((entry) => (
              <AssessmentRow key={entry.id} entry={entry} />
            ))}
          </tbody>
        </table>
      </div>
      <div className="grid gap-2 p-2 md:hidden">
        {entries.map((entry) => (
          <AssessmentCard key={entry.id} entry={entry} />
        ))}
      </div>
    </div>
  )
}

function AssessmentIdentity({ entry }: { entry: AssessmentEntry }) {
  return (
    <span className="grid gap-1">
      <strong className="break-all font-mono text-xs text-ink">
        {entry.criterionId}
      </strong>
      <span className="text-xs text-ink-muted">
        {titleCase(entry.kind)} · {titleCase(entry.dimension)}
      </span>
      {entry.targetId !== entry.criterionId && (
        <span className="font-mono text-label text-ink-muted">
          target {entry.targetId}
        </span>
      )}
    </span>
  )
}

function AssessmentScore({ entry }: { entry: AssessmentEntry }) {
  return (
    <span className="grid gap-1 text-xs">
      <strong className="text-ink-soft">
        {entry.score
          ? `${entry.score.awarded} / ${entry.score.possible}`
          : 'No score'}
      </strong>
    </span>
  )
}

function AssessmentConclusion({ entry }: { entry: AssessmentEntry }) {
  return (
    <span className="grid min-w-[220px] gap-2">
      <span className="text-sm leading-5 text-ink-soft">{entry.summary}</span>
      <EvidenceLinks references={entry.evidence} />
    </span>
  )
}

function AssessmentRow({ entry }: { entry: AssessmentEntry }) {
  return (
    <tr
      data-assessment-entry={entry.id}
      className="border-t border-line align-top"
    >
      <th className="px-3 py-3 font-normal" scope="row">
        <AssessmentIdentity entry={entry} />
      </th>
      <td className="px-3 py-3">
        <span
          className={`inline-flex rounded-full border px-2 py-1 text-label font-semibold ${toneForOutcome(entry.outcome)}`}
        >
          {titleCase(entry.validationOutcome ?? entry.outcome)}
        </span>
      </td>
      <td className="px-3 py-3">
        <AssessmentScore entry={entry} />
      </td>
      <td className="px-3 py-3">
        <AssessmentConclusion entry={entry} />
      </td>
    </tr>
  )
}

function AssessmentCard({ entry }: { entry: AssessmentEntry }) {
  return (
    <article
      data-assessment-entry={entry.id}
      className="grid gap-3 rounded-lg border border-line bg-panel p-3"
    >
      <div className="flex items-start justify-between gap-3">
        <AssessmentIdentity entry={entry} />
        <span
          className={`shrink-0 rounded-full border px-2 py-1 text-label font-semibold ${toneForOutcome(entry.outcome)}`}
        >
          {titleCase(entry.validationOutcome ?? entry.outcome)}
        </span>
      </div>
      <div className="grid grid-cols-1 gap-3 border-y border-line py-2">
        <AssessmentScore entry={entry} />
      </div>
      <AssessmentConclusion entry={entry} />
    </article>
  )
}

function AssessmentDetailContent({
  run,
  entries,
}: {
  run: AssessmentRunView
  entries: AssessmentEntry[]
}) {
  const objectiveFailure = run.systemStatus !== 'passed'

  // Audit ED-25: a run that retained no assessments passed on infrastructure
  // alone. The tiles said "Not reported" twice and left the reader to work out
  // that nothing about the output was ever checked.
  const scoredNothing = run.assessments.length === 0

  return (
    <div className="grid gap-5">
      {scoredNothing ? (
        <Callout
          tone="warning"
          title="only execution and infrastructure were checked"
        >
          This run retained no assessments, so nothing about the deliverable or
          its structure was scored. The outcome below reports that the run
          completed, not that it produced the right thing.
        </Callout>
      ) : null}
      <PrimaryMetricBoard run={run} standalone />

      <section aria-labelledby={`${safeId(run.key)}-outcome`}>
        <div className="mb-3 flex items-center gap-2">
          {objectiveFailure ? (
            <AlertTriangle
              className="text-danger"
              size={17}
              aria-hidden="true"
            />
          ) : (
            <CheckCircle2
              className="text-success"
              size={17}
              aria-hidden="true"
            />
          )}
          <h4
            id={`${safeId(run.key)}-outcome`}
            className="m-0 text-sm text-ink"
          >
            System outcome
          </h4>
        </div>
        <SystemOutcomeBadge outcome={{ value: run.systemStatus }} />
      </section>

      <section
        className="grid gap-2"
        aria-labelledby={`${safeId(run.key)}-next`}
      >
        <h4
          id={`${safeId(run.key)}-next`}
          className="m-0 text-label font-semibold uppercase tracking-[0.06em] text-ink-muted"
        >
          Suggested next step
        </h4>
        <p className="m-0 text-sm leading-5 text-pretty text-ink-soft">
          {buildHarnessRecommendation(run)}
        </p>
      </section>

      <details className="rounded-lg border border-line bg-panel-subtle">
        <summary
          id={`${safeId(run.key)}-runtime`}
          className="min-h-11 cursor-pointer px-4 py-3 text-sm font-semibold text-ink"
        >
          Runtime telemetry
        </summary>
        <div
          className="grid gap-2 border-t border-line p-3 sm:grid-cols-2 lg:grid-cols-4"
          data-run-metrics-detail
        >
          <RunMetricCard
            label="Input tokens"
            value={formatMetricCount(run.metrics.inputTokens)}
          />
          <RunMetricCard
            label="Output tokens"
            value={formatMetricCount(run.metrics.outputTokens)}
          />
          <RunMetricCard
            label="Cache read"
            value={formatMetricCount(run.metrics.cacheReadTokens)}
          />
          <RunMetricCard
            label="Reasoning tokens"
            value={formatMetricCount(run.metrics.reasoningTokens)}
          />
          <RunMetricCard
            label="Sessions"
            value={formatMetricCount(run.metrics.sessions)}
          />
          <RunMetricCard
            label="Turns"
            value={formatMetricCount(run.metrics.turns)}
          />
          <RunMetricCard
            label="Function calls"
            value={formatMetricCount(run.metrics.functionCalls)}
          />
          <RunMetricCard
            label="Duration"
            value={formatRunDuration(run.metrics.durationMs)}
          />
          <RunMetricCard
            label="Function errors"
            value={formatMetricCount(run.metrics.functionCallErrors)}
          />
        </div>
      </details>

      <section
        className="grid gap-3"
        aria-labelledby={`${safeId(run.key)}-matrix`}
      >
        <div className="flex items-center gap-2">
          <ShieldCheck className="text-brand" size={17} aria-hidden="true" />
          <div>
            <h4
              id={`${safeId(run.key)}-matrix`}
              className="m-0 text-sm text-ink"
            >
              Assessment matrix
            </h4>
            <p className="m-0 text-xs text-ink-muted">
              Scored criteria and asset validations, each with the outcome and
              evidence the run retained.
            </p>
          </div>
        </div>
        <AssessmentMatrix entries={entries} />
      </section>
    </div>
  )
}

function RunStatusBadges({ run }: { run: AssessmentRunView }) {
  return (
    <span className="flex flex-wrap items-start justify-end gap-1.5">
      <span
        className={`rounded-full border px-2 py-1 text-label font-semibold ${toneForOutcome(run.systemStatus)}`}
      >
        System:{' '}
        {run.systemStatus === 'hard_gate_failed'
          ? 'Failed (legacy result)'
          : titleCase(run.systemStatus)}
      </span>
    </span>
  )
}

function TranscriptButton({
  run,
  onTranscript,
}: {
  run: AssessmentRunView
  onTranscript?: (run: AssessmentRunView, title: string) => void
}) {
  if (!onTranscript || !run.transcript) return null
  return (
    <button
      className="button inline-flex min-h-11 items-center justify-center gap-2"
      type="button"
      data-transcript-action={run.key}
      aria-label={`Open transcript for ${titleCase(run.scenarioId)}`}
      onClick={(event) => {
        event.stopPropagation()
        onTranscript(run, `${titleCase(run.scenarioId)} · ${run.runId}`)
      }}
    >
      <MessageCircle size={15} aria-hidden="true" />
      Transcript
    </button>
  )
}

export function AssessmentDetailDialog({
  run,
  detail,
  onClose,
}: {
  run: AssessmentRunView
  detail?: DashboardExecutionDetail | null
  onClose: () => void
  onTranscript?: (run: AssessmentRunView, title: string) => void
}) {
  // Audit AW-06: the design-system Dialog opens as a modal and moves focus
  // to the title, so keyboard and screen-reader users land on the record.
  return (
    <Dialog
      open
      onClose={onClose}
      size="lg"
      tall
      kicker="Evidence record"
      title={`${titleCase(run.scenarioId)} · scenario v${run.scenarioVersion}`}
      description={
        <span className="break-all font-mono text-label">
          {run.subjectId} · run {run.runId}
        </span>
      }
      closeLabel="Close assessment detail"
      className="ds-root"
      bodyPadding
      actions={
        <>
          <RunStatusBadges run={run} />
          <ScenarioChatAction
            compact
            detail={detail}
            scenarioId={run.scenarioId}
            subjectId={run.subjectId}
            runId={run.runId}
          />
        </>
      }
    >
      <AssessmentDetailContent run={run} entries={run.assessments} />
    </Dialog>
  )
}

const OUTCOME_STATUSES: Record<AssessmentOutcome, OperationalStatus> = {
  passed: 'passed',
  failed: 'failed',
  error: 'failed',
  partial: 'inconclusive',
  not_evaluated: 'unavailable',
  unavailable: 'unavailable',
}

/** Pairs each criterion of the contract with what happened to it. The spec
 *  supplies the requirement, the run supplies the outcome; a criterion the run
 *  never reported still shows what it would have demanded (audit TH-21). */
function criteriaOutcomes(run: AssessmentRunView, criteria: TestCriterion[]) {
  const byId = new Map(
    run.assessments.map((entry) => [entry.criterionId, entry]),
  )
  return new Map(
    criteria.map((criterion) => {
      const entry = byId.get(criterion.id)
      const outcome = entry?.outcome ?? 'not_evaluated'
      return [
        criterion.id,
        {
          label: outcome.replace(/_/g, ' '),
          status: OUTCOME_STATUSES[outcome],
        },
      ]
    }),
  )
}

function RunAssessment({
  run,
  spec,
  onOpen,
  onTranscript,
}: {
  run: AssessmentRunView
  spec?: TestSpec | null
  onOpen: () => void
  onTranscript?: (run: AssessmentRunView, title: string) => void
}) {
  const criteria = spec?.criteria ?? []
  const nothingEvaluated =
    run.assessments.length > 0 &&
    run.assessments.every((entry) => !isEvaluated(entry.outcome))

  return (
    <article
      data-assessment-run={run.key}
      className="overflow-hidden rounded-[6px] border border-line bg-panel"
    >
      <header className="flex flex-col gap-4 p-4 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <span className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-ink-muted">
            Scenario performance
          </span>
          <h3 className="mt-1 mb-0 text-lg font-semibold tracking-[-0.025em] text-ink">
            {titleCase(run.scenarioId)}
          </h3>
          <p className="mt-1 mb-0 break-all font-mono text-label text-ink-muted">
            v{run.scenarioVersion} · {run.subjectId} · run {run.runId}
          </p>
        </div>
        <RunStatusBadges run={run} />
      </header>

      <PrimaryMetricBoard run={run} />

      {criteria.length > 0 ? (
        // A fill, not a rule: the metric board above already ends on a line,
        // and the design system separates bands by surface (audit DS-11).
        <section
          aria-labelledby={`${safeId(run.key)}-required`}
          className="bg-panel-subtle p-4"
        >
          {nothingEvaluated ? (
            <Callout
              className="mb-3.5"
              tone="warning"
              title="nothing was scored on merit"
            >
              This run ended in {titleCase(run.systemStatus).toLowerCase()}{' '}
              after {formatRunDuration(run.metrics.durationMs)}, before any
              deliverable was captured. Every criterion below is unevaluated —
              none of them failed on the subject's work.
            </Callout>
          ) : null}
          <TestCriteriaList
            criteria={criteria}
            headingId={`${safeId(run.key)}-required`}
            outcomes={criteriaOutcomes(run, criteria)}
          />
        </section>
      ) : null}

      <footer className="flex flex-col gap-3 p-3 sm:flex-row sm:items-center sm:justify-between">
        <p className="m-0 font-mono text-label text-ink-muted">
          Runtime {formatRunDuration(run.metrics.durationMs)} · Tokens{' '}
          {formatMetricCount(run.metrics.totalTokens)} · Function errors{' '}
          {formatMetricCount(run.metrics.functionCallErrors)}
        </p>
        <div className="flex flex-wrap gap-2">
          {onTranscript && run.transcript ? (
            <TranscriptButton run={run} onTranscript={onTranscript} />
          ) : null}
          <button
            className={buttonClassName({ variant: 'secondary' })}
            type="button"
            onClick={onOpen}
            aria-label={`Open details for ${titleCase(run.scenarioId)}`}
          >
            Review evidence
          </button>
        </div>
      </footer>
    </article>
  )
}

export function AssessmentPanel({
  model,
  detail,
  filter,
  spec,
  onFilter,
  onTranscript,
}: {
  model: AssessmentWorkspaceModel
  detail?: DashboardExecutionDetail | null
  filter: AssessmentFilter
  /** The scored contract, so each run reads against what it had to meet. */
  spec?: TestSpec | null
  onFilter?: (filter: AssessmentFilter) => void
  onTranscript?: (run: AssessmentRunView, title: string) => void
}) {
  const [selectedRunKey, setSelectedRunKey] = useState<string | null>(null)
  const visibleRuns = useMemo(
    () =>
      model.availability === 'available'
        ? model.runs.filter(
            (run) =>
              filter === 'all' ||
              run.assessments.some((entry) =>
                matchesAssessmentFilter(entry, filter),
              ) ||
              (filter === 'failed' && run.systemStatus !== 'passed'),
          )
        : [],
    [filter, model],
  )
  const selectedRun = visibleRuns.find((run) => run.key === selectedRunKey)

  useEffect(() => {
    if (selectedRunKey && !selectedRun) setSelectedRunKey(null)
  }, [selectedRun, selectedRunKey])

  if (model.availability !== 'available' || model.runs.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-line bg-panel-subtle p-5">
        <strong className="block text-sm text-ink">
          Assessment data is unavailable
        </strong>
        <p className="mt-2 mb-0 text-sm leading-5 text-ink-muted">
          No assessment contract is available. No status has been inferred.
        </p>
      </div>
    )
  }

  const counts = assessmentFilterCounts(model.runs)

  return (
    <div className="grid gap-3">
      {/* Audit AW-03: a filter bar over zero assessments is noise. */}
      {counts.all > 0 ? (
        <details className="rounded-[6px] border border-line bg-panel-subtle">
          <summary className="flex min-h-11 cursor-pointer items-center justify-between gap-3 px-4 py-3 text-xs font-semibold text-ink-soft">
            <span>Filter scenario runs by assessment signal</span>
            <span className="font-mono text-label font-normal text-ink-muted">
              {counts.all} assessments
            </span>
          </summary>
          <div className="border-t border-line p-3">
            <fieldset className="m-0 flex flex-wrap gap-2 border-0 p-0">
              <legend className="sr-only">Filter assessment matrix</legend>
              {FILTERS.map((candidate) => (
                <button
                  key={candidate.id}
                  className={`min-h-11 rounded-full border px-3 py-2 text-xs font-semibold transition motion-reduce:transition-none ${
                    filter === candidate.id
                      ? 'border-brand bg-brand-soft text-ink'
                      : 'border-line bg-panel text-ink-muted hover:border-line-strong hover:text-ink'
                  }`}
                  type="button"
                  aria-pressed={filter === candidate.id}
                  onClick={() => onFilter?.(candidate.id)}
                >
                  {candidate.label} · {counts[candidate.id]}
                </button>
              ))}
            </fieldset>
            <p className="mt-3 mb-0 text-xs text-ink-muted" role="status">
              {counts[filter]} assessment
              {counts[filter] === 1 ? '' : 's'} match this view.
            </p>
          </div>
        </details>
      ) : null}
      <div className="grid gap-3">
        {visibleRuns.map((run) => (
          <RunAssessment
            key={run.key}
            run={run}
            spec={spec}
            onOpen={() => setSelectedRunKey(run.key)}
            onTranscript={onTranscript}
          />
        ))}
        {visibleRuns.length === 0 ? (
          <div className="rounded-lg border border-dashed border-line p-5 text-sm text-ink-muted">
            No scenario runs match this assessment filter.
          </div>
        ) : null}
      </div>
      {selectedRun && (
        <AssessmentDetailDialog
          run={selectedRun}
          detail={detail}
          onClose={() => setSelectedRunKey(null)}
          onTranscript={onTranscript}
        />
      )}
    </div>
  )
}

export function AssessmentWorkspace({
  detail,
  spec,
  onTranscript,
}: {
  detail: DashboardExecutionDetail | null
  spec?: TestSpec | null
  onTranscript?: (run: AssessmentRunView, title: string) => void
}) {
  const [filter, setFilter] = useState<AssessmentFilter>('all')
  const model = useMemo(() => buildAssessmentWorkspace(detail), [detail])
  return (
    <AssessmentPanel
      model={model}
      detail={detail}
      filter={filter}
      spec={spec}
      onFilter={setFilter}
      onTranscript={onTranscript}
    />
  )
}
