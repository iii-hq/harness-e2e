import { useCallback, useEffect, useMemo, useState } from 'react'
import { ProviderModelDropdown } from '@/components/ProviderModelDropdown'
import { ThemeToggle } from '@/components/ThemeToggle'
import {
  hashForExecution,
  hashForNewPlan,
  hashForPlan,
  hashForPlans,
  hashForWorkspace,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
  type JsonObject,
  type LocalPlan,
} from '@/lib/dashboard-data-source'

type Model = { provider: string; model: string }
type Catalog = { url: string; models: Model[]; scenarios: string[] }

function modelKey(model: Model) {
  return [model.provider, model.model].join('\n')
}

function modelGroups(models: Model[]) {
  const groups = new Map<string, Model[]>()
  for (const model of models) {
    const entries = groups.get(model.provider) ?? []
    if (!entries.some((entry) => entry.model === model.model))
      entries.push(model)
    groups.set(model.provider, entries)
  }
  return [...groups.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([provider, entries]) => ({
      provider,
      models: entries.sort((left, right) =>
        left.model.localeCompare(right.model),
      ),
    }))
}

function catalogValue(value: JsonObject): Catalog {
  const models = Array.isArray(value.models)
    ? value.models.flatMap((candidate) => {
        if (!candidate || typeof candidate !== 'object') return []
        const item = candidate as JsonObject
        return typeof item.provider === 'string' &&
          typeof item.model === 'string'
          ? [{ provider: item.provider, model: item.model }]
          : []
      })
    : []
  return {
    url: typeof value.url === 'string' ? value.url : '',
    models,
    scenarios: Array.isArray(value.scenarios)
      ? value.scenarios.filter(
          (item): item is string => typeof item === 'string',
        )
      : [],
  }
}

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

function scenarioName(scenario: string) {
  return scenario
    .replace(/[_.]+/g, ' ')
    .replace(/\b\w/g, (character) => character.toUpperCase())
}

function stateLabel(plan: LocalPlan) {
  if (
    plan.locked &&
    plan.state === 'draft' &&
    plan.incomplete_execution_ids.length
  )
    return 'Baseline attempt incomplete · retry available'
  return plan.state.replaceAll('_', ' ')
}

type PlanRunRole = 'baseline' | 'candidate'

type PlanRunFeedback = {
  role: PlanRunRole
  phase: 'starting' | 'running' | 'error'
  message: string
  executionId: string | null
}

type PlanNextAction = {
  title: string
  detail: string
  role: PlanRunRole | null
  actionLabel: string
  executionId: string | null
  state: 'ready' | 'running' | 'complete'
}

function roleLabel(role: PlanRunRole) {
  return role === 'baseline' ? 'Baseline' : 'Candidate'
}

function isRoleRunning(plan: LocalPlan, role: PlanRunRole) {
  return plan.state === `${role}_running`
}

function nextPlanAction(plan: LocalPlan): PlanNextAction {
  if (isRoleRunning(plan, 'baseline')) {
    return {
      title: 'Baseline is running',
      detail:
        'The locked scope is executing. Wait for its report before starting a candidate.',
      role: null,
      actionLabel: 'View active execution',
      executionId: plan.last_attempt_id,
      state: 'running',
    }
  }
  if (isRoleRunning(plan, 'candidate')) {
    return {
      title: 'Candidate is running',
      detail:
        'The same locked scope is executing against the captured baseline. This page refreshes automatically.',
      role: null,
      actionLabel: 'View active execution',
      executionId: plan.last_attempt_id,
      state: 'running',
    }
  }
  if (!plan.baseline_execution_id) {
    const retry = plan.incomplete_execution_ids.length > 0
    return {
      title: retry ? 'Retry the baseline' : 'Capture the baseline',
      detail: retry
        ? 'The previous attempt did not produce a report. Retry the same locked scope.'
        : 'Run this scope before the Harness change. Starting it freezes the scope, seeds and policy.',
      role: 'baseline',
      actionLabel: retry ? 'Retry baseline' : 'Run baseline',
      executionId: null,
      state: 'ready',
    }
  }
  if (plan.candidate_execution_ids.length > 0) {
    const latestCandidate = plan.candidate_execution_ids.at(-1) ?? null
    return {
      title: 'Candidate results are ready',
      detail:
        'Review the latest execution first. You can run another candidate later with this same locked scope.',
      role: 'candidate',
      actionLabel: 'View latest candidate',
      executionId: latestCandidate,
      state: 'complete',
    }
  }
  return {
    title: 'Run the candidate',
    detail:
      'Make the Harness change, then rerun this exact scope to produce a local comparison.',
    role: 'candidate',
    actionLabel: 'Run candidate',
    executionId: null,
    state: 'ready',
  }
}

export function PlanLifecycle({
  plan,
  starting,
  feedback,
  onStart,
}: {
  plan: LocalPlan
  starting: PlanRunRole | null
  feedback: PlanRunFeedback | null
  onStart: (role: PlanRunRole) => void
}) {
  const runningRole: PlanRunRole | null = isRoleRunning(plan, 'baseline')
    ? 'baseline'
    : isRoleRunning(plan, 'candidate')
      ? 'candidate'
      : null
  const activeFeedback =
    feedback && feedback.phase !== 'running' ? feedback : null
  const nextAction = nextPlanAction(plan)
  const canStart =
    nextAction.role !== null && starting === null && runningRole === null

  return (
    <section
      className="panel plan-lifecycle-panel"
      aria-labelledby="plan-lifecycle-title"
    >
      <div className="panel-heading">
        <div>
          <div className="section-kicker">Execution lifecycle</div>
          <h2 id="plan-lifecycle-title">Baseline → candidate</h2>
        </div>
        {plan.locked && <span className="status-pill status-pass">Locked</span>}
      </div>
      <div
        className={`plan-next-action plan-next-action-${nextAction.state}`}
        aria-live="polite"
      >
        <div>
          <span className="section-kicker">Next action</span>
          <h3>{nextAction.title}</h3>
          <p>{nextAction.detail}</p>
        </div>
        <div className="plan-next-action-controls">
          {nextAction.executionId ? (
            <a
              className="button button-primary"
              href={hashForExecution(nextAction.executionId)}
            >
              {nextAction.actionLabel}
            </a>
          ) : nextAction.role ? (
            <button
              className="button button-primary"
              type="button"
              onClick={() => onStart(nextAction.role as PlanRunRole)}
              disabled={!canStart}
            >
              {starting === nextAction.role
                ? `Starting ${roleLabel(nextAction.role).toLowerCase()}…`
                : nextAction.actionLabel}
            </button>
          ) : null}
          {nextAction.state === 'complete' && (
            <button
              className="button button-secondary"
              type="button"
              onClick={() => onStart('candidate')}
              disabled={starting !== null || runningRole !== null}
            >
              Run another candidate
            </button>
          )}
        </div>
      </div>
      {activeFeedback && (
        <div
          className={`plan-run-feedback plan-run-feedback-${activeFeedback.phase}`}
          role={activeFeedback.phase === 'error' ? 'alert' : 'status'}
          aria-live="polite"
        >
          <span aria-hidden="true">
            {activeFeedback.phase === 'error' ? '!' : '•'}
          </span>
          <div>
            <strong>{activeFeedback.message}</strong>
            {activeFeedback.executionId && (
              <a href={hashForExecution(activeFeedback.executionId)}>
                Open active execution →
              </a>
            )}
          </div>
        </div>
      )}
      <div className="plan-lifecycle">
        <article
          className={
            plan.baseline_execution_id
              ? 'plan-step complete'
              : isRoleRunning(plan, 'baseline')
                ? 'plan-step running'
                : 'plan-step active'
          }
        >
          <span>01</span>
          <div>
            <strong>Baseline</strong>
            <small>
              {plan.baseline_execution_id
                ? 'Completed report captured'
                : isRoleRunning(plan, 'baseline')
                  ? 'Collecting the baseline report now'
                  : 'Run once before the change'}
            </small>
            {plan.baseline_execution_id && (
              <a href={hashForExecution(plan.baseline_execution_id)}>
                View baseline execution
              </a>
            )}
            {!plan.baseline_execution_id &&
              isRoleRunning(plan, 'baseline') &&
              plan.last_attempt_id && (
                <a href={hashForExecution(plan.last_attempt_id)}>
                  View active execution
                </a>
              )}
          </div>
        </article>
        <article
          className={
            plan.candidate_execution_ids.length
              ? 'plan-step complete'
              : isRoleRunning(plan, 'candidate')
                ? 'plan-step running'
                : 'plan-step'
          }
        >
          <span>02</span>
          <div>
            <strong>Candidate</strong>
            <small>
              {isRoleRunning(plan, 'candidate')
                ? 'Collecting the candidate report now'
                : plan.candidate_execution_ids.length
                  ? `${plan.candidate_execution_ids.length} candidate run${plan.candidate_execution_ids.length === 1 ? '' : 's'}`
                  : 'Rerun the locked scope after the change'}
            </small>
            {plan.candidate_execution_ids.map((id) => (
              <a key={id} href={hashForExecution(id)}>
                View candidate execution
              </a>
            ))}
            {isRoleRunning(plan, 'candidate') && plan.last_attempt_id && (
              <a href={hashForExecution(plan.last_attempt_id)}>
                View active execution
              </a>
            )}
          </div>
        </article>
      </div>
    </section>
  )
}

function PlanHeader({ local = true }: { local?: boolean }) {
  return (
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
      <nav className="topbar-actions" aria-label="Plan actions">
        <a
          className="button button-secondary"
          href={hashForWorkspace()}
          data-mobile-label="Overview"
        >
          Overview
        </a>
        <a
          className="button button-secondary"
          href={hashForPlans()}
          data-mobile-label="Plans"
        >
          Plans
        </a>
        {local && (
          <a
            className="button button-secondary"
            href={hashForWorkspace('tests')}
            data-mobile-label="Tests"
          >
            Test catalog
          </a>
        )}
        <ThemeToggle />
      </nav>
    </header>
  )
}

export function LocalPlanCreatePage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [catalog, setCatalog] = useState<Catalog | null>(null)
  const [label, setLabel] = useState('')
  const [purpose, setPurpose] = useState('')
  const [url, setUrl] = useState('')
  const [subject, setSubject] = useState('')
  const [judge, setJudge] = useState('')
  const [scenarios, setScenarios] = useState<string[]>([])
  const [testQuery, setTestQuery] = useState('')
  const [runs, setRuns] = useState('1')
  const [technicalRetries, setTechnicalRetries] = useState('1')
  const [seed, setSeed] = useState('')
  const [loading, setLoading] = useState(true)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void getDashboardDataBridge()
      .then(async (next) => {
        if (cancelled) return
        setBridge(next)
        if (next.mode !== 'local')
          throw new Error(
            'Local plans are available only in the local dashboard',
          )
        const loaded = catalogValue(await next.getCatalog())
        if (cancelled) return
        setCatalog(loaded)
        setUrl(loaded.url)
        setSubject(
          loaded.models[0]
            ? `${loaded.models[0].provider}\n${loaded.models[0].model}`
            : '',
        )
      })
      .catch((cause) => {
        if (!cancelled) setError(errorText(cause))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const selectedSubject = catalog?.models.find(
    (item) => `${item.provider}\n${item.model}` === subject,
  )
  const selectedJudge = catalog?.models.find(
    (item) => `${item.provider}\n${item.model}` === judge,
  )
  const groupedModels = useMemo(
    () => modelGroups(catalog?.models ?? []),
    [catalog],
  )
  const visibleScenarios = useMemo(() => {
    const query = testQuery.trim().toLocaleLowerCase()
    const all = catalog?.scenarios ?? []
    if (!query) return all
    return all.filter((scenario) =>
      `${scenario} ${scenarioName(scenario)}`
        .toLocaleLowerCase()
        .includes(query),
    )
  }, [catalog?.scenarios, testQuery])
  const runsPerTest = Math.max(1, Number(runs) || 1)
  const retryCount = Math.max(0, Number(technicalRetries) || 0)
  const plannedRuns = scenarios.length * runsPerTest
  const hasPlanLabel = label.trim().length > 0
  const canCreate =
    !loading &&
    !submitting &&
    Boolean(selectedSubject) &&
    scenarios.length > 0 &&
    hasPlanLabel

  const toggleScenario = (scenario: string, checked: boolean) => {
    setScenarios((current) => {
      if (checked)
        return current.includes(scenario) ? current : [...current, scenario]
      return current.filter((item) => item !== scenario)
    })
  }

  const selectVisibleScenarios = () => {
    setScenarios((current) => [
      ...current,
      ...visibleScenarios.filter((scenario) => !current.includes(scenario)),
    ])
  }

  const create = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (
      bridge?.mode !== 'local' ||
      !selectedSubject ||
      scenarios.length === 0 ||
      !hasPlanLabel
    ) {
      setError(
        'Add a plan label, choose an execution model and select at least one test.',
      )
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      const plan = await bridge.createPlan({
        label: label.trim(),
        purpose: purpose.trim(),
        url,
        model: selectedSubject.model,
        provider: selectedSubject.provider,
        judge_model: selectedJudge?.model ?? '',
        judge_provider: selectedJudge?.provider ?? '',
        scenarios,
        runs: runsPerTest,
        technical_retries: retryCount,
        seed: seed ? Number(seed) : null,
      })
      window.location.hash = hashForPlan(plan.id)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <>
      <a className="skip-link" href="#plan-create-main">
        Skip to plan creation
      </a>
      <PlanHeader />
      <main
        id="plan-create-main"
        className="page-shell overview-shell plan-create-shell"
      >
        <section className="page-heading" aria-labelledby="plan-create-title">
          <div>
            <div className="eyebrow">
              <span className="live-dot" aria-hidden="true" />
              Local evidence plan
            </div>
            <h1 id="plan-create-title">Create a focused local plan</h1>
            <p>
              Capture only the tests that matter for this change. The baseline
              locks this scope; every candidate will reuse it exactly.
            </p>
          </div>
          <div className="sync-block">
            <span>Evidence default</span>
            <strong>1 run / test</strong>
          </div>
        </section>
        {error && (
          <section className="empty-state" role="alert">
            <h2>Plan cannot be created</h2>
            <p>{error}</p>
          </section>
        )}
        <section className="panel plan-form-panel">
          <form className="plan-form" onSubmit={create}>
            <section
              className="plan-form-section"
              aria-labelledby="plan-intent-title"
            >
              <div className="plan-form-section-heading">
                <span>01</span>
                <div>
                  <h2 id="plan-intent-title">What are you validating?</h2>
                  <p>
                    A short label makes this local evidence easy to find again.
                  </p>
                </div>
              </div>
              <div className="plan-form-fields plan-form-intent-fields">
                <label>
                  <span>Plan label</span>
                  <input
                    value={label}
                    required
                    maxLength={120}
                    placeholder="Before harness change"
                    onChange={(event) => setLabel(event.target.value)}
                  />
                </label>
                <label>
                  <span>
                    Purpose <small>optional</small>
                  </span>
                  <textarea
                    value={purpose}
                    rows={2}
                    placeholder="Validate the loop-engineering change"
                    onChange={(event) => setPurpose(event.target.value)}
                  />
                </label>
              </div>
            </section>
            <section
              className="plan-form-section"
              aria-labelledby="plan-execution-title"
            >
              <div className="plan-form-section-heading">
                <span>02</span>
                <div>
                  <h2 id="plan-execution-title">Execution setup</h2>
                  <p>The subject and judge are recorded with the plan.</p>
                </div>
              </div>
              <div className="plan-form-fields plan-form-execution-fields">
                <label className="plan-url-field">
                  <span>iii WebSocket URL</span>
                  <input
                    required
                    value={url}
                    onChange={(event) => setUrl(event.target.value)}
                  />
                </label>
                <div>
                  <span>Execution model</span>
                  <ProviderModelDropdown
                    required
                    ariaLabel="Execution model"
                    value={subject}
                    disabled={loading}
                    onChange={setSubject}
                    groups={groupedModels.map((group) => ({
                      provider: group.provider,
                      models: group.models.map((item) => ({
                        label: item.model,
                        value: modelKey(item),
                      })),
                    }))}
                    placeholder="Choose a model"
                  />
                </div>
                <div>
                  <span>
                    Judge model <small>optional</small>
                  </span>
                  <ProviderModelDropdown
                    ariaLabel="Judge model"
                    value={judge}
                    disabled={loading}
                    onChange={setJudge}
                    groups={groupedModels.map((group) => ({
                      provider: group.provider,
                      models: group.models.map((item) => ({
                        label: item.model,
                        value: modelKey(item),
                      })),
                    }))}
                    placeholder="Use default judge"
                  />
                </div>
              </div>
            </section>
            <fieldset className="plan-scope-field">
              <legend>
                <span>03</span>
                Tests <small>choose the smallest useful scope</small>
              </legend>
              <div className="plan-scope-intro">
                <div>
                  <h2>Build the test scope</h2>
                  <p>
                    Start empty. Search for the behaviors touched by your
                    change, then select only those tests.
                  </p>
                </div>
                <output aria-live="polite">
                  <strong>{scenarios.length}</strong>
                  <span>
                    {scenarios.length === 1
                      ? 'test selected'
                      : 'tests selected'}
                    {' · '}
                    {plannedRuns} logical {plannedRuns === 1 ? 'run' : 'runs'}
                  </span>
                </output>
              </div>
              <div className="plan-scope-controls">
                <label className="plan-test-search">
                  <span>Find a test</span>
                  <input
                    type="search"
                    value={testQuery}
                    placeholder="Search by name or id"
                    onChange={(event) => setTestQuery(event.target.value)}
                  />
                </label>
                <div className="plan-scope-actions">
                  <button
                    type="button"
                    onClick={selectVisibleScenarios}
                    disabled={visibleScenarios.length === 0}
                  >
                    Select visible ({visibleScenarios.length})
                  </button>
                  <button
                    type="button"
                    onClick={() => setScenarios([])}
                    disabled={scenarios.length === 0}
                  >
                    Clear selection
                  </button>
                </div>
              </div>
              <div className="plan-test-options">
                {visibleScenarios.map((scenario) => {
                  const selected = scenarios.includes(scenario)
                  return (
                    <label
                      className={`plan-test-option${selected ? ' is-selected' : ''}`}
                      key={scenario}
                    >
                      <input
                        type="checkbox"
                        checked={selected}
                        onChange={(event) =>
                          toggleScenario(scenario, event.target.checked)
                        }
                      />
                      <span className="plan-test-option-copy">
                        <strong>{scenarioName(scenario)}</strong>
                        <code>{scenario}</code>
                      </span>
                    </label>
                  )
                })}
                {!visibleScenarios.length && (
                  <p className="plan-test-empty">
                    No tests match “{testQuery}”. Try another name or clear the
                    search.
                  </p>
                )}
              </div>
              <p className="plan-scope-note">
                The scope becomes immutable when the baseline starts. Technical
                retries do not add evidence samples.
              </p>
            </fieldset>
            <details className="plan-advanced">
              <summary>
                <span className="plan-advanced-summary-copy">
                  <span>Optional controls</span>
                  <strong>Sampling and retries</strong>
                  <small>
                    Keep the default unless you deliberately need more local
                    evidence.
                  </small>
                </span>
                <span className="plan-advanced-summary-meta">
                  {runsPerTest} {runsPerTest === 1 ? 'run' : 'runs'} / test ·{' '}
                  {retryCount} {retryCount === 1 ? 'retry' : 'retries'}
                </span>
              </summary>
              <div className="plan-advanced-body">
                <p className="plan-advanced-intro">
                  Runs create logical evidence samples. Retries only recover a
                  technical failure and never increase the evidence count.
                </p>
                <div className="plan-advanced-grid">
                  <label className="plan-advanced-control">
                    <span>
                      <strong>Runs per test</strong>
                      <small>default: 1</small>
                    </span>
                    <p>
                      Increase only when you want repeatable local evidence.
                    </p>
                    <input
                      type="number"
                      min="1"
                      max="20"
                      value={runs}
                      onChange={(event) => setRuns(event.target.value)}
                    />
                  </label>
                  <label className="plan-advanced-control">
                    <span>
                      <strong>Technical retries</strong>
                      <small>default: 1</small>
                    </span>
                    <p>Retries a technical failure without adding a sample.</p>
                    <input
                      type="number"
                      min="0"
                      max="3"
                      value={technicalRetries}
                      onChange={(event) =>
                        setTechnicalRetries(event.target.value)
                      }
                    />
                  </label>
                  <label className="plan-advanced-control">
                    <span>
                      <strong>Seed</strong>
                      <small>optional</small>
                    </span>
                    <p>Leave blank to resolve the canonical case seeds.</p>
                    <input
                      type="number"
                      min="0"
                      placeholder="Canonical"
                      value={seed}
                      onChange={(event) => setSeed(event.target.value)}
                    />
                  </label>
                </div>
              </div>
            </details>
            <div className="plan-form-actions">
              <p
                id="plan-create-requirements"
                className="plan-create-requirements"
              >
                {hasPlanLabel && scenarios.length > 0
                  ? 'Ready to create a draft. The scope stays editable until you start the baseline.'
                  : `Before creating: ${hasPlanLabel ? 'select at least one test' : 'add a plan label'}${hasPlanLabel || scenarios.length === 0 ? '' : ' and select at least one test'}.`}
              </p>
              <a className="button" href={hashForPlans()}>
                Cancel
              </a>
              <button
                className="button button-primary"
                type="submit"
                disabled={!canCreate}
                aria-describedby="plan-create-requirements"
              >
                {submitting ? 'Creating…' : 'Create draft plan'}
              </button>
            </div>
          </form>
        </section>
      </main>
      <footer>
        <span>Harness E2E · local plans</span>
        <a href={hashForWorkspace()}>Back to home</a>
      </footer>
    </>
  )
}

export function LocalPlanDetailPage({ planId }: { planId: string }) {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [plan, setPlan] = useState<LocalPlan | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [starting, setStarting] = useState<PlanRunRole | null>(null)
  const [runFeedback, setRunFeedback] = useState<PlanRunFeedback | null>(null)

  const load = useCallback(async () => {
    const next = bridge ?? (await getDashboardDataBridge())
    setBridge(next)
    if (next.mode !== 'local')
      throw new Error('Local plans are available only in the local dashboard')
    setPlan(await next.getPlan(planId))
  }, [bridge, planId])

  useEffect(() => {
    void load()
      .catch((cause) => setLoadError(errorText(cause)))
      .finally(() => setLoading(false))
  }, [load])
  useEffect(() => {
    if (
      !plan ||
      !['baseline_running', 'candidate_running'].includes(plan.state)
    )
      return
    const timer = window.setInterval(() => {
      void load().catch(() => undefined)
    }, 2_000)
    return () => window.clearInterval(timer)
  }, [load, plan])

  const start = async (role: PlanRunRole) => {
    if (!bridge || !plan) return
    setStarting(role)
    setRunFeedback({
      role,
      phase: 'starting',
      message: `Starting ${roleLabel(role).toLowerCase()}…`,
      executionId: null,
    })
    try {
      const nextPlan = await bridge.startPlan(plan.id, role)
      setPlan(nextPlan)
      setRunFeedback({
        role,
        phase: 'running',
        message: isRoleRunning(nextPlan, role)
          ? `${roleLabel(role)} is running. This page refreshes automatically while the report is collected.`
          : `${roleLabel(role)} started. Check the execution detail for the latest report.`,
        executionId: nextPlan.last_attempt_id,
      })
    } catch (cause) {
      setRunFeedback({
        role,
        phase: 'error',
        message: `Could not start ${roleLabel(role).toLowerCase()}: ${errorText(cause)}`,
        executionId: null,
      })
    } finally {
      setStarting(null)
    }
  }

  const readiness = useMemo(() => {
    if (!plan)
      return {
        label: 'Loading',
        tone: 'status-incomplete',
        detail: 'Reading the local plan.',
      }
    if (isRoleRunning(plan, 'baseline'))
      return {
        label: 'Baseline in progress',
        tone: 'status-incomplete',
        detail:
          'The baseline is running; candidate actions stay unavailable until its report is complete.',
      }
    if (isRoleRunning(plan, 'candidate'))
      return {
        label: 'Candidate in progress',
        tone: 'status-incomplete',
        detail: 'The locked scope is running against the captured baseline.',
      }
    if (!plan.baseline_execution_id)
      return {
        label:
          plan.incomplete_execution_ids.length > 0
            ? 'Baseline retry available'
            : 'Baseline required',
        tone: 'status-incomplete',
        detail:
          plan.incomplete_execution_ids.length > 0
            ? 'The last attempt did not produce a report; retry the same locked scope.'
            : 'The scope is defined but no completed baseline report exists.',
      }
    if (plan.candidate_execution_ids.length > 0)
      return {
        label: 'Comparison available',
        tone: 'status-pass',
        detail:
          'Candidate reports are ready to inspect against the locked baseline.',
      }
    if (plan.incomplete_execution_ids.length)
      return {
        label: 'Candidate retry available',
        tone: 'status-pass',
        detail:
          'An incomplete attempt is retained for diagnosis, but it does not block a retry or count as comparison evidence.',
      }
    return {
      label: 'Scope ready',
      tone: 'status-pass',
      detail:
        'Baseline and locked scope are structurally ready for a candidate.',
    }
  }, [plan])

  return (
    <>
      <a className="skip-link" href="#plan-detail-main">
        Skip to plan detail
      </a>
      <PlanHeader />
      <main id="plan-detail-main" className="page-shell overview-shell">
        {loading && (
          <section className="panel">
            <p className="table-empty">Loading plan…</p>
          </section>
        )}
        {loadError && !plan && (
          <section className="empty-state" role="alert">
            <h2>Plan unavailable</h2>
            <p>{loadError}</p>
            <a className="button" href={hashForNewPlan()}>
              Create another plan
            </a>
          </section>
        )}
        {plan && (
          <>
            <section
              className="page-heading"
              aria-labelledby="plan-detail-title"
            >
              <div>
                <div className="eyebrow">
                  <span className="live-dot" aria-hidden="true" />
                  Local plan · {stateLabel(plan)}
                </div>
                <h1 id="plan-detail-title">{plan.label || plan.id}</h1>
                <p>
                  {plan.purpose ||
                    'Explicit local baseline and candidate scope.'}
                </p>
              </div>
              <div className="sync-block">
                <span>Scope state</span>
                <strong>
                  {plan.locked ? 'Locked after baseline' : 'Editable draft'}
                </strong>
              </div>
            </section>
            <section className="plan-status-grid">
              <article className="panel plan-status-card">
                <div className="section-kicker">Comparison readiness</div>
                <strong className={`status-pill ${readiness.tone}`}>
                  {readiness.label}
                </strong>
                <p>{readiness.detail}</p>
                <small>
                  Statistical maturity is reported separately; it does not block
                  a local baseline.
                </small>
              </article>
              <article className="panel plan-status-card">
                <div className="section-kicker">Scope snapshot</div>
                <strong>
                  {plan.scenarios.length} tests · {plan.runs} run
                  {plan.runs === 1 ? '' : 's'} each
                </strong>
                <p>
                  {plan.model} · {plan.provider}
                </p>
                <small>
                  {plan.judge_model
                    ? `Judge: ${plan.judge_provider} · ${plan.judge_model}`
                    : 'Judge: default policy'}
                </small>
              </article>
            </section>
            <PlanLifecycle
              plan={plan}
              starting={starting}
              feedback={runFeedback}
              onStart={(role) => void start(role)}
            />
            {plan.incomplete_execution_ids.length > 0 && (
              <section className="panel">
                <div className="section-kicker">Incomplete attempts</div>
                <p className="trend-description">
                  These executions are retained for diagnosis but are excluded
                  from comparison and maturity.
                </p>
                {plan.incomplete_execution_ids.map((id) => (
                  <a
                    className="block text-link"
                    key={id}
                    href={hashForExecution(id)}
                  >
                    View incomplete execution
                  </a>
                ))}
              </section>
            )}
          </>
        )}
      </main>
      <footer>
        <span>Harness E2E · local plan detail</span>
        <a href={hashForPlans()}>Back to plans</a>
      </footer>
    </>
  )
}
