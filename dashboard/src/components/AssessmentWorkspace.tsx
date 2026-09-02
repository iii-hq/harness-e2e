import {
  AlertTriangle,
  BrainCircuit,
  CheckCircle2,
  MessageCircle,
  ShieldCheck,
} from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import { buttonClassName } from '@/design-system'
import type {
  AnalyzerIdentity,
  AnalyzerUsage,
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
  LOW_CONFIDENCE_THRESHOLD,
  matchesAssessmentFilter,
} from '@/lib/assessment-view'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import { formatDuration } from '@/lib/execution-view'

const FILTERS: Array<{ id: AssessmentFilter; label: string }> = [
  { id: 'all', label: 'All' },
  { id: 'failed', label: 'Failed' },
  { id: 'low_confidence', label: 'Low confidence' },
  { id: 'unavailable', label: 'Unavailable' },
  { id: 'asset', label: 'Asset-related' },
  { id: 'ai', label: 'AI-evaluated' },
]

function titleCase(value: string) {
  return value
    .replaceAll('_', ' ')
    .replaceAll('-', ' ')
    .replace(/\b\w/g, (letter) => letter.toUpperCase())
}

function formatConfidence(value: number | undefined) {
  if (value == null || !Number.isFinite(value)) return 'Not reported'
  return `${Math.round(value * 100)}%`
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
  context: 'Objective' | 'Advisory' | 'Observed'
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

const DIALOG_CLOSE_CLASS =
  'inline-grid size-11 shrink-0 place-items-center rounded-[6px] border-0 bg-transparent text-xl leading-none text-ink-soft hover:bg-panel-subtle hover:text-ink'

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
  document.getElementById('technical')?.scrollIntoView({ block: 'start' })
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

function primaryRunMetrics(run: AssessmentRunView): PrimaryMetric[] {
  const hardGates = run.assessments.filter(
    (entry) => entry.policy === 'hard_gate',
  )
  const passedHardGates = hardGates.filter(
    (entry) => entry.outcome === 'passed',
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
  const aiResult = run.finalAssessment.result
  const hardGateTone: PrimaryMetricTone =
    hardGates.length === 0
      ? 'unavailable'
      : passedHardGates === hardGates.length
        ? 'positive'
        : 'negative'
  // Audit AW-04: a run with no retained assessments is unavailable, not
  // "0/0 passed".
  const assessmentTone: PrimaryMetricTone =
    run.assessments.length === 0
      ? 'unavailable'
      : passedAssessments === run.assessments.length
        ? 'positive'
        : 'warning'
  const aiTone: PrimaryMetricTone = !aiResult
    ? 'unavailable'
    : aiResult.verdict === 'pass'
      ? 'positive'
      : aiResult.verdict === 'pass_with_concerns' ||
          aiResult.verdict === 'inconclusive'
        ? 'warning'
        : 'negative'

  const objectiveMetric: PrimaryMetric =
    hardGates.length === 0 && run.systemStatus !== 'passed'
      ? {
          label: 'Objective result',
          value: titleCase(run.systemStatus),
          detail: 'Authoritative system result',
          context: 'Objective',
          tone: 'negative',
        }
      : {
          label: 'Objective hard gates',
          value:
            hardGates.length > 0
              ? `${passedHardGates}/${hardGates.length}`
              : 'Not reported',
          detail:
            hardGates.length > 0
              ? `${hardGates.length - passedHardGates} failed`
              : 'No deterministic gates retained',
          context: 'Objective',
          tone: hardGateTone,
        }

  return [
    objectiveMetric,
    detection
      ? {
          label: 'Seeded detection',
          value: metricRatio(detection),
          detail: `${scorePercent(detection) ?? '—'}% advisory coverage`,
          context: 'Advisory',
          tone: metricToneForEntry(detection),
        }
      : {
          label: 'Assessment outcomes',
          value:
            run.assessments.length > 0
              ? `${passedAssessments}/${run.assessments.length}`
              : 'Not reported',
          detail:
            run.assessments.length > 0
              ? `${run.assessments.length - passedAssessments} need review`
              : 'No assessments retained',
          context: 'Observed',
          tone: assessmentTone,
        },
    patchApplicability
      ? {
          label: 'Optional patch checks',
          value: metricRatio(patchApplicability),
          detail: `${scorePercent(patchApplicability) ?? '—'}% applied cleanly`,
          context: 'Advisory',
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
      label: 'AI quality',
      value:
        aiResult?.quality_score != null
          ? `${aiResult.quality_score}/100`
          : 'Not reported',
      detail: aiResult
        ? `${formatConfidence(aiResult.confidence)} confidence`
        : 'No advisory conclusion',
      context: 'Advisory',
      tone: aiTone,
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
  if (outcome === 'passed' || outcome === 'pass' || outcome === 'valid') {
    return 'border-success/30 bg-success/5 text-success'
  }
  if (
    outcome === 'failed' ||
    outcome === 'fail' ||
    outcome === 'error' ||
    outcome === 'resource_limit' ||
    outcome.endsWith('_failed') ||
    outcome.endsWith('_error')
  ) {
    return 'border-danger/30 bg-danger/5 text-danger'
  }
  if (
    outcome === 'partial' ||
    outcome === 'pass_with_concerns' ||
    outcome === 'passed_with_concerns'
  ) {
    return 'border-warning/30 bg-warning/5 text-warning'
  }
  return 'border-line bg-panel-subtle text-ink-muted'
}

function StatusCard({
  label,
  value,
  caption,
}: {
  label: string
  value: string
  caption: string
}) {
  return (
    <div className={`rounded-lg border p-3 ${toneForOutcome(value)}`}>
      <span className="block text-label font-semibold uppercase tracking-[0.065em] opacity-80">
        {label}
      </span>
      <strong className="mt-1 block text-sm text-current">
        {titleCase(value)}
      </strong>
      <span className="mt-1 block text-xs leading-5 text-ink-muted">
        {caption}
      </span>
    </div>
  )
}

function AnalyzerProvenance({
  analyzer,
  usage,
}: {
  analyzer?: AnalyzerIdentity
  usage?: AnalyzerUsage
}) {
  if (!analyzer) {
    return (
      <span className="text-xs text-ink-muted">
        No analyzer provenance was recorded.
      </span>
    )
  }
  const usageParts = [
    usage?.latency_ms != null ? `${usage.latency_ms} ms` : '',
    usage?.input_tokens != null ? `${usage.input_tokens} input tokens` : '',
    usage?.output_tokens != null ? `${usage.output_tokens} output tokens` : '',
    usage?.cost_usd != null ? `$${usage.cost_usd.toFixed(4)}` : '',
  ].filter(Boolean)
  return (
    <div className="grid gap-1 text-xs text-ink-muted">
      <span>
        <strong className="text-ink-soft">{analyzer.analyzer}</strong>
        {analyzer.provider || analyzer.model
          ? ` · ${[analyzer.provider, analyzer.model].filter(Boolean).join('/')}`
          : ''}
      </span>
      <code className="break-all text-label" title={analyzer.input_sha256}>
        input {shortHash(analyzer.input_sha256)}
      </code>
      {usageParts.length > 0 && <span>{usageParts.join(' · ')}</span>}
    </div>
  )
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
              <th className="px-3 py-2.5 font-semibold">Policy / source</th>
              <th className="px-3 py-2.5 font-semibold">Outcome</th>
              <th className="px-3 py-2.5 font-semibold">Score / confidence</th>
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
      <span
        className={
          entry.confidence != null &&
          entry.confidence < LOW_CONFIDENCE_THRESHOLD
            ? 'text-warning'
            : 'text-ink-muted'
        }
      >
        {formatConfidence(entry.confidence)} confidence
      </span>
    </span>
  )
}

function AssessmentConclusion({ entry }: { entry: AssessmentEntry }) {
  return (
    <span className="grid min-w-[220px] gap-2">
      <span className="text-sm leading-5 text-ink-soft">{entry.summary}</span>
      <EvidenceLinks references={entry.evidence} />
      {entry.analyzer && (
        <details className="text-xs text-ink-muted">
          <summary className="cursor-pointer font-semibold text-ink-soft">
            Analyzer provenance
          </summary>
          <div className="mt-2">
            <AnalyzerProvenance
              analyzer={entry.analyzer}
              usage={entry.analyzerUsage}
            />
          </div>
        </details>
      )}
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
      <td className="px-3 py-3 text-xs text-ink-muted">
        <strong className="block text-ink-soft">
          {titleCase(entry.policy)}
        </strong>
        {titleCase(entry.source)}
      </td>
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
      <div className="grid grid-cols-2 gap-3 border-y border-line py-2">
        <span className="text-xs text-ink-muted">
          <strong className="block text-ink-soft">
            {titleCase(entry.policy)}
          </strong>
          {titleCase(entry.source)}
        </span>
        <AssessmentScore entry={entry} />
      </div>
      <AssessmentConclusion entry={entry} />
    </article>
  )
}

type AiSectionId = 'facts' | 'strengths' | 'concerns' | 'limitations'

const AI_SECTION_LABELS: ReadonlyArray<{
  id: AiSectionId
  label: string
}> = [
  { id: 'facts', label: 'AI-reported facts' },
  { id: 'strengths', label: 'Strengths' },
  { id: 'concerns', label: 'Concerns' },
  { id: 'limitations', label: 'Limitations' },
]

function AiNarrativeTabs({
  narrativeId,
  result,
}: {
  narrativeId: string
  result: NonNullable<AssessmentRunView['finalAssessment']['result']>
}) {
  const [activeSection, setActiveSection] = useState<AiSectionId>('facts')
  const tabRefs = useRef(new Map<AiSectionId, HTMLButtonElement>())
  const activeIndex = AI_SECTION_LABELS.findIndex(
    (section) => section.id === activeSection,
  )
  const activeLabel =
    AI_SECTION_LABELS[activeIndex < 0 ? 0 : activeIndex] ?? AI_SECTION_LABELS[0]
  const items = result[activeLabel.id]

  const selectSection = (id: AiSectionId, focus = false) => {
    setActiveSection(id)
    if (focus) {
      window.requestAnimationFrame(() => tabRefs.current.get(id)?.focus())
    }
  }

  const onTabKeyDown = (
    event: React.KeyboardEvent<HTMLButtonElement>,
    id: AiSectionId,
  ) => {
    const index = AI_SECTION_LABELS.findIndex((section) => section.id === id)
    let nextIndex: number | null = null
    if (event.key === 'ArrowRight')
      nextIndex = (index + 1) % AI_SECTION_LABELS.length
    if (event.key === 'ArrowLeft')
      nextIndex =
        (index - 1 + AI_SECTION_LABELS.length) % AI_SECTION_LABELS.length
    if (event.key === 'Home') nextIndex = 0
    if (event.key === 'End') nextIndex = AI_SECTION_LABELS.length - 1
    if (nextIndex == null) return
    event.preventDefault()
    selectSection(AI_SECTION_LABELS[nextIndex].id, true)
  }

  return (
    <div className="grid w-full gap-2">
      <div
        className="flex w-full min-w-0 overflow-x-auto rounded-lg border border-line bg-panel-subtle p-1"
        role="tablist"
        aria-label="Diagnostic narrative sections"
      >
        {AI_SECTION_LABELS.map((section) => {
          const selected = section.id === activeSection
          const count = result[section.id]?.length ?? 0
          return (
            <button
              key={section.id}
              ref={(node) => {
                if (node) tabRefs.current.set(section.id, node)
                else tabRefs.current.delete(section.id)
              }}
              id={`${narrativeId}-tab-${section.id}`}
              className={`flex min-h-11 min-w-max flex-1 items-center justify-between gap-2 rounded-md px-3 py-2 text-left text-label font-semibold uppercase tracking-[0.05em] transition sm:min-w-0 ${
                selected
                  ? 'bg-brand-soft text-ink'
                  : 'text-ink-muted hover:bg-panel/60 hover:text-ink'
              }`}
              type="button"
              role="tab"
              aria-label={`${section.label}, ${count} reported`}
              aria-selected={selected}
              aria-controls={`${narrativeId}-panel-${section.id}`}
              tabIndex={selected ? 0 : -1}
              onClick={() => selectSection(section.id)}
              onKeyDown={(event) => onTabKeyDown(event, section.id)}
            >
              <span>{section.label}</span>
              <span
                className={
                  selected
                    ? 'inline-flex h-7 min-w-7 shrink-0 items-center justify-center rounded-full border-2 border-brand bg-panel px-1.5 text-[0.7rem] font-bold leading-none tabular-nums text-ink'
                    : 'inline-flex h-6 min-w-6 shrink-0 items-center justify-center rounded-full border border-line-strong bg-panel px-1.5 text-label font-bold leading-none tabular-nums text-ink-soft'
                }
                title={`${count} reported`}
              >
                {count}
              </span>
            </button>
          )
        })}
      </div>
      <section
        id={`${narrativeId}-panel-${activeLabel.id}`}
        className="w-full rounded-lg border border-line bg-panel-subtle px-3 py-3"
        role="tabpanel"
        aria-labelledby={`${narrativeId}-tab-${activeLabel.id}`}
      >
        {items?.length ? (
          <ul className="m-0 grid gap-1.5 pl-4 text-sm leading-5 text-ink-soft">
            {items.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        ) : (
          <p className="m-0 text-sm leading-5 text-ink-muted">
            No {activeLabel.label.toLowerCase()} were retained for this run.
          </p>
        )}
      </section>
    </div>
  )
}

function FinalAiCard({ run }: { run: AssessmentRunView }) {
  const assessment = run.finalAssessment
  const result = assessment.result
  const narrativeId = `${safeId(run.key)}-ai-narrative`
  if (!result) {
    return (
      <article className="rounded-lg border border-line bg-panel-subtle p-4">
        <div className="flex items-start gap-3">
          <BrainCircuit
            className="mt-0.5 shrink-0 text-ink-muted"
            size={18}
            aria-hidden="true"
          />
          <div className="grid min-w-0 gap-2">
            <div>
              <span className="section-kicker">Advisory AI conclusion</span>
              <h4 className="m-0 text-base text-ink">
                {titleCase(assessment.availability)}
              </h4>
            </div>
            <p className="m-0 text-sm leading-5 text-ink-muted">
              {assessment.reason ||
                'No final AI conclusion was recorded for this run.'}
            </p>
            <AnalyzerProvenance
              analyzer={assessment.analyzer}
              usage={assessment.analyzer_usage}
            />
          </div>
        </div>
      </article>
    )
  }

  return (
    <article
      className={`w-full overflow-hidden rounded-lg border ${
        run.hasAiDisagreement
          ? 'border-warning/50 bg-warning/5'
          : 'border-line bg-panel'
      }`}
    >
      <header className="border-b border-line p-4">
        <div className="flex min-w-0 items-start gap-3">
          <BrainCircuit
            className="mt-0.5 shrink-0 text-brand"
            size={19}
            aria-hidden="true"
          />
          <div className="min-w-0">
            <span className="section-kicker">Advisory AI conclusion</span>
            <h4 className="m-0 text-lg text-ink">
              {titleCase(result.verdict)}
            </h4>
            <p className="mt-1 mb-0 text-sm leading-5 text-ink-soft">
              {result.summary}
            </p>
          </div>
        </div>
      </header>
      {run.hasAiDisagreement && (
        <div
          className="flex items-start gap-2 border-b border-warning/30 px-4 py-3 text-sm text-warning"
          role="alert"
        >
          <AlertTriangle
            className="mt-0.5 shrink-0"
            size={16}
            aria-hidden="true"
          />
          <strong>
            Advisory AI and the objective system outcome disagree. The objective
            outcome remains authoritative.
          </strong>
        </div>
      )}
      <div className="grid w-full gap-5 p-4">
        <section className="grid gap-2 lg:col-span-2">
          <h5 className="m-0 text-label font-semibold uppercase tracking-[0.06em] text-ink-muted">
            AI advisory
          </h5>
          <p className="m-0 text-xs text-ink-muted">
            Advisory guidance from the AI assessment. The objective system
            outcome remains authoritative.
          </p>
          {result.diagnosis ? (
            <div className="grid gap-1">
              <p className="m-0 text-label font-semibold uppercase tracking-[0.06em] text-ink-muted">
                What happened
              </p>
              <p className="m-0 border-l-2 border-[var(--color-rule)] pl-3 text-sm leading-5 text-ink-soft">
                {result.diagnosis}
              </p>
            </div>
          ) : null}
          <div className="grid gap-1">
            <p className="m-0 text-label font-semibold uppercase tracking-[0.06em] text-ink-muted">
              Suggested correction or improvement
            </p>
            <p className="m-0 border-l-2 border-brand pl-3 text-sm leading-5 text-ink-soft">
              {result.recommendation || buildHarnessRecommendation(run)}
            </p>
          </div>
        </section>
        <section
          className="grid w-full gap-2 lg:col-span-2"
          aria-labelledby={narrativeId}
        >
          <div>
            <h5
              id={narrativeId}
              className="m-0 text-label font-semibold uppercase tracking-[0.06em] text-ink-muted"
            >
              Diagnostic narrative
            </h5>
            <p className="m-0 mt-1 text-xs text-ink-muted">
              Facts shown first; choose another tab to inspect the rest.
            </p>
          </div>
          <AiNarrativeTabs narrativeId={narrativeId} result={result} />
        </section>
        <section className="grid gap-2 lg:col-span-2">
          <h5 className="m-0 text-label font-semibold uppercase tracking-[0.06em] text-ink-muted">
            Evidence supporting this AI conclusion
          </h5>
          <EvidenceLinks references={result.evidence ?? []} label="Reference" />
        </section>
        <section className="grid gap-2 border-t border-line pt-3 lg:col-span-2">
          <h5 className="m-0 text-label font-semibold uppercase tracking-[0.06em] text-ink-muted">
            Analyzer provenance
          </h5>
          <AnalyzerProvenance
            analyzer={assessment.analyzer}
            usage={assessment.analyzer_usage}
          />
        </section>
      </div>
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
  const ai = run.finalAssessment
  const aiLabel = ai.result?.verdict ?? ai.availability
  const objectiveFailure = run.systemStatus !== 'passed'

  return (
    <div className="grid gap-5">
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
            Outcome boundaries
          </h4>
        </div>
        <div className="grid gap-2 sm:grid-cols-3">
          <StatusCard
            label="Objective system"
            value={run.systemStatus}
            caption="Deterministic gates, execution, and infrastructure."
          />
          <StatusCard
            label="Advisory AI"
            value={aiLabel}
            caption="Separate qualitative conclusion; never overrides the system."
          />
          <StatusCard
            label="Effective harness"
            value={run.effectiveStatus}
            caption="Canonical final status exposed by the result contract."
          />
        </div>
      </section>

      <FinalAiCard run={run} />

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
              Required checks, advisory signals, dimensions, and asset checks
              retain their own policy and provenance.
            </p>
          </div>
        </div>
        <AssessmentMatrix entries={entries} />
      </section>
    </div>
  )
}

function RunStatusBadges({
  run,
  aiLabel,
}: {
  run: AssessmentRunView
  aiLabel: string
}) {
  return (
    <span className="flex flex-wrap items-start justify-end gap-1.5">
      <span
        className={`rounded-full border px-2 py-1 text-label font-semibold ${toneForOutcome(run.systemStatus)}`}
      >
        System: {titleCase(run.systemStatus)}
      </span>
      <span
        className={`rounded-full border px-2 py-1 text-label font-semibold ${toneForOutcome(aiLabel)}`}
      >
        AI: {titleCase(aiLabel)}
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
  const ref = useRef<HTMLDialogElement>(null)
  const titleRef = useRef<HTMLHeadingElement>(null)
  const aiLabel =
    run.finalAssessment.result?.verdict ?? run.finalAssessment.availability

  // Audit AW-06: open as a modal and move focus to the title, so keyboard and
  // screen-reader users land on the record instead of the close button.
  useEffect(() => {
    const dialog = ref.current
    if (!dialog) return
    if (!dialog.open) dialog.showModal()
    titleRef.current?.focus()
  }, [])

  return (
    <dialog
      ref={ref}
      className="assessment-detail-dialog m-auto h-[min(860px,calc(100dvh-48px))] w-[min(1120px,calc(100%-32px))] max-w-none overflow-hidden rounded-[6px] border border-line-strong bg-panel shadow-panel backdrop:bg-app-backdrop backdrop:backdrop-blur-[5px] max-[560px]:m-0 max-[560px]:h-dvh max-[560px]:w-screen max-[560px]:rounded-none max-[560px]:border-0"
      onClose={onClose}
      aria-labelledby={`${safeId(run.key)}-dialog-title`}
    >
      <div className="flex h-full min-h-0 flex-col">
        <header className="assessment-detail-header border-b border-line bg-panel">
          <div className="assessment-detail-heading">
            <div className="section-kicker mb-1.5">Evidence record</div>
            <h2
              id={`${safeId(run.key)}-dialog-title`}
              ref={titleRef}
              tabIndex={-1}
              className="m-0 break-words text-[1.25rem] font-[570] tracking-[-0.025em] text-ink outline-none"
            >
              {titleCase(run.scenarioId)} · scenario v{run.scenarioVersion}
            </h2>
            <p className="m-0 mt-1 break-all font-mono text-label text-ink-muted">
              {run.subjectId} · run {run.runId}
            </p>
          </div>
          <div className="assessment-detail-actions">
            <RunStatusBadges run={run} aiLabel={aiLabel} />
            <ScenarioChatAction
              compact
              detail={detail}
              scenarioId={run.scenarioId}
              subjectId={run.subjectId}
              runId={run.runId}
            />
            <button
              className={DIALOG_CLOSE_CLASS}
              type="button"
              onClick={onClose}
              aria-label="Close assessment detail"
            >
              ×
            </button>
          </div>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto p-4 sm:p-6">
          <AssessmentDetailContent run={run} entries={run.assessments} />
        </div>
      </div>
    </dialog>
  )
}

function RunAssessment({
  run,
  onOpen,
  onTranscript,
}: {
  run: AssessmentRunView
  onOpen: () => void
  onTranscript?: (run: AssessmentRunView, title: string) => void
}) {
  const aiLabel =
    run.finalAssessment.result?.verdict ?? run.finalAssessment.availability

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
        <RunStatusBadges run={run} aiLabel={aiLabel} />
      </header>

      <PrimaryMetricBoard run={run} />

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
  onFilter,
  onTranscript,
}: {
  model: AssessmentWorkspaceModel
  detail?: DashboardExecutionDetail | null
  filter: AssessmentFilter
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
          {model.availability === 'unavailable'
            ? 'Assessment data is unavailable'
            : 'This retained result has no assessment contract'}
        </strong>
        <p className="mt-2 mb-0 text-sm leading-5 text-ink-muted">
          {model.availability === 'unavailable'
            ? 'The producer explicitly reported that no assessment contract is available. No status or AI conclusion has been inferred.'
            : 'Legacy and aggregate-only results remain readable, but they cannot show assessment conclusions or evidence.'}
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
              {filter === 'low_confidence'
                ? `Low confidence means below ${Math.round(LOW_CONFIDENCE_THRESHOLD * 100)}%.`
                : `${counts[filter]} assessment${counts[filter] === 1 ? '' : 's'} match this view.`}
            </p>
          </div>
        </details>
      ) : null}
      <div className="grid gap-3">
        {visibleRuns.map((run) => (
          <RunAssessment
            key={run.key}
            run={run}
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
  onTranscript,
}: {
  detail: DashboardExecutionDetail | null
  onTranscript?: (run: AssessmentRunView, title: string) => void
}) {
  const [filter, setFilter] = useState<AssessmentFilter>('all')
  const model = useMemo(() => buildAssessmentWorkspace(detail), [detail])
  return (
    <AssessmentPanel
      model={model}
      detail={detail}
      filter={filter}
      onFilter={setFilter}
      onTranscript={onTranscript}
    />
  )
}
