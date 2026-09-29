import { AlertTriangle, CheckCircle2, ShieldCheck } from 'lucide-react'
import { SystemOutcomeBadge } from '@/components/SystemOutcome'
import { Callout } from '@/design-system'
import type {
  AssessmentOutcome,
  EvidenceReference,
} from '@/lib/assessment-contract'
import type { AssessmentEntry, AssessmentRunView } from '@/lib/assessment-view'
import { formatDuration } from '@/lib/execution-view'

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
    <div className="min-w-0">
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
  context: 'Score' | 'Signal' | 'Observed'
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
function revealTechnicalSection() {
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
function isEvaluated(outcome: AssessmentOutcome): boolean {
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
      label: 'Score',
      value: run.score === null ? 'Not reported' : `${run.score}/100`,
      detail:
        run.score === null
          ? 'No score retained'
          : 'Points the evaluated criteria awarded',
      context: 'Score',
      tone:
        run.score === null
          ? 'unavailable'
          : run.score === 100
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
        standalone ? 'border-b border-line' : 'border-y border-line'
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

export function AssessmentDetailContent({
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
          title="Only execution and infrastructure were checked"
        >
          This run retained no assessments, so nothing about the deliverable or
          its structure was scored. The outcome below reports that the run
          completed, not that it produced the right thing.
        </Callout>
      ) : null}
      <section
        className="@container/run-metrics overflow-hidden rounded-lg border border-line bg-panel"
        aria-label="Run metrics"
      >
        <PrimaryMetricBoard run={run} standalone />
        <div
          className="grid grid-cols-1 gap-5 p-4 @[400px]/run-metrics:grid-cols-2 @[700px]/run-metrics:grid-cols-3"
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
            label="Cache written"
            value={formatMetricCount(run.metrics.cacheWriteTokens)}
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
          {primaryRunMetrics(run).some(
            (metric) => metric.label === 'Runtime',
          ) ? null : (
            <RunMetricCard
              label="Runtime"
              value={formatRunDuration(run.metrics.durationMs)}
            />
          )}
          <RunMetricCard
            label="Function errors"
            value={formatMetricCount(run.metrics.functionCallErrors)}
          />
        </div>
      </section>

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
