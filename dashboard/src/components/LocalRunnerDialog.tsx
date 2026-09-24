import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ExecutionSetup,
  ExecutionSetupFooter,
  focusFirstInvalid,
  validateExecutionSetup,
} from '@/components/ExecutionSetup'
import { buttonClassName, Dialog, Field, Select } from '@/design-system'
import { hashForExecution } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  DashboardExecutionSummary,
  ExecutionParameters,
  JsonObject,
  Suite,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  executionTitle,
  providerModel,
} from '@/lib/execution-view'

type RunnerModel = { provider: string; model: string }
export type RunnerCatalog = {
  models: RunnerModel[]
  scenarios: string[]
  /** Scenarios that run only together, in order. */
  groups: string[][]
}

export type RunnerForm = {
  label: string
  subject: string
  /** The suite picked, by id; empty for scenarios ticked by hand. */
  suite: string
  scenarios: string[]
  runs: string
  technicalRetries: string
  agent: string
}

const initialForm: RunnerForm = {
  label: '',
  subject: '',
  suite: '',
  scenarios: [],
  runs: '1',
  technicalRetries: '1',
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
  label = '',
): RunnerForm {
  if (!parameters) return { ...initialForm, scenarios }
  return {
    ...initialForm,
    // The name it runs again under, to edit.
    label,
    subject: modelKey(parameters),
    // A subset of its scenarios is ticked by hand.
    suite: scenarios.length > 0 ? '' : (parameters.suite?.id ?? ''),
    scenarios: scenarios.length > 0 ? scenarios : parameters.scenarios,
    runs: String(parameters.runs),
    technicalRetries: String(parameters.technical_retries),
    agent: parameters.agent ?? '',
  }
}

/** Run tests starts from the model of the newest execution (newest first)
 *  whose model this stack still lists; without one, no model is chosen. */
export function lastUsedModel(
  executions: DashboardExecutionSummary[],
  models: RunnerModel[],
): RunnerModel | null {
  for (const { parameters } of executions) {
    const listed =
      parameters &&
      models.find((model) => modelKey(model) === modelKey(parameters))
    if (listed) return listed
  }
  return null
}

/** The execution a busy runner names: its id, in parentheses, before "is
 *  still running". */
export function runningExecutionId(message: string): string | null {
  return /\(([\w-]+)\) is still running/.exec(message)?.[1] ?? null
}

/** A sequential group runs whole: ticking one of its tests ticks the group,
 *  unticking one unticks it. */
export function withSequentialGroups(
  next: string[],
  previous: string[],
  groups: string[][],
): string[] {
  let result = next
  for (const group of groups) {
    if (group.some((id) => previous.includes(id) && !next.includes(id)))
      result = result.filter((id) => !group.includes(id))
    else if (group.some((id) => result.includes(id)))
      result = [...result, ...group.filter((id) => !result.includes(id))]
  }
  return result
}

/** What a suite holds, to fill the form and to tell whether it still does. */
export type SuiteContent = Pick<
  Suite,
  'id' | 'label' | 'scenarios' | 'repetitions' | 'technical_retries'
> & {
  /** An execution's suite this runner does not list, as it ran. */
  recorded?: boolean
}

/** The suites the form offers: this runner's, and the one an execution ran
 *  when this runner does not list it (with what it ran). */
export function suiteChoices(
  suites: Suite[],
  parameters: ExecutionParameters | null,
): SuiteContent[] {
  const recorded = parameters?.suite?.id
  return recorded && !suites.some((suite) => suite.id === recorded)
    ? [
        ...suites,
        {
          id: recorded,
          label: parameters.suite?.label || recorded,
          scenarios: parameters.scenarios,
          repetitions: parameters.runs,
          technical_retries: parameters.technical_retries,
          recorded: true,
        },
      ]
    : suites
}

/** The picked suite while the form still holds exactly what it does; any
 *  change to its tests, runs or retries makes the suite unnamed. */
export function namedSuite(
  form: RunnerForm,
  choices: SuiteContent[],
): SuiteContent | null {
  const suite = choices.find((choice) => choice.id === form.suite)
  if (!suite) return null
  const same =
    suite.scenarios.length === form.scenarios.length &&
    suite.scenarios.every((id) => form.scenarios.includes(id)) &&
    suite.repetitions === Number(form.runs) &&
    suite.technical_retries === Number(form.technicalRetries)
  return same ? suite : null
}

/** What `execution-start` receives for the form. */
export function executionStartRequest(
  form: RunnerForm,
  suite: SuiteContent | null = null,
): {
  parameters: ExecutionParameters
  label: string
} {
  const [provider = '', model = ''] = form.subject.split('\n')
  return {
    label: form.label.trim(),
    parameters: {
      suite: suite ? { id: suite.id, label: suite.label } : null,
      scenarios: form.scenarios,
      runs: Number(form.runs) || 1,
      technical_retries: Number(form.technicalRetries) || 0,
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

export function asCatalog(value: JsonObject): RunnerCatalog {
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
  const strings = (list: unknown) =>
    Array.isArray(list)
      ? list.filter((item): item is string => typeof item === 'string')
      : []
  const groups = Array.isArray(value.scenario_groups)
    ? value.scenario_groups.map(strings).filter((group) => group.length > 1)
    : []
  return { models, scenarios: strings(value.scenarios), groups }
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** Why a run did not start: a busy runner names the execution that holds it
 *  by its title, to open; any other error reads as it came. */
export async function describeStartError(
  bridge: DashboardDataBridge,
  cause: unknown,
): Promise<{ error: string; running: { id: string; title: string } | null }> {
  const message = errorMessage(cause)
  const id = runningExecutionId(message)
  const detail = id ? await bridge.getExecution(id).catch(() => null) : null
  if (!id || !detail) return { error: message, running: null }
  const { title } = executionTitle(buildExecutionPresentation(detail))
  return {
    error: `"${title}" is still running. Wait for it to finish or cancel it.`,
    running: { id, title },
  }
}

/** Run tests and Run again: one form that starts an execution on this stack
 *  and then follows it on its page. */
export function LocalRunnerDialog({
  bridge,
  open,
  initialScenarios = NO_SCENARIOS,
  parameters = null,
  label = '',
  onClose,
}: {
  bridge: DashboardDataBridge | null
  open: boolean
  /** Tests preselected by the page that opened the dialog (audit TH-06). */
  initialScenarios?: string[]
  /** Parameters of the execution to run again; the form starts from them. */
  parameters?: ExecutionParameters | null
  /** Name of the execution run again; the new one starts with it. */
  label?: string
  onClose: () => void
}) {
  const [catalog, setCatalog] = useState<RunnerCatalog | null>(null)
  const [suites, setSuites] = useState<Suite[]>([])
  const [form, setForm] = useState<RunnerForm>(initialForm)
  const [scenarioQuery, setScenarioQuery] = useState('')
  const [loadingCatalog, setLoadingCatalog] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [attempted, setAttempted] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // The execution holding a busy runner, to open.
  const [running, setRunning] = useState<{ id: string; title: string } | null>(
    null,
  )
  // The model taken from the last execution, said so under the field.
  const [lastSubject, setLastSubject] = useState('')
  // A new setup sheet per opening, so its filters start fresh.
  const [opening, setOpening] = useState(0)

  const refreshCatalog = useCallback(async () => {
    if (!bridge) return
    setLoadingCatalog(true)
    setError(null)
    try {
      const [next, recent, listed] = await Promise.all([
        bridge.getCatalog().then(asCatalog),
        // Running again brings its own model.
        parameters
          ? []
          : bridge
              .listExecutions({ limit: 20 })
              .then((manifest) => manifest.executions)
              .catch(() => []),
        bridge
          .listSuites()
          .then((response) => response.suites)
          .catch(() => []),
      ])
      const last = lastUsedModel(recent, next.models)
      setLastSubject(last ? modelKey(last) : '')
      setCatalog(next)
      setSuites(listed)
      setForm((current) => ({
        ...current,
        // Never a model the user did not pick or run last: without one the
        // field asks for it.
        subject: current.subject || (last ? modelKey(last) : ''),
        // Keep the local loop deliberate: selecting every scenario is too
        // expensive for the default path. The developer chooses the scope.
        scenarios: withSequentialGroups(current.scenarios, [], next.groups),
      }))
    } catch (cause) {
      setCatalog(null)
      setError(errorMessage(cause))
    } finally {
      setLoadingCatalog(false)
    }
  }, [bridge, parameters])

  useEffect(() => {
    if (!open) return
    setOpening((count) => count + 1)
    setRunning(null)
    if (parameters) setForm(runnerForm(parameters, initialScenarios, label))
    else if (initialScenarios.length > 0)
      setForm((current) => ({
        ...current,
        scenarios: [
          ...current.scenarios,
          ...initialScenarios.filter((id) => !current.scenarios.includes(id)),
        ],
      }))
  }, [open, parameters, initialScenarios, label])

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
      ...[...(parameters?.scenarios ?? []), ...form.scenarios].filter(
        (id, index, all) => !listed.includes(id) && all.indexOf(id) === index,
      ),
    ]
  }, [catalog, parameters, form.scenarios])
  const choices = useMemo(
    () => suiteChoices(suites, parameters),
    [suites, parameters],
  )
  const picked = choices.find((choice) => choice.id === form.suite) ?? null
  const suite = namedSuite(form, choices)
  const pickSuite = (id: string) => {
    const chosen = choices.find((choice) => choice.id === id)
    setForm((current) =>
      chosen
        ? {
            ...current,
            suite: id,
            scenarios: chosen.scenarios,
            runs: String(chosen.repetitions),
            technicalRetries: String(chosen.technical_retries),
          }
        : { ...current, suite: '' },
    )
  }
  const suiteHint = suite
    ? `${suite.scenarios.length} ${suite.scenarios.length === 1 ? 'test' : 'tests'} · ${suite.repetitions} ${suite.repetitions === 1 ? 'run' : 'runs'} each · ${suite.technical_retries} ${suite.technical_retries === 1 ? 'retry' : 'retries'}`
    : picked
      ? `Changed from ${picked.label}: runs as an unnamed suite.`
      : 'Pick a suite, or tick the tests below.'
  const suiteGroups: Array<[string, SuiteContent[]]> = [
    ['Repository', suites.filter((choice) => choice.source === 'repository')],
    ['This Console', suites.filter((choice) => choice.source === 'local')],
    ['This execution', choices.filter((choice) => choice.recorded)],
  ]
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
  // Without the catalog the form still sends what it holds.
  const validation = () =>
    validateExecutionSetup({
      mode: 'quick',
      label: form.label,
      subject: form.subject,
      selectedScenarios: form.scenarios,
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
    setRunning(null)
    try {
      const started = await bridge.startExecution(
        executionStartRequest(form, suite),
      )
      onClose()
      window.location.hash = hashForExecution(started.execution_id)
    } catch (cause) {
      const described = await describeStartError(bridge, cause)
      setRunning(described.running)
      setError(described.error)
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
    subject: form.subject ? providerModel(request.parameters) : '',
  }
  // Said before running: a sequential group always runs whole.
  const groupNote = (catalog?.groups ?? [])
    .filter((group) => group.some((id) => form.scenarios.includes(id)))
    .map((group) => `${group.join(' then ')} run only together, in this order.`)
    .join(' ')

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      tall
      kicker="Execution setup"
      title={parameters ? 'Run again' : 'Run tests'}
      description={
        parameters
          ? 'Starts a new execution on this stack with the suite and parameters of this one. Change anything before running.'
          : 'Runs a suite, or the tests ticked, on this stack as a new execution. To compare, tick two executions in the list.'
      }
      closeLabel="Close execution form"
      className="ds-root"
      footer={
        <ExecutionSetupFooter
          summary={summary}
          pending={Object.values(errors)}
          error={error}
          status={groupNote || null}
        >
          {running ? (
            <a
              className={buttonClassName({
                variant: 'secondary',
                className: 'no-underline',
              })}
              href={hashForExecution(running.id)}
              onClick={onClose}
            >
              open {running.title}
            </a>
          ) : null}
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
        {/* The first field: what to test. Ticking tests by hand after
            picking a suite makes it unnamed. */}
        <div className="grid items-start gap-4 sm:grid-cols-2">
          <Field label="Suite" htmlFor="quick-execution-suite" hint={suiteHint}>
            <Select
              id="quick-execution-suite"
              value={form.suite}
              disabled={submitting}
              onChange={(event) => pickSuite(event.target.value)}
            >
              <option value="">Unnamed · the tests ticked below</option>
              {suiteGroups.map(([group, entries]) =>
                entries.length > 0 ? (
                  <optgroup key={group} label={group}>
                    {entries.map((entry) => (
                      <option key={entry.id} value={entry.id}>
                        {entry.recorded
                          ? `${entry.label} · as recorded`
                          : entry.label}
                      </option>
                    ))}
                  </optgroup>
                ) : null,
              )}
            </Select>
          </Field>
        </div>
        <ExecutionSetup
          key={opening}
          // Running again shows what will run first.
          initialOnlySelected={parameters !== null}
          idPrefix="quick-execution"
          mode="quick"
          stickyOffset="dialog"
          label={form.label}
          subject={form.subject}
          subjectHint={
            form.subject && form.subject === lastSubject && !parameters
              ? 'The model of your last execution.'
              : undefined
          }
          modelGroups={modelOptions}
          availableScenarios={scenarios}
          selectedScenarios={form.scenarios}
          query={scenarioQuery}
          runs={form.runs}
          technicalRetries={form.technicalRetries}
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
          onSubjectChange={(value) => update('subject', value)}
          onSelectedScenariosChange={(value) =>
            update(
              'scenarios',
              withSequentialGroups(
                value,
                form.scenarios,
                catalog?.groups ?? [],
              ),
            )
          }
          onQueryChange={setScenarioQuery}
          onRunsChange={(value) => update('runs', value)}
          onTechnicalRetriesChange={(value) =>
            update('technicalRetries', value)
          }
          onAgentChange={(value) => update('agent', value)}
        />
      </form>
    </Dialog>
  )
}
