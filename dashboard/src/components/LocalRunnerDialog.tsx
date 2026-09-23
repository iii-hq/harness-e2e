import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ExecutionSetup,
  ExecutionSetupFooter,
  focusFirstInvalid,
  requestPlanFromSelection,
  validateExecutionSetup,
} from '@/components/ExecutionSetup'
import { buttonClassName, Dialog } from '@/design-system'
import { hashForExecution, hashForNewPlan } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  ExecutionParameters,
  JsonObject,
} from '@/lib/dashboard-data-source'

type RunnerModel = { provider: string; model: string }
type RunnerCatalog = {
  url: string
  models: RunnerModel[]
  scenarios: string[]
}

export type RunnerForm = {
  label: string
  url: string
  subject: string
  scenarios: string[]
  runs: string
  technicalRetries: string
  seed: string
  agent: string
}

const initialForm: RunnerForm = {
  label: '',
  url: '',
  subject: '',
  scenarios: [],
  runs: '1',
  technicalRetries: '1',
  seed: '',
  agent: '',
}

const NO_SCENARIOS: string[] = []

function modelKey(model: RunnerModel) {
  return `${model.provider}\n${model.model}`
}

/** The form a run starts from: an execution's parameters when it runs again,
 *  with only `scenarios` marked when some are given (rerun a subset). */
export function runnerForm(
  parameters: ExecutionParameters | null,
  scenarios: string[] = [],
): RunnerForm {
  if (!parameters) return { ...initialForm, scenarios }
  return {
    ...initialForm,
    subject: modelKey(parameters),
    scenarios: scenarios.length > 0 ? scenarios : parameters.scenarios,
    runs: String(parameters.runs),
    technicalRetries: String(parameters.technical_retries),
    // Copied as is; cleared means the canonical case set.
    seed: parameters.seed === null ? '' : String(parameters.seed),
    agent: parameters.agent ?? '',
  }
}

/** What `execution-start` receives for the form. */
export function executionStartRequest(form: RunnerForm): {
  parameters: ExecutionParameters
  label: string
} {
  const [provider = '', model = ''] = form.subject.split('\n')
  return {
    label: form.label.trim(),
    parameters: {
      scenarios: form.scenarios,
      runs: Number(form.runs) || 1,
      technical_retries: Number(form.technicalRetries) || 0,
      seed: form.seed.trim() ? Number(form.seed) : null,
      model,
      provider,
      agent: form.agent.trim() || null,
    },
  }
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
  return {
    url: typeof value.url === 'string' ? value.url : '',
    models,
    scenarios,
  }
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** Run tests and Run again: one form that starts an execution on this stack
 *  and then follows it on its page. */
export function LocalRunnerDialog({
  bridge,
  open,
  initialScenarios = NO_SCENARIOS,
  parameters = null,
  onClose,
}: {
  bridge: DashboardDataBridge | null
  open: boolean
  /** Tests preselected by the page that opened the dialog (audit TH-06). */
  initialScenarios?: string[]
  /** Parameters of the execution to run again; the form starts from them. */
  parameters?: ExecutionParameters | null
  onClose: () => void
}) {
  const [catalog, setCatalog] = useState<RunnerCatalog | null>(null)
  const [form, setForm] = useState<RunnerForm>(initialForm)
  const [scenarioQuery, setScenarioQuery] = useState('')
  const [loadingCatalog, setLoadingCatalog] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [attempted, setAttempted] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refreshCatalog = useCallback(async () => {
    if (!bridge) return
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
    if (!open) return
    if (parameters) setForm(runnerForm(parameters, initialScenarios))
    else if (initialScenarios.length > 0)
      setForm((current) => ({
        ...current,
        scenarios: [
          ...current.scenarios,
          ...initialScenarios.filter((id) => !current.scenarios.includes(id)),
        ],
      }))
  }, [open, parameters, initialScenarios])

  useEffect(() => {
    if (!open || !bridge) return
    setError(null)
    void refreshCatalog()
  }, [bridge, open, refreshCatalog])

  // The execution run again may name a model or scenarios this stack's
  // catalog lacks: they stay listed and selected, and fail in their slots.
  const models = useMemo(() => {
    const listed = catalog?.models ?? []
    return parameters &&
      !listed.some((model) => modelKey(model) === modelKey(parameters))
      ? [...listed, { provider: parameters.provider, model: parameters.model }]
      : listed
  }, [catalog, parameters])
  const scenarios = useMemo(() => {
    const listed = catalog?.scenarios ?? []
    return [
      ...listed,
      ...(parameters?.scenarios ?? []).filter((id) => !listed.includes(id)),
    ]
  }, [catalog, parameters])
  const modelOptions = modelGroups(models).map((group) => ({
    provider: group.provider,
    models: group.models.map((model) => ({
      label: model.model,
      value: modelKey(model),
    })),
  }))
  const runsPerScenario = Math.max(1, Number(form.runs) || 1)
  const technicalRetries = Math.max(0, Number(form.technicalRetries) || 0)
  const testCount = form.scenarios.length
  const runLabel = submitting
    ? 'starting…'
    : testCount > 0
      ? `run ${testCount} ${testCount === 1 ? 'test' : 'tests'}`
      : 'run tests'
  // Audit RS-10 / PN-05: the primary stays enabled; after a submit attempt
  // the footer lists what is still pending and the fields show it inline.
  const validation = () =>
    validateExecutionSetup({
      mode: 'quick',
      label: form.label,
      subject: form.subject,
      selectedScenarios: form.scenarios,
      url: form.url,
    })
  const errors = attempted ? validation() : {}

  const update = <K extends keyof RunnerForm>(key: K, value: RunnerForm[K]) => {
    setForm((current) => ({ ...current, [key]: value }))
  }

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const nextErrors = validation()
    if (Object.keys(nextErrors).length > 0 || !bridge) {
      setAttempted(true)
      focusFirstInvalid('quick-execution', nextErrors)
      if (!bridge) setError('The local runner is not connected.')
      return
    }
    setSubmitting(true)
    setError(null)
    try {
      const started = await bridge.startExecution(executionStartRequest(form))
      onClose()
      window.location.hash = hashForExecution(started.execution_id)
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setSubmitting(false)
    }
  }

  const request = executionStartRequest(form)
  const summary = {
    mode: 'quick' as const,
    selectedScenarios: form.scenarios.length,
    runsPerScenario,
    technicalRetries,
    seed: form.seed,
    subject: form.subject
      ? `${request.parameters.provider} / ${request.parameters.model}`
      : '',
    url: form.url,
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      tall
      kicker="Execution setup"
      title={parameters ? 'Run again' : 'Run suite'}
      description={
        parameters
          ? 'Starts a new execution on this stack with the parameters of this one. Change anything before running.'
          : 'Runs the selected tests on this stack as a new execution. Use a plan when you need a fixed baseline and candidate comparison.'
      }
      closeLabel="Close execution form"
      className="ds-root"
      footer={
        <ExecutionSetupFooter
          summary={summary}
          pending={Object.values(errors)}
          error={error}
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
          <button
            className={buttonClassName({ variant: 'secondary' })}
            type="button"
            onClick={onClose}
          >
            cancel
          </button>
          <button
            className={buttonClassName({ variant: 'primary' })}
            type="submit"
            form="local-runner-form"
            disabled={submitting}
            aria-busy={submitting}
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
          modelGroups={modelOptions}
          availableScenarios={scenarios}
          selectedScenarios={form.scenarios}
          query={scenarioQuery}
          runs={form.runs}
          technicalRetries={form.technicalRetries}
          seed={form.seed}
          agent={form.agent}
          disabled={submitting}
          catalogLoading={loadingCatalog}
          catalogStatus={
            loadingCatalog
              ? { tone: 'loading', text: 'loading catalog…' }
              : catalog
                ? {
                    tone: 'ready',
                    text: `catalog ready · ${catalog.models.length} model${catalog.models.length === 1 ? '' : 's'} · ${catalog.scenarios.length} test${catalog.scenarios.length === 1 ? '' : 's'}`,
                  }
                : { tone: 'unavailable', text: 'catalog unavailable' }
          }
          errors={errors}
          onRefreshCatalog={() => void refreshCatalog()}
          onLabelChange={(value) => update('label', value)}
          onUrlChange={(value) => update('url', value)}
          onSubjectChange={(value) => update('subject', value)}
          onSelectedScenariosChange={(value) => update('scenarios', value)}
          onQueryChange={setScenarioQuery}
          onRunsChange={(value) => update('runs', value)}
          onTechnicalRetriesChange={(value) =>
            update('technicalRetries', value)
          }
          onSeedChange={(value) => update('seed', value)}
          onAgentChange={(value) => update('agent', value)}
        />
      </form>
    </Dialog>
  )
}
