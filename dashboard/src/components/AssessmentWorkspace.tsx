import {
  AlertTriangle,
  BrainCircuit,
  CheckCircle2,
  Database,
  ShieldCheck,
} from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { loadLegacyPage } from '@/hooks/useLegacyPage'
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
  LOW_CONFIDENCE_THRESHOLD,
  matchesAssessmentFilter,
} from '@/lib/assessment-view'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'

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
  if (outcome === 'partial' || outcome === 'pass_with_concerns') {
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
      <span className="block text-[0.61rem] font-semibold uppercase tracking-[0.065em] opacity-80">
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
      <code className="break-all text-[0.64rem]" title={analyzer.input_sha256}>
        input {shortHash(analyzer.input_sha256)}
      </code>
      {usageParts.length > 0 && <span>{usageParts.join(' · ')}</span>}
    </div>
  )
}

function EvidenceLinks({
  run,
  references,
  label = 'Evidence',
}: {
  run: AssessmentRunView
  references: EvidenceReference[]
  label?: string
}) {
  if (references.length === 0) {
    return <span className="text-xs text-ink-muted">No evidence linked</span>
  }
  return (
    <span className="flex flex-wrap gap-1.5">
      {references.map((reference, index) => (
        <a
          key={`${reference.artifact_id}:${reference.artifact_sha256}:${reference.locator ?? ''}`}
          className="rounded-full border border-line bg-panel px-2 py-1 font-mono text-[0.62rem] text-ink-soft no-underline hover:border-brand hover:text-ink"
          href={`#${evidenceId(run, reference)}`}
        >
          {label} {index + 1}
        </a>
      ))}
    </span>
  )
}

function evidenceId(run: AssessmentRunView, reference: EvidenceReference) {
  return safeId(
    `evidence-${run.key}-${reference.artifact_id}-${reference.artifact_sha256}-${reference.locator ?? ''}`,
  )
}

function AssessmentMatrix({
  run,
  entries,
}: {
  run: AssessmentRunView
  entries: AssessmentEntry[]
}) {
  if (entries.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-line px-4 py-6 text-sm text-ink-muted">
        No assessments match this filter for this run.
      </div>
    )
  }

  return (
    <div className="overflow-hidden rounded-lg border border-line">
      <div className="hidden overflow-x-auto md:block">
        <table className="w-full border-collapse text-left text-sm">
          <thead className="bg-panel-subtle text-[0.62rem] uppercase tracking-[0.06em] text-ink-muted">
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
              <AssessmentRow key={entry.id} run={run} entry={entry} />
            ))}
          </tbody>
        </table>
      </div>
      <div className="grid gap-2 p-2 md:hidden">
        {entries.map((entry) => (
          <AssessmentCard key={entry.id} run={run} entry={entry} />
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
        <span className="font-mono text-[0.62rem] text-ink-muted">
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

function AssessmentConclusion({
  run,
  entry,
}: {
  run: AssessmentRunView
  entry: AssessmentEntry
}) {
  return (
    <span className="grid min-w-[220px] gap-2">
      <span className="text-sm leading-5 text-ink-soft">{entry.summary}</span>
      <EvidenceLinks run={run} references={entry.evidence} />
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

function AssessmentRow({
  run,
  entry,
}: {
  run: AssessmentRunView
  entry: AssessmentEntry
}) {
  return (
    <tr className="border-t border-line align-top">
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
          className={`inline-flex rounded-full border px-2 py-1 text-[0.62rem] font-semibold ${toneForOutcome(entry.outcome)}`}
        >
          {titleCase(entry.validationOutcome ?? entry.outcome)}
        </span>
      </td>
      <td className="px-3 py-3">
        <AssessmentScore entry={entry} />
      </td>
      <td className="px-3 py-3">
        <AssessmentConclusion run={run} entry={entry} />
      </td>
    </tr>
  )
}

function AssessmentCard({
  run,
  entry,
}: {
  run: AssessmentRunView
  entry: AssessmentEntry
}) {
  return (
    <article className="grid gap-3 rounded-lg border border-line bg-panel p-3">
      <div className="flex items-start justify-between gap-3">
        <AssessmentIdentity entry={entry} />
        <span
          className={`shrink-0 rounded-full border px-2 py-1 text-[0.62rem] font-semibold ${toneForOutcome(entry.outcome)}`}
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
      <AssessmentConclusion run={run} entry={entry} />
    </article>
  )
}

function AiList({ label, items }: { label: string; items?: string[] }) {
  if (!items?.length) return null
  return (
    <section className="grid gap-2">
      <h5 className="m-0 text-[0.62rem] font-semibold uppercase tracking-[0.06em] text-ink-muted">
        {label}
      </h5>
      <ul className="m-0 grid gap-1.5 pl-4 text-sm leading-5 text-ink-soft">
        {items.map((item) => (
          <li key={item}>{item}</li>
        ))}
      </ul>
    </section>
  )
}

function FinalAiCard({ run }: { run: AssessmentRunView }) {
  const assessment = run.finalAssessment
  const result = assessment.result
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
      className={`overflow-hidden rounded-lg border ${
        run.hasAiDisagreement
          ? 'border-warning/50 bg-warning/5'
          : 'border-line bg-panel'
      }`}
    >
      <header className="grid gap-3 border-b border-line p-4 sm:grid-cols-[minmax(0,1fr)_auto]">
        <div className="flex items-start gap-3">
          <BrainCircuit
            className="mt-0.5 shrink-0 text-brand"
            size={19}
            aria-hidden="true"
          />
          <div>
            <span className="section-kicker">Advisory AI conclusion</span>
            <h4 className="m-0 text-lg text-ink">
              {titleCase(result.verdict)}
            </h4>
            <p className="mt-1 mb-0 text-sm leading-5 text-ink-soft">
              {result.summary}
            </p>
          </div>
        </div>
        <div className="grid grid-cols-2 gap-2 text-right">
          <span className="rounded-lg border border-line bg-panel-subtle px-3 py-2">
            <small className="block text-[0.6rem] uppercase text-ink-muted">
              Quality
            </small>
            <strong className="text-base text-ink">
              {result.quality_score}
            </strong>
          </span>
          <span className="rounded-lg border border-line bg-panel-subtle px-3 py-2">
            <small className="block text-[0.6rem] uppercase text-ink-muted">
              Confidence
            </small>
            <strong className="text-base text-ink">
              {formatConfidence(result.confidence)}
            </strong>
          </span>
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
      <div className="grid gap-5 p-4 lg:grid-cols-2">
        <AiList label="AI-reported facts" items={result.facts} />
        <AiList label="Strengths" items={result.strengths} />
        <AiList label="Concerns" items={result.concerns} />
        <AiList label="Limitations" items={result.limitations} />
        <section className="grid gap-2 lg:col-span-2">
          <h5 className="m-0 text-[0.62rem] font-semibold uppercase tracking-[0.06em] text-ink-muted">
            Recommendation
          </h5>
          <p className="m-0 border-l-2 border-brand pl-3 text-sm leading-5 text-ink-soft">
            {result.recommendation}
          </p>
        </section>
        <section className="grid gap-2 lg:col-span-2">
          <h5 className="m-0 text-[0.62rem] font-semibold uppercase tracking-[0.06em] text-ink-muted">
            Evidence supporting this AI conclusion
          </h5>
          <EvidenceLinks
            run={run}
            references={result.evidence ?? []}
            label="Reference"
          />
        </section>
        <section className="grid gap-2 border-t border-line pt-3 lg:col-span-2">
          <h5 className="m-0 text-[0.62rem] font-semibold uppercase tracking-[0.06em] text-ink-muted">
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

function EvidenceRegister({ run }: { run: AssessmentRunView }) {
  if (run.evidence.length === 0) {
    return (
      <p className="m-0 text-sm text-ink-muted">
        No approved evidence references were attached to this run.
      </p>
    )
  }
  return (
    <div className="grid gap-2 sm:grid-cols-2">
      {run.evidence.map((reference) => (
        <article
          id={evidenceId(run, reference)}
          key={`${reference.artifact_id}:${reference.artifact_sha256}:${reference.locator ?? ''}`}
          className="scroll-mt-24 rounded-lg border border-line bg-panel-subtle p-3 target:border-brand target:bg-brand-soft"
        >
          <strong className="block break-all font-mono text-xs text-ink">
            {reference.artifact_id}
          </strong>
          <code
            className="mt-1 block break-all text-[0.62rem] text-ink-muted"
            title={reference.artifact_sha256}
          >
            {shortHash(reference.artifact_sha256)}
          </code>
          {reference.locator && (
            <code className="mt-1 block break-all text-[0.62rem] text-ink-soft">
              {reference.locator}
            </code>
          )}
        </article>
      ))}
    </div>
  )
}

function RunAssessment({
  run,
  filter,
  initiallyOpen,
}: {
  run: AssessmentRunView
  filter: AssessmentFilter
  initiallyOpen: boolean
}) {
  const entries = run.assessments.filter((entry) =>
    matchesAssessmentFilter(entry, filter),
  )
  const ai = run.finalAssessment
  const aiLabel = ai.result?.verdict ?? ai.availability
  const objectiveFailure = run.systemStatus !== 'passed'

  return (
    <details
      className={`overflow-hidden rounded-lg border bg-panel ${
        objectiveFailure ? 'border-danger/40' : 'border-line'
      }`}
      open={initiallyOpen || objectiveFailure || run.hasAiDisagreement}
    >
      <summary className="grid min-h-16 cursor-pointer list-none items-center gap-3 px-4 py-3 sm:grid-cols-[minmax(0,1fr)_auto]">
        <span className="min-w-0">
          <strong className="block text-sm text-ink">
            {titleCase(run.scenarioId)} · scenario v{run.scenarioVersion}
          </strong>
          <span className="mt-1 block break-all font-mono text-[0.62rem] text-ink-muted">
            {run.subjectId} · run {run.runId} · attempt {run.attemptId}
          </span>
        </span>
        <span className="flex flex-wrap gap-1.5">
          <span
            className={`rounded-full border px-2 py-1 text-[0.62rem] font-semibold ${toneForOutcome(run.systemStatus)}`}
          >
            System: {titleCase(run.systemStatus)}
          </span>
          <span
            className={`rounded-full border px-2 py-1 text-[0.62rem] font-semibold ${toneForOutcome(aiLabel)}`}
          >
            AI: {titleCase(aiLabel)}
          </span>
        </span>
      </summary>
      <div className="grid gap-5 border-t border-line p-4">
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
          <AssessmentMatrix run={run} entries={entries} />
        </section>

        <FinalAiCard run={run} />

        <details className="rounded-lg border border-line bg-panel-subtle">
          <summary className="flex min-h-11 cursor-pointer list-none items-center gap-2 px-4 py-3 text-sm font-semibold text-ink">
            <Database size={15} aria-hidden="true" /> Evidence register ·{' '}
            {run.evidence.length}
          </summary>
          <div className="border-t border-line p-4">
            <EvidenceRegister run={run} />
          </div>
        </details>
      </div>
    </details>
  )
}

export function AssessmentPanel({
  model,
  filter,
  onFilter,
}: {
  model: AssessmentWorkspaceModel
  filter: AssessmentFilter
  onFilter?: (filter: AssessmentFilter) => void
}) {
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
    <div className="grid gap-4">
      <fieldset className="m-0 flex flex-wrap gap-2 border-0 p-0">
        <legend className="sr-only">Filter assessment matrix</legend>
        {FILTERS.map((candidate) => (
          <button
            key={candidate.id}
            className={`min-h-10 rounded-full border px-3 py-2 text-xs font-semibold transition ${
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
      <p className="m-0 text-xs text-ink-muted" role="status">
        {filter === 'low_confidence'
          ? `Low confidence means below ${Math.round(LOW_CONFIDENCE_THRESHOLD * 100)}%.`
          : `${counts[filter]} assessment${counts[filter] === 1 ? '' : 's'} match this view.`}
      </p>
      <div className="grid gap-3">
        {model.runs.map((run, index) => (
          <RunAssessment
            key={run.key}
            run={run}
            filter={filter}
            initiallyOpen={index === 0}
          />
        ))}
      </div>
    </div>
  )
}

export function AssessmentWorkspace({ executionId }: { executionId: string }) {
  const [filter, setFilter] = useState<AssessmentFilter>('all')
  const [model, setModel] = useState<AssessmentWorkspaceModel | null>(null)
  const [error, setError] = useState<Error | null>(null)

  useEffect(() => {
    let active = true
    let timeout: number | undefined
    setModel(null)
    setError(null)

    const showDetail = (detail: DashboardExecutionDetail | undefined) => {
      if (!active) return false
      if (!detail) return false
      window.clearTimeout(timeout)
      setModel(buildAssessmentWorkspace(detail))
      return true
    }
    const handleDetail = (event: Event) => {
      const payload = (
        event as CustomEvent<{
          executionId: string
          detail?: DashboardExecutionDetail | null
          availability?: string
          error?: string
        }>
      ).detail
      if (!active || payload?.executionId !== executionId) return
      window.clearTimeout(timeout)
      if (payload.error) {
        setError(new Error(payload.error))
        return
      }
      if (showDetail(payload.detail ?? undefined)) return
      setModel(
        payload.availability === 'unavailable'
          ? { availability: 'unavailable', runs: [] }
          : buildAssessmentWorkspace(undefined),
      )
    }
    window.addEventListener('harness:execution-detail-ready', handleDetail)
    loadLegacyPage('execution')
      .then(() => {
        if (!active) return
        const detail = window.HARNESS_EXECUTION_DETAILS?.[executionId] as
          | DashboardExecutionDetail
          | undefined
        if (showDetail(detail)) return
        const summary = window.HARNESS_EXECUTIONS?.executions?.find(
          (candidate) => candidate.id === executionId,
        )
        if (summary?.availability !== 'full') {
          setModel(buildAssessmentWorkspace(undefined))
          return
        }
        timeout = window.setTimeout(() => {
          if (active) {
            setError(
              new Error(
                'The retained assessment detail did not finish loading.',
              ),
            )
          }
        }, 10_000)
      })
      .catch((cause: unknown) => {
        if (!active) return
        setError(cause instanceof Error ? cause : new Error(String(cause)))
      })
    return () => {
      active = false
      window.clearTimeout(timeout)
      window.removeEventListener('harness:execution-detail-ready', handleDetail)
    }
  }, [executionId])

  const content = useMemo(() => {
    if (error) {
      return (
        <div
          className="rounded-lg border border-danger/30 bg-danger/5 p-4 text-sm text-danger"
          role="alert"
        >
          Assessment presentation could not be loaded: {error.message}
        </div>
      )
    }
    if (!model) {
      return (
        <div className="grid gap-2" aria-busy="true" role="status">
          <span className="sr-only">Loading assessment evidence</span>
          <div className="h-16 animate-pulse rounded-lg border border-line bg-panel-raised" />
          <div className="h-36 animate-pulse rounded-lg border border-line bg-panel-raised" />
        </div>
      )
    }
    return (
      <AssessmentPanel model={model} filter={filter} onFilter={setFilter} />
    )
  }, [error, filter, model])

  return content
}
