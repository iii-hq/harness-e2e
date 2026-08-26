import { useCallback, useEffect, useMemo, useState } from 'react'
import type {
  DashboardDataBridge,
  ImprovementLoopRecord,
  ImprovementLoopReport,
  JsonObject,
} from '@/lib/dashboard-data-source'

const ACTIVE_PHASES = new Set([
  'preflight',
  'baseline_running',
  'advising',
  'patching',
  'checking',
  'candidate_running',
  'comparing',
  'revising',
])

function asObject(value: unknown): JsonObject | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as JsonObject)
    : null
}

function pretty(value: unknown) {
  return JSON.stringify(value, null, 2)
}

function proposalFrom(report: ImprovementLoopReport | null) {
  const latest = report?.artifacts.iterations.at(-1)
  return asObject(latest?.proposal)
}

export function AdvisorAnswer({ report }: { report: ImprovementLoopReport }) {
  const latest = report.artifacts.iterations.at(-1)
  const proposal = asObject(latest?.proposal)
  const response = asObject(latest?.advisor_response)
  const outcome = asObject(response?.outcome)
  const hypothesis = asObject(proposal?.hypothesis)
  const action = asObject(proposal?.action)
  const objective = asObject(proposal?.objective)
  const evidence = Array.isArray(hypothesis?.evidence)
    ? hypothesis.evidence
    : []

  return (
    <section
      className="improvement-answer"
      aria-labelledby="advisor-answer-title"
    >
      <div className="panel-heading">
        <div>
          <div className="section-kicker">AI Advisory · consultative</div>
          <h3 id="advisor-answer-title">What should change in Harness</h3>
        </div>
      </div>
      {proposal ? (
        <div className="improvement-answer-grid">
          <article>
            <span>One causal hypothesis</span>
            <strong>{String(hypothesis?.summary ?? 'Unavailable')}</strong>
            <small>
              Root cause: {String(hypothesis?.root_cause ?? 'unknown')} ·
              confidence {String(hypothesis?.confidence ?? '—')}
            </small>
          </article>
          <article>
            <span>Harness behavior to change</span>
            <strong>{String(action?.behavior_change ?? 'Unavailable')}</strong>
            <small>
              Surfaces:{' '}
              {Array.isArray(action?.surfaces)
                ? action.surfaces.map(String).join(', ')
                : '—'}
            </small>
          </article>
          <article>
            <span>Frozen measurable objective</span>
            <strong>
              {String(objective?.metric ?? 'metric')} ·{' '}
              {String(objective?.direction ?? 'direction')}{' '}
              {String(objective?.minimum_change ?? '—')}
            </strong>
            <small>{String(proposal.validation_method ?? '')}</small>
          </article>
          <article>
            <span>Evidence identities</span>
            <strong>{evidence.length} linked artifact references</strong>
            <small>
              {evidence
                .map((item) => {
                  const ref = asObject(item)
                  return `${String(ref?.artifact_id ?? 'artifact')} · ${String(ref?.artifact_sha256 ?? 'sha unavailable')}`
                })
                .join(' | ') || 'No evidence was accepted.'}
            </small>
          </article>
        </div>
      ) : (
        <p className="table-empty">
          {String(
            outcome?.reason ?? 'The Advisor has not closed a proposal yet.',
          )}
        </p>
      )}
    </section>
  )
}

function LoopDetail({
  report,
  busy,
  onAction,
}: {
  report: ImprovementLoopReport
  busy: boolean
  onAction: (action: 'start' | 'resume' | 'cancel') => void
}) {
  const { record } = report
  const proposal = proposalFrom(report)
  const objective = asObject(proposal?.objective)
  const latest = report.artifacts.iterations.at(-1)
  const latestRecord = record.iterations.at(-1)
  const terminal = !ACTIVE_PHASES.has(record.phase) && record.phase !== 'draft'
  const comparison = asObject(latestRecord?.comparison)
  const cases = Array.isArray(comparison?.cases) ? comparison.cases : []
  const targetCases = cases.filter((item) => {
    const key = asObject(asObject(item)?.key)
    return key?.scenario_id === record.spec.target_scenario
  })
  const sentinelCases = cases.filter((item) => {
    const key = asObject(asObject(item)?.key)
    return key?.scenario_id !== record.spec.target_scenario
  })

  return (
    <article className="improvement-detail">
      <div className="panel-heading">
        <div>
          <div className="section-kicker">{record.id}</div>
          <h3>{record.spec.label}</h3>
          <p>
            {record.spec.target_scenario} · {record.spec.runs} samples · base{' '}
            {record.spec.base_revision.slice(0, 12)}
          </p>
        </div>
        <span className="status-badge status-neutral">{record.phase}</span>
      </div>

      <div className="improvement-controls">
        {record.phase === 'draft' && (
          <button
            type="button"
            className="button button-primary"
            disabled={busy}
            onClick={() => onAction('start')}
          >
            Start loop
          </button>
        )}
        {(record.phase === 'failed' ||
          record.phase === 'needs_reconciliation') && (
          <button
            type="button"
            className="button button-primary"
            disabled={busy}
            onClick={() => onAction('resume')}
          >
            Reconcile and resume
          </button>
        )}
        {!terminal && record.phase !== 'draft' && (
          <button
            type="button"
            className="button button-secondary"
            disabled={busy}
            onClick={() => onAction('cancel')}
          >
            Cancel
          </button>
        )}
        <span>
          Cost ${record.consumed_cost_usd.toFixed(4)} · deadline{' '}
          {new Date(record.deadline_at).toLocaleString()}
        </span>
      </div>

      <AdvisorAnswer report={report} />

      <div className="improvement-evidence-grid">
        <section>
          <div className="section-kicker">Target metric</div>
          <h4>{String(objective?.metric ?? 'Waiting for proposal')}</h4>
          <pre>{pretty(targetCases)}</pre>
        </section>
        <section>
          <div className="section-kicker">Sentinel deltas</div>
          <h4>{latestRecord?.decision?.accepted ? 'Accepted' : 'Protected'}</h4>
          <pre>{pretty(sentinelCases)}</pre>
          <div className="section-kicker">Deterministic decision</div>
          <pre>{pretty(latestRecord?.decision ?? {})}</pre>
        </section>
      </div>

      {latest && (
        <section className="improvement-variant">
          <div className="panel-heading">
            <div>
              <div className="section-kicker">Variant {latest.number}</div>
              <h4>{latest.branch}</h4>
            </div>
          </div>
          <div className="improvement-checks">
            {latest.checks.map((check) => (
              <div
                key={`${check.kind}-${String(check.command)}-${String(check.duration_ms)}`}
              >
                <span aria-hidden="true">{check.passed ? '✓' : '×'}</span>
                <strong>{check.kind}</strong>
                <small>{check.summary}</small>
              </div>
            ))}
          </div>
          <details>
            <summary>Candidate diff</summary>
            <pre>{latest.patch || 'No candidate diff persisted yet.'}</pre>
          </details>
        </section>
      )}

      <section className="improvement-timeline">
        <div className="section-kicker">Supervisor timeline</div>
        <ol>
          {record.transitions.map((transition) => (
            <li
              key={`${transition.at}-${transition.phase}-${transition.reason}`}
            >
              <strong>{transition.phase}</strong>
              <span>{transition.reason}</span>
              <time>{new Date(transition.at).toLocaleString()}</time>
            </li>
          ))}
        </ol>
      </section>
    </article>
  )
}

export function ImprovementLoopsPanel({
  bridge,
}: {
  bridge: DashboardDataBridge | null
}) {
  const [loops, setLoops] = useState<ImprovementLoopRecord[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [report, setReport] = useState<ImprovementLoopReport | null>(null)
  const [specSource, setSpecSource] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const enabled = bridge?.mode === 'local' && bridge.improvementLoopEnabled
  const selected = useMemo(
    () => loops.find((loop) => loop.id === selectedId) ?? null,
    [loops, selectedId],
  )

  const load = useCallback(async () => {
    if (!bridge || !enabled) return
    try {
      const response = await bridge.listImprovementLoops()
      setLoops(response.improvement_loops)
      const nextId = selectedId ?? response.improvement_loops[0]?.id ?? null
      setSelectedId(nextId)
      if (nextId) setReport(await bridge.getImprovementLoop(nextId))
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [bridge, enabled, selectedId])

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    if (!selected || !ACTIVE_PHASES.has(selected.phase)) return
    const timer = window.setInterval(() => void load(), 3000)
    return () => window.clearInterval(timer)
  }, [load, selected])

  const select = async (loopId: string) => {
    if (!bridge) return
    setSelectedId(loopId)
    setBusy(true)
    try {
      setReport(await bridge.getImprovementLoop(loopId))
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  const create = async () => {
    if (!bridge) return
    setBusy(true)
    try {
      const parsed = JSON.parse(specSource) as JsonObject
      const record = await bridge.createImprovementLoop(parsed)
      setSpecSource('')
      await load()
      await select(record.id)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  const action = async (name: 'start' | 'resume' | 'cancel') => {
    if (!bridge || !selectedId) return
    setBusy(true)
    try {
      if (name === 'start') await bridge.startImprovementLoop(selectedId)
      if (name === 'resume') await bridge.resumeImprovementLoop(selectedId)
      if (name === 'cancel') await bridge.cancelImprovementLoop(selectedId)
      await load()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section
      className="panel improvement-panel"
      aria-labelledby="improvement-title"
    >
      <div className="panel-heading">
        <div>
          <div className="section-kicker">Protected local supervisor</div>
          <h2 id="improvement-title">Harness improvement loops</h2>
          <p>
            AI proposes one Harness change; frozen E2E evidence accepts,
            rejects, or requests review.
          </p>
        </div>
      </div>

      {!enabled ? (
        <div className="plans-empty">
          <strong>Host mutations are disabled</strong>
          <p>
            Restart the local dashboard with --enable-improvement-loop to create
            or control loops.
          </p>
        </div>
      ) : (
        <>
          {error && <div className="error-callout">{error}</div>}
          <details className="improvement-create">
            <summary>Create from frozen ImprovementLoopSpecV1</summary>
            <textarea
              value={specSource}
              onChange={(event) => setSpecSource(event.target.value)}
              placeholder="Paste the complete JSON spec. Unknown fields and any drift from seed 4404 / 5 runs are rejected."
              rows={8}
            />
            <button
              type="button"
              className="button button-primary"
              disabled={busy || !specSource.trim()}
              onClick={create}
            >
              Validate and create
            </button>
          </details>
          <div className="improvement-layout">
            <nav aria-label="Improvement loops">
              {loops.length ? (
                loops.map((loop) => (
                  <button
                    type="button"
                    key={loop.id}
                    className={loop.id === selectedId ? 'is-active' : ''}
                    onClick={() => void select(loop.id)}
                  >
                    <strong>{loop.spec.label}</strong>
                    <span>{loop.phase}</span>
                    <small>{loop.id}</small>
                  </button>
                ))
              ) : (
                <p>No improvement loops yet.</p>
              )}
            </nav>
            {report ? (
              <LoopDetail
                report={report}
                busy={busy}
                onAction={(name) => void action(name)}
              />
            ) : (
              <div className="plans-empty">
                Select or create a loop to inspect its evidence.
              </div>
            )}
          </div>
        </>
      )}
    </section>
  )
}
