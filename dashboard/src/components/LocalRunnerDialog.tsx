import { ModelPicker } from '@iii-dev/console-ui'
import {
  AlertCircle,
  ArrowRight,
  Info,
  Minus,
  Plus,
  TriangleAlert,
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { GithubCard } from '@/components/run-dialog/GithubCard'
import { Picker, type PickerGroup } from '@/components/run-dialog/Picker'
import {
  pendingReasons,
  pendingText,
  plural,
  runLabel,
  stackDeclares,
  summaryCounts,
  summaryDetail,
  whereHint,
} from '@/components/run-dialog/run-dialog-model'
import {
  type CatalogStatus,
  TestsColumn,
} from '@/components/run-dialog/TestsColumn'
import '@/components/run-dialog/run-dialog.css'
import { Dialog } from '@/design-system'
import { hashForExecution, hashForStacks } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  DashboardExecutionSummary,
  ExecutionParameters,
  GithubStatus,
  JsonObject,
  Stack,
  Suite,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  executionTitle,
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
  /** On this harness, or in Docker or on GitHub on a stack. */
  where: 'harness' | 'docker' | 'github'
  /** The stack picked for Docker or GitHub, by id, or the execution's as
   *  recorded. */
  stack: string
}

const initialForm: RunnerForm = {
  label: '',
  subject: '',
  suite: '',
  scenarios: [],
  runs: '1',
  technicalRetries: '1',
  agent: '',
  where: 'harness',
  stack: '',
}

/** The stack select's value for the stack an execution ran on, as recorded. */
const RECORDED_STACK = 'recorded'

/** A stack Docker can run on: one of this Console's list, or the one an
 *  execution recorded. Its YAML is what the executor receives. */
export type StackChoice = {
  value: string
  label: string
  source: 'repository' | 'local' | 'recorded'
  /** What the execution records it as. */
  name: string
  yaml: string
  sha256?: string
}

/** The stacks the form offers for Docker: the listed ones, and the one the
 *  execution ran on as it recorded it, unless a listed stack holds that very
 *  YAML. */
export function stackChoices(
  stacks: Stack[],
  parameters: ExecutionParameters | null,
): StackChoice[] {
  const listed: StackChoice[] = stacks.map((stack) => ({
    value: stack.id,
    label: stack.label,
    source: stack.source,
    name: stack.source === 'repository' ? stack.id : stack.label,
    yaml: stack.yaml,
  }))
  const recorded = parameters?.stack
  if (!recorded || listed.some((choice) => choice.yaml === recorded.yaml))
    return listed
  return [
    ...listed,
    {
      value: RECORDED_STACK,
      label: recorded.name,
      source: 'recorded',
      name: recorded.name,
      yaml: recorded.yaml,
      sha256: recorded.sha256,
    },
  ]
}

/** The stack a select value names; the recorded one is the listed stack of
 *  the same YAML when there is one. */
export function pickedStack(
  value: string,
  choices: StackChoice[],
  parameters: ExecutionParameters | null,
): StackChoice | null {
  return (
    choices.find((choice) => choice.value === value) ??
    (value === RECORDED_STACK && parameters?.stack
      ? choices.find((choice) => choice.yaml === parameters.stack?.yaml)
      : undefined) ??
    null
  )
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
    // Its suite as it ran; a subset of its scenarios is ticked by hand.
    suite:
      scenarios.length === 0 && parameters.suite?.id
        ? `${RECORDED}${parameters.suite.id}`
        : '',
    scenarios: scenarios.length > 0 ? scenarios : parameters.scenarios,
    runs: String(parameters.runs),
    technicalRetries: String(parameters.technical_retries),
    agent: parameters.agent ?? '',
    // Where it ran, on the stack it recorded: a GitHub run runs on GitHub
    // again when its stack is known.
    where:
      parameters.where === 'github' && parameters.stack
        ? 'github'
        : parameters.where === 'docker'
          ? 'docker'
          : 'harness',
    stack: parameters.stack ? RECORDED_STACK : '',
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
  /** An execution's suite as it ran, when this runner lists it otherwise
   *  or not at all. */
  recorded?: boolean
}

/** The select value of the suite an execution ran, apart from the suite of
 *  that id this runner lists. */
const RECORDED = 'recorded:'

export function choiceValue(choice: SuiteContent) {
  return choice.recorded ? `${RECORDED}${choice.id}` : choice.id
}

function holds(
  suite: SuiteContent,
  scenarios: string[],
  runs: number,
  technicalRetries: number,
) {
  return (
    suite.scenarios.length === scenarios.length &&
    suite.scenarios.every((id) => scenarios.includes(id)) &&
    suite.repetitions === runs &&
    suite.technical_retries === technicalRetries
  )
}

/** The suites the form offers: this runner's, and the one an execution ran
 *  as it ran, unless this runner lists that suite holding the same. */
export function suiteChoices(
  suites: Suite[],
  parameters: ExecutionParameters | null,
): SuiteContent[] {
  const id = parameters?.suite?.id
  if (!parameters || !id) return suites
  const listed = suites.find((suite) => suite.id === id)
  return listed &&
    holds(
      listed,
      parameters.scenarios,
      parameters.runs,
      parameters.technical_retries,
    )
    ? suites
    : [
        ...suites,
        {
          id,
          label: parameters.suite?.label || id,
          scenarios: parameters.scenarios,
          repetitions: parameters.runs,
          technical_retries: parameters.technical_retries,
          recorded: true,
        },
      ]
}

/** The suite a select value names. The suite an execution ran is the listed
 *  one of its id when that holds the same. */
export function pickedSuite(
  value: string,
  choices: SuiteContent[],
): SuiteContent | null {
  return (
    choices.find((choice) => choiceValue(choice) === value) ??
    (value.startsWith(RECORDED)
      ? choices.find((choice) => choice.id === value.slice(RECORDED.length))
      : undefined) ??
    null
  )
}

/** The picked suite while the form still holds exactly what it does; any
 *  change to its tests, runs or retries makes the suite unnamed. */
export function namedSuite(
  form: RunnerForm,
  choices: SuiteContent[],
): SuiteContent | null {
  const suite = pickedSuite(form.suite, choices)
  return suite &&
    holds(
      suite,
      form.scenarios,
      Number(form.runs),
      Number(form.technicalRetries),
    )
    ? suite
    : null
}

/** What `execution-start` receives for the form. Docker and GitHub get the
 *  stack's YAML: the executor knows nothing of this Console's stacks. */
export function executionStartRequest(
  form: RunnerForm,
  suite: SuiteContent | null = null,
  stack: StackChoice | null = null,
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
      where: form.where,
      ...(form.where !== 'harness' && stack
        ? { stack: { name: stack.name, yaml: stack.yaml } }
        : {}),
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

const SUITE_SOURCE: Record<string, string> = {
  repository: 'Repository suite',
  local: 'Saved in this Console',
  recorded: 'As this execution ran',
}

function shortSha(sha?: string) {
  return sha ? sha.replace('sha256:', '').slice(0, 12) : ''
}

/** Run tests and Run again: one dialog that starts an execution on this
 *  harness, in Docker or on GitHub, then follows it on its page. */
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
  const [dockerGroups, setDockerGroups] = useState(2)
  const [suites, setSuites] = useState<Suite[]>([])
  const [stacks, setStacks] = useState<Stack[]>([])
  const [form, setForm] = useState<RunnerForm>(initialForm)
  const [query, setQuery] = useState('')
  const [onlySelected, setOnlySelected] = useState(false)
  const [loadingCatalog, setLoadingCatalog] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [github, setGithub] = useState<GithubStatus | 'loading' | null>(null)
  // The execution holding a busy runner, to open.
  const [running, setRunning] = useState<{ id: string; title: string } | null>(
    null,
  )
  // The model taken from the last execution, said so under the field.
  const [lastSubject, setLastSubject] = useState('')
  // One Run dialog is open at a time: stable ids let the browser journeys
  // (scripts/*.browser.mjs) address its controls.
  const id = (name: string) => `run-dialog-${name}`

  const refreshCatalog = useCallback(async () => {
    if (!bridge) return
    setLoadingCatalog(true)
    setError(null)
    try {
      const [raw, recent, listed, stackList] = await Promise.all([
        bridge.getCatalog(),
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
        bridge
          .listStacks()
          .then((response) => response.stacks)
          .catch(() => []),
      ])
      const next = asCatalog(raw)
      const last = lastUsedModel(recent, next.models)
      setLastSubject(last ? modelKey(last) : '')
      setCatalog(next)
      if (typeof raw.docker_parallel_groups === 'number')
        setDockerGroups(raw.docker_parallel_groups)
      setSuites(listed)
      setStacks(stackList)
      setForm((current) => ({
        ...current,
        // Never a model the user did not pick or run last.
        subject: current.subject || (last ? modelKey(last) : ''),
        scenarios: withSequentialGroups(current.scenarios, [], next.groups),
      }))
    } catch {
      setCatalog(null)
    } finally {
      setLoadingCatalog(false)
    }
  }, [bridge, parameters])

  const refreshGithub = useCallback(async () => {
    if (!bridge) return
    setGithub('loading')
    try {
      setGithub(await bridge.getGithubStatus())
    } catch (cause) {
      setGithub({
        ready: false,
        repository: '',
        account: null,
        message: errorMessage(cause),
      })
    }
  }, [bridge])

  useEffect(() => {
    if (!open) return
    setRunning(null)
    setError(null)
    setQuery('')
    setGithub(null)
    // Running again shows what will run first.
    setOnlySelected(parameters !== null)
    if (parameters) setForm(runnerForm(parameters, initialScenarios, label))
    else if (initialScenarios.length > 0)
      setForm((current) => ({
        ...current,
        scenarios: [
          ...current.scenarios,
          ...initialScenarios.filter(
            (item) => !current.scenarios.includes(item),
          ),
        ],
      }))
  }, [open, parameters, initialScenarios, label])

  useEffect(() => {
    if (!open || !bridge) return
    void refreshCatalog()
  }, [bridge, open, refreshCatalog])

  useEffect(() => {
    if (open && form.where === 'github' && github === null) void refreshGithub()
  }, [open, form.where, github, refreshGithub])

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
        (item, index, all) =>
          !listed.includes(item) && all.indexOf(item) === index,
      ),
    ]
  }, [catalog, parameters, form.scenarios])
  const choices = useMemo(
    () => suiteChoices(suites, parameters),
    [suites, parameters],
  )
  const picked = pickedSuite(form.suite, choices)
  const suite = namedSuite(form, choices)
  const stackOptions = useMemo(
    () => stackChoices(stacks, parameters),
    [stacks, parameters],
  )
  const stack = pickedStack(form.stack, stackOptions, parameters)
  const stackView = (choice: StackChoice) =>
    choice.source === 'recorded'
      ? null
      : (stacks.find((entry) => entry.id === choice.value) ?? null)

  const status: CatalogStatus = loadingCatalog
    ? 'loading'
    : catalog
      ? 'ready'
      : 'failed'
  const ready = status === 'ready'
  const where = form.where
  const needsStack = where !== 'harness'
  const githubStatus = github && github !== 'loading' ? github : null
  const githubBlocked = where === 'github' && githubStatus?.ready === false
  const runs = Math.max(1, Number(form.runs) || 1)
  const retries = Math.max(0, Number(form.technicalRetries) || 0)
  const tests = form.scenarios.length
  const pending = pendingReasons({
    ready,
    where,
    hasStack: stack !== null,
    githubBlocked,
    hasModel: form.subject !== '',
    tests,
  })
  const canRun =
    ready &&
    pending.length === 0 &&
    !(where === 'github' && github === 'loading') &&
    !submitting
  const modelName = form.subject.split('\n')[1] ?? ''
  const stackName = stack
    ? stack.source === 'recorded'
      ? `${stack.label} · as recorded`
      : stack.label
    : null

  const update = <K extends keyof RunnerForm>(key: K, value: RunnerForm[K]) =>
    setForm((current) => ({ ...current, [key]: value }))
  const select = (next: string[]) =>
    setForm((current) => ({
      ...current,
      scenarios: withSequentialGroups(
        next,
        current.scenarios,
        catalog?.groups ?? [],
      ),
    }))
  const pickWhere = (value: RunnerForm['where']) =>
    setForm((current) => ({
      ...current,
      where: value,
      // Docker and GitHub start on the repository's default stack.
      stack:
        current.stack ||
        (
          stackOptions.find((choice) => choice.value === 'default') ??
          stackOptions[0]
        )?.value ||
        '',
    }))
  const pickSuite = (value: string) => {
    const chosen = pickedSuite(value, choices)
    setForm((current) =>
      chosen
        ? {
            ...current,
            suite: value,
            scenarios: chosen.scenarios,
            runs: String(chosen.repetitions),
            technicalRetries: String(chosen.technical_retries),
          }
        : { ...current, suite: '' },
    )
  }
  const step = (key: 'runs' | 'technicalRetries', by: number) => {
    const [min, max] = key === 'runs' ? [1, 20] : [0, 3]
    const next = (key === 'runs' ? runs : retries) + by
    if (next >= min && next <= max) update(key, String(next))
  }

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!canRun || !bridge) return
    setSubmitting(true)
    setError(null)
    setRunning(null)
    try {
      const started = await bridge.startExecution(
        executionStartRequest(form, suite, stack),
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

  const suiteSource = (choice: SuiteContent) =>
    choice.recorded
      ? 'recorded'
      : (suites.find((entry) => entry.id === choice.id)?.source ?? 'repository')
  const suiteGroups: PickerGroup[] = [
    {
      label: null,
      options: [{ value: '', label: 'Custom', meta: 'tests you tick' }],
    },
    ...(
      [
        ['Repository', 'repository'],
        ['This Console', 'local'],
        ['This execution', 'recorded'],
      ] as const
    ).map(([groupLabel, source]) => ({
      label: groupLabel,
      options: choices
        .filter((choice) => suiteSource(choice) === source)
        .map((choice) => ({
          value: choiceValue(choice),
          label: choice.recorded
            ? `${choice.label} · as recorded`
            : choice.label,
          meta: plural(choice.scenarios.length, 'test', 'tests'),
        })),
    })),
  ]
  const suiteHint = suite
    ? `${SUITE_SOURCE[suiteSource(suite)]} · ${plural(suite.repetitions, 'run', 'runs')} per test · ${plural(suite.technical_retries, 'retry', 'retries')}`
    : picked
      ? `Changed from ${picked.label}. Runs as a custom selection.`
      : 'Pick a suite, or tick tests in the list.'

  const stackGroups: PickerGroup[] = (
    [
      ['Repository', 'repository'],
      ['This Console', 'local'],
      ['This execution', 'recorded'],
    ] as const
  ).map(([groupLabel, source]) => ({
    label: groupLabel,
    options: stackOptions
      .filter((choice) => choice.source === source)
      .map((choice) => {
        const view = stackView(choice)
        const warnings = view?.warnings.length ?? 0
        return {
          value: choice.value,
          label:
            choice.source === 'recorded'
              ? `${choice.label} · as recorded`
              : choice.label,
          sub: view ? stackDeclares(view) : 'As this execution recorded it',
          meta: warnings
            ? plural(warnings, 'warning', 'warnings')
            : shortSha(choice.sha256),
          metaTone: warnings ? ('warn' as const) : ('faint' as const),
        }
      }),
  }))
  const currentStack = stack ? stackView(stack) : null
  const stackWarnings = currentStack?.warnings ?? []
  const stackHint = !stack
    ? 'Its YAML is what the executor assembles.'
    : stack.source === 'recorded'
      ? `As this execution recorded it · ${shortSha(stack.sha256)}: the iii release and the runner pinned.${where === 'github' ? ' Sent as YAML.' : ''}`
      : `${currentStack ? stackDeclares(currentStack) : stack.label}. ${
          where === 'github'
            ? stack.source === 'repository'
              ? `Sent by name: the workflow reads stacks/${stack.value}.yaml from the default branch.`
              : 'Sent as YAML.'
            : 'Its YAML is what the executor assembles.'
        }`

  // The Console's own picker: searchable, grouped by provider, a popover that
  // is not clipped by the dialog and a sheet on phones. Ids are
  // `provider::model`; the form keeps `provider\nmodel`.
  const modelOptions = modelGroups(models).flatMap((group) =>
    group.models.map((model) => ({
      id: `${model.provider}::${model.model}`,
      label: model.model,
    })),
  )
  const modelHint = parameters
    ? 'The model this execution ran with.'
    : form.subject && form.subject === lastSubject
      ? 'The model of your last execution.'
      : ''

  const description = parameters
    ? where === 'github'
      ? 'Starts a new execution with the suite and parameters of this one, where it ran: on GitHub, on the stack it recorded. Change anything first.'
      : `Starts a new execution with the suite and parameters of ${label ? `“${label}”` : 'this one'}. Change anything first.`
    : 'Starts a new execution on this harness, in Docker or on GitHub.'
  const busy = running !== null && where === 'harness'

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="xl"
      tall
      title={parameters ? 'Run again' : 'Run tests'}
      description={description}
      closeLabel="Close"
      className="ds-root rd-dialog"
      hostOverlays
      bodyClassName="rd-body"
      footer={
        <div className="rd-footer">
          {busy && running ? (
            <div role="alert" className="rd-alert rd-grow" data-tone="warn">
              <TriangleAlert
                size={16}
                aria-hidden="true"
                className="rd-alert-icon"
              />
              <div className="rd-grow">
                <p className="rd-strong">
                  “{running.title}” is still running on this harness.
                </p>
                <p className="rd-faint">
                  This harness runs one execution at a time. Docker and GitHub
                  don’t wait for it.
                </p>
              </div>
              <button
                type="button"
                className="rd-ghost rd-small"
                onClick={() => {
                  pickWhere('docker')
                  setRunning(null)
                  setError(null)
                }}
              >
                Run in Docker
              </button>
              <a
                className="rd-ghost rd-small rd-link-button"
                href={hashForExecution(running.id)}
                onClick={onClose}
              >
                Open
                <ArrowRight size={16} aria-hidden="true" />
              </a>
            </div>
          ) : (
            <div className="rd-summary rd-grow" aria-live="polite">
              <p className="rd-summary-counts rd-ellipsis">
                <span>{summaryCounts(tests, runs)}</span>
                {form.subject ? (
                  <>
                    <span className="rd-faint rd-normal"> on </span>
                    <span className="rd-mono">{modelName}</span>
                  </>
                ) : null}
              </p>
              {error ? (
                <p role="alert" className="rd-summary-line" data-tone="alert">
                  <AlertCircle size={16} aria-hidden="true" />
                  {error}
                </p>
              ) : !ready || pending.length > 0 ? (
                <p className="rd-summary-line rd-faint">
                  <Info size={16} aria-hidden="true" />
                  {pendingText(ready, pending)}
                </p>
              ) : (
                <p className="rd-summary-line rd-faint rd-ellipsis">
                  {summaryDetail({
                    runs,
                    retries,
                    suite: suite?.label ?? null,
                    where,
                    stack: stackName,
                  })}
                </p>
              )}
            </div>
          )}
          <div className="rd-actions">
            <button
              type="button"
              className="rd-ghost rd-button rd-cancel"
              onClick={onClose}
            >
              Cancel
            </button>
            <button
              type="submit"
              form={id('form')}
              className="rd-primary"
              disabled={!canRun}
              aria-busy={submitting || undefined}
            >
              {submitting ? 'Starting…' : runLabel(tests, where)}
            </button>
          </div>
        </div>
      }
    >
      <form id={id('form')} className="rd-grid" onSubmit={submit} noValidate>
        <div className="rd-setup rd-scroll">
          <div className="rd-field">
            <label className="rd-label" htmlFor={id('suite')}>
              Suite
            </label>
            <Picker
              id={id('suite')}
              label="Suites"
              groups={suiteGroups}
              value={suite ? choiceValue(suite) : ''}
              valueLabel={
                suite
                  ? suite.recorded
                    ? `${suite.label} · as recorded`
                    : suite.label
                  : 'Custom'
              }
              valueMeta={
                tests
                  ? suite
                    ? plural(tests, 'test', 'tests')
                    : `${tests} ticked`
                  : ''
              }
              disabled={!ready || submitting}
              describedBy={id('suite-hint')}
              onPick={pickSuite}
            />
            <div className="rd-hint-row">
              <p id={id('suite-hint')} className="rd-hint rd-grow">
                {suiteHint}
              </p>
              {picked && !suite ? (
                <button
                  type="button"
                  className="rd-ghost rd-tiny"
                  onClick={() => pickSuite(choiceValue(picked))}
                >
                  Reset
                </button>
              ) : null}
            </div>
          </div>

          <div className="rd-field">
            <span className="rd-label" id={id('where')}>
              Where
            </span>
            <div
              role="radiogroup"
              aria-labelledby={id('where')}
              className="rd-where"
            >
              {(
                [
                  ['harness', 'This harness'],
                  ['docker', 'Docker'],
                  ['github', 'GitHub'],
                ] as const
              ).map(([value, text]) => (
                // biome-ignore lint/a11y/useSemanticElements: a segmented control styled as buttons
                <button
                  key={value}
                  type="button"
                  role="radio"
                  aria-checked={where === value}
                  className="rd-segment"
                  disabled={!ready || submitting}
                  onClick={() => pickWhere(value)}
                >
                  {text}
                </button>
              ))}
            </div>
            <p className="rd-hint">{whereHint(where, dockerGroups)}</p>
          </div>

          {needsStack ? (
            <div className="rd-field">
              <div className="rd-label-row">
                <label className="rd-label" htmlFor={id('stack')}>
                  Stack
                </label>
                <span className="rd-hint">required</span>
              </div>
              <Picker
                id={id('stack')}
                label="Stacks"
                groups={stackGroups}
                value={stack?.value ?? ''}
                valueLabel={stackName ?? 'Choose a stack'}
                placeholder={!stack}
                valueMeta={
                  stackWarnings.length
                    ? plural(stackWarnings.length, 'warning', 'warnings')
                    : ''
                }
                valueMetaTone="warn"
                disabled={!ready || submitting}
                describedBy={id('stack-hint')}
                onPick={(value) => update('stack', value)}
              />
              <div className="rd-hint-stack">
                <p id={id('stack-hint')} className="rd-hint">
                  {stackHint}
                </p>
                {stackWarnings.map((warning) => (
                  <p key={warning} className="rd-hint rd-warning">
                    <TriangleAlert size={12} aria-hidden="true" />
                    <span>{warning}</span>
                  </p>
                ))}
                <a className="rd-hint rd-underline" href={hashForStacks()}>
                  Open in Stacks
                </a>
              </div>
            </div>
          ) : null}

          {where === 'github' ? (
            <GithubCard
              status={github}
              onCheckAgain={() => void refreshGithub()}
            />
          ) : null}

          <div className="rd-field">
            <div className="rd-label-row">
              <span className="rd-label" id={id('model')}>
                Model
              </span>
              <span className="rd-hint">required</span>
            </div>
            <div className="rd-model">
              <ModelPicker
                value={form.subject ? form.subject.replace('\n', '::') : null}
                options={modelOptions}
                thinkingLevel="default"
                onChange={(next) => update('subject', next.replace('::', '\n'))}
                onThinkingLevelChange={() => {}}
                showRefresh={false}
                showProviderConfiguration={false}
                showReasoningEffort={false}
                loading={loadingCatalog}
                placeholder={!ready ? 'Catalog unavailable' : 'Choose a model'}
                disabled={!ready || submitting}
              />
            </div>
            {modelHint ? <p className="rd-hint">{modelHint}</p> : null}
          </div>

          <div className="rd-field">
            <div className="rd-steppers">
              {(
                [
                  ['runs', 'Runs per test', runs, 1, 20, 'runs'],
                  [
                    'technicalRetries',
                    'Retries on crash',
                    retries,
                    0,
                    3,
                    'retries',
                  ],
                ] as const
              ).map(([key, text, value, min, max, noun]) => (
                // biome-ignore lint/a11y/useSemanticElements: a labelled stepper group
                <div
                  key={key}
                  role="group"
                  aria-labelledby={id(key)}
                  className="rd-field"
                >
                  <span className="rd-label" id={id(key)}>
                    {text}
                  </span>
                  <div className="rd-control rd-stepper">
                    <button
                      type="button"
                      className="rd-ghost rd-step-button"
                      aria-label={`Fewer ${noun}`}
                      disabled={value <= min || submitting}
                      onClick={() => step(key, -1)}
                    >
                      <Minus size={16} aria-hidden="true" />
                    </button>
                    <output
                      id={id(`${key}-value`)}
                      className="rd-stepper-value"
                      aria-live="polite"
                    >
                      {value}
                    </output>
                    <button
                      type="button"
                      className="rd-ghost rd-step-button"
                      aria-label={`More ${noun}`}
                      disabled={value >= max || submitting}
                      onClick={() => step(key, 1)}
                    >
                      <Plus size={16} aria-hidden="true" />
                    </button>
                  </div>
                </div>
              ))}
            </div>
            <p className="rd-hint">
              More runs give steadier comparisons. A retry only reruns a crashed
              attempt and adds no sample.
            </p>
          </div>

          <div className="rd-field">
            <div className="rd-label-row">
              <label className="rd-label" htmlFor={id('agent')}>
                Agent profile
              </label>
              <span className="rd-hint">optional</span>
            </div>
            <input
              id={id('agent')}
              type="text"
              className="rd-control rd-input rd-mono"
              placeholder="Harness default"
              value={form.agent}
              disabled={submitting}
              onChange={(event) => update('agent', event.target.value)}
            />
          </div>

          <div className="rd-field">
            <div className="rd-label-row">
              <label className="rd-label" htmlFor={id('label')}>
                Label
              </label>
              <span className="rd-hint">optional</span>
            </div>
            <input
              id={id('label')}
              type="text"
              maxLength={80}
              className="rd-control rd-input"
              placeholder="Before system prompt change"
              value={form.label}
              disabled={submitting}
              onChange={(event) => update('label', event.target.value)}
            />
            <p className="rd-hint">
              Makes this execution easier to find later.
            </p>
          </div>
        </div>

        <TestsColumn
          status={status}
          tests={scenarios}
          sequences={catalog?.groups ?? []}
          selected={form.scenarios}
          onSelect={select}
          query={query}
          onQuery={setQuery}
          onlySelected={onlySelected}
          onOnlySelected={setOnlySelected}
          modelCount={catalog?.models.length ?? 0}
          onRefresh={() => void refreshCatalog()}
        />
      </form>
    </Dialog>
  )
}
