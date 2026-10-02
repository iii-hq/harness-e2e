import {
  CheckCircle2,
  CircleX,
  ExternalLink,
  FileText,
  RotateCcw,
  ShieldCheck,
  TriangleAlert,
} from 'lucide-react'
import { type ReactNode, useId, useState } from 'react'
import { AssessmentDetailContent } from '@/components/AssessmentWorkspace'
import { InvestigationAction } from '@/components/InvestigationAction'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import { runCriteria } from '@/components/ScenarioMatrix'
import { SemanticTestFlow } from '@/components/SemanticTestFlow'
import { buttonClassName, PageHeader, StatusBadge } from '@/design-system'
import type { AssessmentRunView } from '@/lib/assessment-view'
import type {
  DashboardExecutionDetail,
  JsonObject,
} from '@/lib/dashboard-data-source'
import { shortDefinition } from '@/lib/definition-digest'
import { sentenceCase } from '@/lib/format'
import { formatFull, formatSpan, formatUsd } from './ExecutionTotals'
import './execution-page.css'
import { copyText } from '@/lib/clipboard'

function objects(value: unknown): JsonObject[] {
  return Array.isArray(value)
    ? value.filter(
        (item): item is JsonObject =>
          Boolean(item) && typeof item === 'object' && !Array.isArray(item),
      )
    : []
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value ? value : null
}

function bytes(value: unknown) {
  if (typeof value !== 'number') return null
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`
  return `${(value / 1024 / 1024).toFixed(1)} MiB`
}

function sha(value: unknown) {
  const hash = text(value)
  return hash ? `sha ${hash.replace('sha256:', '').slice(0, 8)}` : null
}

/** The reason without the description it often repeats first. */
function reasonOnly(c: { reason: string; description: string }) {
  const reason = c.reason.trim()
  if (!reason || reason === c.description) return ''
  return reason.startsWith(c.description)
    ? reason.slice(c.description.length).trim()
    : reason
}

function basename(path: string) {
  return path.split('/').pop() ?? path
}

const DIMENSIONS: Record<string, [string, string]> = {
  deliverable: ['Deliverable', 'what the Worker delivered'],
  structural_integrity: [
    'Structural integrity',
    'checked against the Harness oracle',
  ],
  efficiency: ['Efficiency', 'how the run spent its budget'],
  robustness: ['Robustness', 'how it held up under faults'],
  e2e_infrastructure: ['E2E infrastructure', 'the harness around the run'],
}

/** The run projection of one retained run, from the execution detail. */
export function runProjection(
  detail: DashboardExecutionDetail | null | undefined,
  run: AssessmentRunView,
): JsonObject | null {
  if (!detail) return null
  for (const record of detail.reports) {
    if (record.subject_id !== run.subjectId) continue
    for (const scenario of record.report?.scenarios ?? []) {
      if (scenario.scenario_id !== run.scenarioId) continue
      const found = scenario.runs.find(
        (candidate) =>
          candidate.run_id === run.runId &&
          candidate.attempt_id === run.attemptId,
      )
      if (found) return found as JsonObject
    }
  }
  return null
}

/** A worker's phrase as a sentence: its full stop when it has none. */
function sentence(value: string) {
  return /[.!?…]$/.test(value.trim()) ? value : `${value}.`
}

function AuditSection({ flags }: { flags: JsonObject[] }) {
  const [open, setOpen] = useState(false)
  const listId = useId()
  const causes = new Map<string, JsonObject[]>()
  for (const flag of flags) {
    const kind = text(flag.kind) ?? 'audit'
    causes.set(kind, [...(causes.get(kind) ?? []), flag])
  }
  const functions = new Map<string, number>()
  for (const flag of flags)
    for (const evidence of objects(flag.evidence)) {
      const fn = text(evidence.function_id)
      if (fn) functions.set(fn, (functions.get(fn) ?? 0) + 1)
    }
  const first = flags[0]
  return (
    <section className="er-audit" aria-label="Audit" data-audit>
      <div className="er-audit-head">
        <TriangleAlert size={16} aria-hidden="true" className="ep-warn-icon" />
        <div>
          <p className="ep-strong">
            {flags.length} audit {flags.length === 1 ? 'warning' : 'warnings'},{' '}
            {causes.size === 1 ? 'one cause' : `${causes.size} causes`}
          </p>
          <p className="ep-faint">
            {sentence(text(first?.summary) ?? [...causes.keys()].join(', '))}{' '}
            Audit flags don’t change the score.
          </p>
        </div>
        <button
          type="button"
          className="ds-button ds-button-quiet ds-button-compact"
          aria-expanded={open}
          aria-controls={listId}
          onClick={() => setOpen(!open)}
        >
          {open ? 'Hide flags' : 'Show flags'}
        </button>
      </div>
      {functions.size > 0 ? (
        <div className="er-chips">
          {[...functions].map(([fn, count]) => (
            <span className="ep-fact ep-mono" key={fn}>
              {fn}
              {count > 1 ? <span className="ep-faint">×{count}</span> : null}
            </span>
          ))}
        </div>
      ) : null}
      {open ? (
        <ol id={listId} className="er-flags">
          {flags.map((flag, index) => {
            const evidence = objects(flag.evidence)[0]
            return (
              // biome-ignore lint/suspicious/noArrayIndexKey: flags have no id and never reorder
              <li key={index} className="ep-mono">
                {[
                  text(flag.kind),
                  text(evidence?.detail),
                  text(evidence?.function_id)
                    ? `via ${text(evidence?.function_id)}`
                    : null,
                ]
                  .filter(Boolean)
                  .join(' · ')}
              </li>
            )
          })}
        </ol>
      ) : null}
    </section>
  )
}

/** One retained run, as its own page (canvas: Evidence record). */
export function EvidenceRecordPage({
  run,
  detail,
  backHref,
  backLabel = 'Back to the execution',
  transcriptHref,
  onRerun,
  onOpenFile,
  children,
}: {
  run: AssessmentRunView
  detail?: DashboardExecutionDetail | null
  backHref: string
  /** "Back to comparison" when it was opened from one. */
  backLabel?: string
  transcriptHref?: string
  onRerun?: () => void
  /** Open a file the run's report declares (evidence or a deliverable). */
  onOpenFile?: (path: string) => Promise<void>
  /** The run's screenshots. */
  children?: ReactNode
}) {
  const [fileError, setFileError] = useState<string | null>(null)
  const [copied, setCopied] = useState<string | null>(null)
  const projection = runProjection(detail, run)
  const criteria = runCriteria(projection)
  const met = criteria.filter((c) => c.awarded >= c.possible).length
  const awarded = criteria.reduce((sum, c) => sum + c.awarded, 0)
  const possible = criteria.reduce((sum, c) => sum + c.possible, 0)
  const flags = objects((projection?.audit as JsonObject | undefined)?.flags)
  const deliverables = objects(projection?.deliverables)
  const files = objects(projection?.evidence)
  const completion = text(projection?.completion)
  const technical = text(projection?.technical)
  const passed = run.systemStatus === 'passed'
  const resultLine = [
    run.score === null
      ? null
      : `${Math.round(run.score * 10) / 10}/${possible || 100}`,
    criteria.length ? `${met} of ${criteria.length} criteria met` : null,
    completion ? `task ${completion}` : null,
    technical ? `technically ${technical}` : null,
  ]
    .filter(Boolean)
    .join(' · ')
  const dimensionOf = new Map(
    run.assessments.map((entry) => [entry.criterionId, entry.dimension]),
  )
  // A criterion's evidence (Evidence per criterion), from the first of its
  // references that resolves: the transcript as its page, another file by
  // opening it; nothing when none is at hand.
  const evidenceLink = (criterionId: string) => {
    const refs = run.assessments
      .filter((entry) => entry.criterionId === criterionId)
      .flatMap((entry) => entry.evidence ?? [])
    const label = `Evidence for ${criterionId}`
    for (const ref of refs) {
      if (ref.artifact_id === 'transcript' && transcriptHref && run.transcript)
        return (
          <a
            className="er-evidence-link"
            href={transcriptHref}
            aria-label={label}
          >
            Evidence
          </a>
        )
      const path = text(
        files.find((file) => file.sha256 === ref.artifact_sha256)?.path,
      )
      if (path && onOpenFile)
        return (
          <button
            type="button"
            className="er-evidence-link"
            aria-label={label}
            onClick={() => open(path)}
          >
            Evidence
          </button>
        )
    }
    return null
  }
  const groups = new Map<string, typeof criteria>()
  for (const criterion of criteria) {
    const key = dimensionOf.get(criterion.id) ?? 'criteria'
    groups.set(key, [...(groups.get(key) ?? []), criterion])
  }
  const cost = (projection?.cost as JsonObject | undefined)?.total_usd
  const m = run.metrics
  const facts: Array<[string, string]> = [
    [
      'Score',
      run.score === null
        ? '—'
        : `${Math.round(run.score * 10) / 10}/${possible || 100}`,
    ],
    ['Runtime', formatSpan(m.durationMs)],
    ['Cost', formatUsd(typeof cost === 'number' ? cost : null)],
    ['Input tokens', formatFull(m.inputTokens)],
    ['Output tokens', formatFull(m.outputTokens)],
    ['Cache read', formatFull(m.cacheReadTokens)],
    ['Cache written', formatFull(m.cacheWriteTokens)],
    [
      'Reasoning tokens',
      m.reasoningTokens === null
        ? 'not reported'
        : formatFull(m.reasoningTokens),
    ],
    ['Turns', formatFull(m.turns)],
    ['Function calls', formatFull(m.functionCalls)],
    ['Function errors', formatFull(m.functionCallErrors)],
    ['Sessions', formatFull(m.sessions)],
    ['Model', run.subjectId],
    ['Definition', shortDefinition(run.behaviorSha256) ?? '—'],
    [
      'Run',
      `${run.runId.slice(0, 12)}… · attempt ${Number(projection?.attempt_number ?? 1)}`,
    ],
  ]
  const open = (path: string) => {
    if (!onOpenFile) return
    setFileError(null)
    onOpenFile(path).catch((cause) =>
      setFileError(cause instanceof Error ? cause.message : String(cause)),
    )
  }
  const copy = (path: string) => {
    void copyText(path).then((ok) => {
      if (!ok) return
      setCopied(path)
      window.setTimeout(() => setCopied(null), 1500)
    })
  }
  const workflow = objects(projection?.semantic_tests).length > 0
  return (
    <div className="ds-root page-shell ep-page" data-evidence-page>
      <PageHeader
        variant="detail"
        mono
        back={{ label: backLabel, href: backHref }}
        context="Evidence record"
        title={run.scenarioId}
        summary={
          <>
            <StatusBadge
              status={passed ? 'passed' : 'failed'}
              label={passed ? 'Passed' : sentenceCase(run.systemStatus)}
            />{' '}
            <span data-result-line>{resultLine}</span>
          </>
        }
        actions={
          <>
            {transcriptHref && run.transcript ? (
              <a
                className={buttonClassName({
                  variant: 'quiet',
                  className: 'no-underline',
                })}
                href={transcriptHref}
              >
                <FileText size={15} aria-hidden="true" />
                Transcript
              </a>
            ) : null}
            <ScenarioChatAction
              multipleOnly
              buttonClass={buttonClassName({ variant: 'quiet' })}
              detail={detail}
              scenarioId={run.scenarioId}
              subjectId={run.subjectId}
              runId={run.runId}
            />
            {detail ? (
              <InvestigationAction
                label="Investigate"
                buttonClass={buttonClassName({ variant: 'quiet' })}
                executionId={detail.id}
                focus={{
                  scenarioId: run.scenarioId,
                  subjectId: run.subjectId,
                  runId: run.runId || undefined,
                }}
              />
            ) : null}
            {onRerun ? (
              <button
                type="button"
                className={buttonClassName({ variant: 'secondary' })}
                onClick={onRerun}
              >
                <RotateCcw size={15} aria-hidden="true" />
                Run again
              </button>
            ) : null}
          </>
        }
      />
      {fileError ? (
        <p className="ep-faint ep-warn-text" role="status">
          Could not open the file: {fileError}
        </p>
      ) : null}
      <div className="er-layout">
        <div className="er-main">
          {flags.length > 0 ? <AuditSection flags={flags} /> : null}
          {criteria.length > 0 ? (
            <section aria-labelledby="er-criteria" data-criteria>
              <div className="ep-section-head">
                <h2 id="er-criteria" className="ep-h2">
                  Criteria
                </h2>
                <span className="ep-faint">
                  {awarded} of {possible} points ·{' '}
                  {met === criteria.length
                    ? 'every criterion met'
                    : `${criteria.length - met} lost points`}
                </span>
              </div>
              {[...groups].map(([key, items]) => {
                const [label, help] = DIMENSIONS[key] ?? ['Criteria', '']
                const got = items.reduce((sum, c) => sum + c.awarded, 0)
                const max = items.reduce((sum, c) => sum + c.possible, 0)
                return (
                  <section className="er-group" aria-label={label} key={key}>
                    <div className="er-group-head">
                      <span className="ep-strong">{label}</span>
                      <span className="ep-faint">{help}</span>
                      <span className="ep-mono">
                        {got}/{max}
                      </span>
                    </div>
                    {items.map((c) => {
                      const ok = c.awarded >= c.possible
                      return (
                        <div
                          className="er-criterion"
                          key={c.id}
                          data-criterion={c.id}
                        >
                          {ok ? (
                            <CheckCircle2
                              size={16}
                              aria-hidden="true"
                              className="er-ok"
                            />
                          ) : (
                            <CircleX
                              size={16}
                              aria-hidden="true"
                              className="er-miss"
                            />
                          )}
                          <div className="er-criterion-copy">
                            <span className="ep-strong">
                              {c.description || c.id}
                              {c.gate ? (
                                <span className={ok ? 'ep-faint' : 'ep-gate'}>
                                  {' '}
                                  · hard gate
                                </span>
                              ) : null}
                            </span>
                            {c.description ? (
                              <span className="ep-mono ep-faint">{c.id}</span>
                            ) : null}
                            {reasonOnly(c) ? (
                              <span className="ep-faint">{reasonOnly(c)}</span>
                            ) : null}
                          </div>
                          <span className="er-points">
                            <span className="ep-score-bar" aria-hidden="true">
                              <span
                                style={{
                                  width: `${c.possible ? Math.round((c.awarded / c.possible) * 100) : 0}%`,
                                }}
                              />
                            </span>
                            <span className="ep-mono">
                              {c.awarded}/{c.possible}
                            </span>
                          </span>
                          {evidenceLink(c.id)}
                        </div>
                      )
                    })}
                  </section>
                )
              })}
            </section>
          ) : (
            <AssessmentDetailContent run={run} entries={run.assessments} />
          )}
          {deliverables.length > 0 ? (
            <section aria-labelledby="er-deliverable" data-deliverables>
              <h2 id="er-deliverable" className="ep-h2">
                {deliverables.length === 1 ? 'Deliverable' : 'Deliverables'}
              </h2>
              {deliverables.map((deliverable) => {
                const artifact = objects([deliverable.artifact])[0] ?? {}
                const path = text(artifact.path)
                const format = (
                  text(deliverable.content_format) ??
                  text(artifact.media_type) ??
                  'file'
                ).toUpperCase()
                const invariants = objects(deliverable.invariants)
                const held = invariants.filter(
                  (inv) =>
                    inv.passed === true ||
                    inv.holds === true ||
                    inv.outcome === 'passed',
                ).length
                const shots = objects(deliverable.screenshots).length
                return (
                  <div
                    className="er-deliverable"
                    key={text(deliverable.id) ?? path ?? format}
                  >
                    <div className="er-deliverable-head">
                      <span className="ep-mono ep-strong">
                        {text(deliverable.id)}
                      </span>
                      <span className="ep-faint">{text(deliverable.kind)}</span>
                      <span className="ep-faint ep-mono">
                        {[
                          format,
                          bytes(artifact.size_bytes),
                          sha(artifact.sha256),
                        ]
                          .filter(Boolean)
                          .join(' · ')}
                      </span>
                      {path && onOpenFile ? (
                        <button
                          type="button"
                          className="ds-button ds-button-quiet ds-button-compact er-push"
                          onClick={() => open(path)}
                        >
                          Open {format === 'JSON' ? 'JSON' : 'file'}
                        </button>
                      ) : null}
                      {path ? (
                        <button
                          type="button"
                          className={`ds-button ds-button-quiet ds-button-compact ${onOpenFile ? '' : 'er-push'}`}
                          onClick={() => copy(path)}
                        >
                          {copied === path ? 'Copied' : 'Copy path'}
                        </button>
                      ) : null}
                    </div>
                    <div className="er-checks">
                      <Check
                        ok={deliverable.schema_valid === true}
                        label={
                          deliverable.schema_valid === true
                            ? 'Schema valid'
                            : 'Schema not valid'
                        }
                      />
                      <Check
                        ok={deliverable.provenance_valid === true}
                        label={
                          deliverable.provenance_valid === true
                            ? 'Provenance valid'
                            : 'Provenance not valid'
                        }
                      />
                      {invariants.length > 0 ? (
                        <Check
                          ok={held === invariants.length}
                          label={`${held} of ${invariants.length} invariants hold`}
                        />
                      ) : null}
                      {shots > 0 ? (
                        <Check
                          ok
                          label={`${shots} ${shots === 1 ? 'screenshot' : 'screenshots'}`}
                        />
                      ) : null}
                    </div>
                  </div>
                )
              })}
            </section>
          ) : null}
          {children ? (
            <section aria-labelledby="er-shots" data-evidence-shots>
              <div className="ep-section-head">
                <h2 id="er-shots" className="ep-h2">
                  Screenshots
                </h2>
                <span className="ep-faint">Select one to open the viewer</span>
              </div>
              {children}
            </section>
          ) : null}
          {workflow && detail ? (
            <SemanticTestFlow
              detail={{
                ...detail,
                reports: detail.reports.filter(
                  (r) => r.scenario_id === run.scenarioId,
                ),
              }}
            />
          ) : null}
        </div>
        <aside className="er-aside" aria-label="Run facts">
          <dl className="er-facts">
            {facts.map(([label, value]) => (
              <div key={label}>
                <dt>{label}</dt>
                <dd title={value}>{value}</dd>
              </div>
            ))}
          </dl>
          {files.length > 0 ? (
            <div className="er-files">
              <h2 className="ep-h2">Evidence files</h2>
              {files.map((file) => {
                const path = text(file.path)
                if (!path) return null
                return (
                  <div className="er-file" key={path}>
                    <span className="er-file-copy">
                      <span className="ep-mono">{basename(path)}</span>
                      <span className="ep-faint ep-mono">
                        {[bytes(file.size_bytes), sha(file.sha256)]
                          .filter(Boolean)
                          .join(' · ')}
                      </span>
                    </span>
                    {onOpenFile ? (
                      <button
                        type="button"
                        className="ds-button ds-button-quiet ds-button-compact"
                        aria-label={`Open ${basename(path)}`}
                        onClick={() => open(path)}
                      >
                        <ExternalLink size={14} aria-hidden="true" />
                      </button>
                    ) : null}
                  </div>
                )
              })}
            </div>
          ) : null}
        </aside>
      </div>
    </div>
  )
}

function Check({ ok, label }: { ok: boolean; label: string }) {
  return (
    <span className={`er-check ${ok ? 'er-ok' : 'er-miss'}`}>
      {ok ? (
        <ShieldCheck size={14} aria-hidden="true" />
      ) : (
        <CircleX size={14} aria-hidden="true" />
      )}
      {label}
    </span>
  )
}
