import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ExecutionSetup,
  ExecutionSetupFooter,
  focusFirstInvalid,
  requestPlanFromSelection,
  validateExecutionSetup,
} from '@/components/ExecutionSetup'
import { buttonClassName, Dialog } from '@/design-system'
import { hashForNewPlan } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  JsonObject,
} from '@/lib/dashboard-data-source'
import {
  type LocalScenarioSummary,
  localScenariosFromCatalog,
} from '@/lib/local-scenario-catalog'

type RunnerModel = { provider: string; model: string }
type RunnerCatalog = {
  url: string
  models: RunnerModel[]
  scenarios: string[]
  localScenarios: LocalScenarioSummary[]
}
type RunnerJob = {
  id?: string
  status?: string
  log?: string
  log_offset?: number
  log_truncated?: boolean
  error?: string | null
  defaults?: JsonObject
}

type RunnerForm = {
  label: string
  url: string
  subject: string
  judge: string
  scenarios: string[]
  runs: string
  technicalRetries: string
  seed: string
}

const initialForm: RunnerForm = {
  label: '',
  url: '',
  subject: '',
  judge: '',
  scenarios: [],
  runs: '1',
  technicalRetries: '1',
  seed: '',
}

function modelKey(model: RunnerModel) {
  return `${model.provider}\n${model.model}`
}

function modelGroups(models: RunnerModel[]) {
  const groups = new Map<string, RunnerModel[]>()
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

function asCatalog(value: JsonObject): RunnerCatalog {
  const models = Array.isArray(value.models)
    ? value.models.flatMap((candidate) => {
        if (!candidate || typeof candidate !== 'object') return []
        const model = candidate as JsonObject
        if (
          typeof model.model !== 'string' ||
          typeof model.provider !== 'string'
        ) {
          return []
        }
        return [{ model: model.model, provider: model.provider }]
      })
    : []
  const scenarios = Array.isArray(value.scenarios)
    ? value.scenarios.filter(
        (scenario): scenario is string => typeof scenario === 'string',
      )
    : []
  const localScenarios = localScenariosFromCatalog(value)
  return {
    url: typeof value.url === 'string' ? value.url : '',
    models,
    scenarios,
    localScenarios,
  }
}

function asJob(value: JsonObject): RunnerJob {
  const job = value.job && typeof value.job === 'object' ? value.job : value
  return job && typeof job === 'object' ? (job as RunnerJob) : {}
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

function statusLabel(status: string | undefined) {
  return (
    {
      running: 'Running…',
      cancelling: 'Cancelling…',
      cancelled: 'Cancelled',
      completed: 'Results saved',
      failed: 'Runner failed',
    }[status ?? ''] ?? 'Ready'
  )
}

export function LocalRunnerDialog({
  bridge,
  open,
  initialScenarios = [],
  onClose,
  onCompleted,
}: {
  bridge: DashboardDataBridge | null
  open: boolean
  /** Tests preselected by the page that opened the dialog (audit TH-06). */
  initialScenarios?: string[]
  onClose: () => void
  onCompleted?: () => void
}) {
  const [catalog, setCatalog] = useState<RunnerCatalog | null>(null)
  const [form, setForm] = useState<RunnerForm>(initialForm)
  const [scenarioQuery, setScenarioQuery] = useState('')
  const [job, setJob] = useState<RunnerJob | null>(null)
  // Audit RS-01: the snapshot also returns the previous job. Only a job
  // started from this dialog session may drive the status pill.
  const [ownJob, setOwnJob] = useState(false)
  const [log, setLog] = useState('')
  const [logOffset, setLogOffset] = useState(0)
  const [loadingCatalog, setLoadingCatalog] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [attempted, setAttempted] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refreshJob = useCallback(async () => {
    if (bridge?.mode !== 'local') return
    try {
      const response = asJob(await bridge.getRunSnapshot(logOffset))
      setJob(response)
      if (response.log_truncated) setLog('[Earlier runner output omitted]\n')
      if (response.log) setLog((current) => `${current}${response.log}`)
      if (typeof response.log_offset === 'number')
        setLogOffset(response.log_offset)
      if (response.status === 'completed') onCompleted?.()
    } catch (cause) {
      setError(errorMessage(cause))
    }
  }, [bridge, logOffset, onCompleted])

  const refreshCatalog = useCallback(async () => {
    if (bridge?.mode !== 'local') return
    setLoadingCatalog(true)
    setError(null)
    try {
      const next = asCatalog(await bridge.getCatalog(form.url || undefined))
      setCatalog(next)
      setForm((current) => ({
        ...current,
        url: current.url || next.url,
        subject:
          current.subject || (next.models[0] ? modelKey(next.models[0]) : ''),
        // Keep the local loop deliberate: selecting every scenario is too
        // expensive for the default path. The developer chooses the scope.
        scenarios: current.scenarios,
      }))
    } catch (cause) {
      setCatalog(null)
      setError(errorMessage(cause))
    } finally {
      setLoadingCatalog(false)
    }
  }, [bridge, form.url])

  useEffect(() => {
    if (!open || !bridge || bridge.mode !== 'local') return
    setError(null)
    setOwnJob(false)
    if (initialScenarios.length > 0)
      setForm((current) => ({
        ...current,
        scenarios: [
          ...current.scenarios,
          ...initialScenarios.filter((id) => !current.scenarios.includes(id)),
        ],
      }))
    void refreshCatalog()
    void refreshJob()
    let unsubscribe: (() => void) | undefined
    let interval: number | undefined
    bridge
      .subscribeRunChanges(() => void refreshJob())
      .then((off) => {
        unsubscribe = off
      })
      .catch(() => undefined)
    interval = window.setInterval(() => void refreshJob(), 2_000)
    return () => {
      unsubscribe?.()
      if (interval) window.clearInterval(interval)
    }
  }, [bridge, open, initialScenarios, refreshCatalog, refreshJob])

  const active =
    job?.status === 'running' || job?.status === 'cancelling' || submitting
  const selectedSubject = catalog?.models.find(
    (model) => modelKey(model) === form.subject,
  )
  const selectedJudge = catalog?.models.find(
    (model) => modelKey(model) === form.judge,
  )
  const groupedModels = useMemo(
    () => modelGroups(catalog?.models ?? []),
    [catalog],
  )
  const modelOptions = groupedModels.map((group) => ({
    provider: group.provider,
    models: group.models.map((model) => ({
      label: model.model,
      value: modelKey(model),
    })),
  }))
  const runsPerScenario = Math.max(1, Number(form.runs) || 1)
  const technicalRetries = Math.max(0, Number(form.technicalRetries) || 0)
  // The worker refuses a local Markdown scenario without a judge.
  const needsJudge = form.scenarios.some((id) =>
    (catalog?.localScenarios ?? []).some((scenario) => scenario.id === id),
  )
  const showJobStatus = Boolean(job?.status) && (ownJob || active)
  const testCount = form.scenarios.length
  const runLabel = submitting
    ? 'starting…'
    : active
      ? 'running…'
      : testCount > 0
        ? `run ${testCount} ${testCount === 1 ? 'test' : 'tests'}`
        : 'run tests'
  // Audit RS-10 / PN-05: the primary stays enabled; after a submit attempt
  // the footer lists what is still pending and the fields show it inline.
  const errors = attempted
    ? validateExecutionSetup({
        mode: 'quick',
        label: form.label,
        subject: form.subject,
        selectedScenarios: form.scenarios,
        url: form.url,
      })
    : {}
  const pending = Object.values(errors)

  const update = <K extends keyof RunnerForm>(key: K, value: RunnerForm[K]) => {
    setForm((current) => ({ ...current, [key]: value }))
  }

  // The worker refuses a local Markdown scenario without an explicit judge,
  // so selecting one fills the judge with the execution model. The rule used
  // to key on a "markdown_" prefix that the compiler no longer emits; the
  // catalog's own local list is the source of truth.
  const updateScenarios = (scenarios: string[]) => {
    setForm((current) => ({ ...current, scenarios }))
  }

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const nextErrors = validateExecutionSetup({
      mode: 'quick',
      label: form.label,
      subject: form.subject,
      selectedScenarios: form.scenarios,
      url: form.url,
    })
    if (Object.keys(nextErrors).length > 0 || !bridge || !selectedSubject) {
      setAttempted(true)
      focusFirstInvalid('quick-execution', nextErrors)
      if (!bridge) setError('The local runner is not connected.')
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      // A local scenario always travels with a judge; without an explicit
      // one it follows the execution model, resolved here so it can never
      // lag behind a model chosen after the scenario was ticked.
      const judge = selectedJudge ?? (needsJudge ? selectedSubject : null)
      const response = await bridge.startRun({
        // RunRequest.label is intentionally a string: empty labels remain
        // compatible with persisted execution metadata.
        label: form.label.trim(),
        url: form.url,
        model: selectedSubject.model,
        provider: selectedSubject.provider,
        judge_model: judge?.model || '',
        judge_provider: judge?.provider || '',
        scenarios: form.scenarios,
        runs: Number(form.runs),
        technical_retries: Number(form.technicalRetries),
        seed: form.seed ? Number(form.seed) : null,
      })
      setJob(asJob(response))
      setOwnJob(true)
      setLog('')
      setLogOffset(0)
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setSubmitting(false)
    }
  }

  const cancel = async () => {
    if (!bridge) return
    try {
      setJob(asJob(await bridge.cancelRun()))
    } catch (cause) {
      setError(errorMessage(cause))
    }
  }

  const summary = {
    mode: 'quick' as const,
    selectedScenarios: form.scenarios.length,
    runsPerScenario,
    technicalRetries,
    seed: form.seed,
    subject: selectedSubject
      ? `${selectedSubject.provider} / ${selectedSubject.model}`
      : '',
    judge: selectedJudge
      ? `${selectedJudge.provider} / ${selectedJudge.model}`
      : needsJudge && selectedSubject
        ? `${selectedSubject.provider} / ${selectedSubject.model} · same as model`
        : '',
    url: form.url,
  }
  // statusLabel already ends its running states with an ellipsis.
  const sentence = (label: string) =>
    /[.…]$/.test(label) ? label : `${label}.`
  const footerStatus =
    showJobStatus && job
      ? sentence(statusLabel(job.status))
      : job?.status
        ? `Previous runner job: ${statusLabel(job.status).toLowerCase()}.`
        : null
  const localCount = catalog?.localScenarios.length ?? 0

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      tall
      kicker="Execution setup"
      title="Run suite"
      description="Runs the selected tests once and saves an independent result. Use a plan when you need a fixed baseline and candidate comparison."
      closeLabel="Close execution form"
      className="ds-root"
      footer={
        <ExecutionSetupFooter
          summary={summary}
          pending={pending}
          error={error}
          status={footerStatus}
        >
          <a
            className={buttonClassName({
              variant: 'quiet',
              className: 'no-underline',
            })}
            href={hashForNewPlan()}
            onClick={() => requestPlanFromSelection(form.scenarios)}
          >
            create a reusable plan instead
          </a>
          {active ? (
            <button
              className={buttonClassName({ variant: 'secondary' })}
              type="button"
              onClick={() => void cancel()}
            >
              cancel execution
            </button>
          ) : (
            <button
              className={buttonClassName({ variant: 'secondary' })}
              type="button"
              onClick={onClose}
            >
              cancel
            </button>
          )}
          <button
            className={buttonClassName({ variant: 'primary' })}
            type="submit"
            form="local-runner-form"
            disabled={active}
            aria-busy={active}
          >
            {runLabel}
          </button>
        </ExecutionSetupFooter>
      }
    >
      <form
        id="local-runner-form"
        className="grid min-w-0 gap-6"
        onSubmit={submit}
        noValidate
      >
        <ExecutionSetup
          idPrefix="quick-execution"
          mode="quick"
          stickyOffset="dialog"
          label={form.label}
          url={form.url}
          subject={form.subject}
          judge={form.judge}
          modelGroups={modelOptions}
          availableScenarios={catalog?.scenarios ?? []}
          localScenarioIds={
            catalog?.localScenarios.map((scenario) => scenario.id) ?? []
          }
          scenarioTitles={Object.fromEntries(
            catalog?.localScenarios.map((scenario) => [
              scenario.id,
              scenario.title,
            ]) ?? [],
          )}
          selectedScenarios={form.scenarios}
          query={scenarioQuery}
          runs={form.runs}
          technicalRetries={form.technicalRetries}
          seed={form.seed}
          disabled={active}
          catalogLoading={loadingCatalog}
          catalogStatus={
            loadingCatalog
              ? { tone: 'loading', text: 'loading catalog…' }
              : catalog
                ? {
                    tone: 'ready',
                    text: `catalog ready · ${catalog.models.length} model${catalog.models.length === 1 ? '' : 's'} · ${catalog.scenarios.length} test${catalog.scenarios.length === 1 ? '' : 's'}${localCount > 0 ? ` · ${localCount} local` : ''}`,
                  }
                : { tone: 'unavailable', text: 'catalog unavailable' }
          }
          errors={errors}
          onRefreshCatalog={() => void refreshCatalog()}
          onLabelChange={(value) => update('label', value)}
          onUrlChange={(value) => update('url', value)}
          onSubjectChange={(value) => update('subject', value)}
          onJudgeChange={(value) => update('judge', value)}
          onSelectedScenariosChange={updateScenarios}
          onQueryChange={setScenarioQuery}
          onRunsChange={(value) => update('runs', value)}
          onTechnicalRetriesChange={(value) =>
            update('technicalRetries', value)
          }
          onSeedChange={(value) => update('seed', value)}
        />
        {log ? (
          <details
            className="rounded-[6px] bg-[var(--surface-fill)] p-4"
            open={active}
          >
            <summary className="cursor-pointer font-mono text-xs text-ink-soft">
              live runner output
            </summary>
            <pre
              className="mt-3 mb-0 max-h-80 w-full overflow-auto rounded-[6px] bg-canvas p-4 font-mono text-label leading-5 text-ink-soft"
              aria-live="polite"
            >
              {log}
            </pre>
          </details>
        ) : null}
      </form>
    </Dialog>
  )
}
