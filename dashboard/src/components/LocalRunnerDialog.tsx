import {
  Button,
  Checkbox,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  Input,
  SearchField,
  SegmentedControl,
  Select,
  Selector,
  Skeleton,
  StatusDot,
} from '@iii-dev/console-ui'
import {
  ArrowRight,
  CircleAlert,
  Info,
  Link2,
  Minus,
  Plus,
  RefreshCw,
  Search,
  TriangleAlert,
} from 'lucide-react'
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from 'react'
import { hashForExecution, hashForStacks } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  DashboardExecutionSummary,
  ExecutionParameters,
  JsonObject,
  Stack,
  Suite,
} from '@/lib/dashboard-data-source'
import {
  buildExecutionPresentation,
  executionTitle,
  providerModel,
} from '@/lib/execution-view'
import { plural } from '@/lib/format'
import {
  harnessBusy,
  pendingText,
  recordedStackDeclares,
  runLabel,
  runSummary,
  sequenceStep,
  stackDeclares,
  suiteHint,
  testFamilies,
  tickState,
  visibleTests,
} from '@/lib/run-tests'

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
  /** On this harness, or in Docker on a stack. */
  where: 'harness' | 'docker'
  /** The stack picked for Docker, by id, or the execution's as recorded. */
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
  /** Its iii, workers and pins, in one line. */
  declares: string
  /** What may not run as written; never blocking. */
  warnings: string[]
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
    declares: stackDeclares(stack),
    warnings: stack.warnings,
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
      declares: recordedStackDeclares(recorded.yaml),
      warnings: [],
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
    // Where it ran, on the stack it recorded; a GitHub run runs in Docker
    // here.
    where:
      parameters.where === 'docker' ||
      (parameters.where === 'github' && parameters.stack)
        ? 'docker'
        : 'harness',
    stack: parameters.stack ? RECORDED_STACK : '',
  }
}

/** Where it runs, changed: Docker starts on the repository's default stack,
 *  and a start the server refused is dropped, since it was about the other
 *  place (a busy harness does not hold Docker). */
export function chooseWhere(
  state: { form: RunnerForm; error: string | null },
  where: RunnerForm['where'],
  stacks: StackChoice[],
): { form: RunnerForm; error: string | null } {
  return {
    form: {
      ...state.form,
      where,
      stack:
        state.form.stack ||
        (stacks.find((choice) => choice.value === 'default') ?? stacks[0])
          ?.value ||
        '',
    },
    error: null,
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
  source?: Suite['source']
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

/** What `execution-start` receives for the form. Docker gets the stack's
 *  YAML: the executor knows nothing of this Console's stacks. */
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
      ...(form.where === 'docker' && stack
        ? { stack: { name: stack.name, yaml: stack.yaml } }
        : {}),
    },
  }
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

function shortSha(sha256: string | undefined) {
  return sha256?.replace('sha256:', '').slice(0, 12) ?? ''
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

const FORM_ID = 'run-tests-form'

const WHERE_HINTS: Record<RunnerForm['where'], string> = {
  harness: 'On the stack this Console runs on, one execution at a time.',
  docker:
    'In the executor image, one container per group, 2 groups at a time. Its page follows the groups; the results arrive once every group has finished.',
}

/** The skeleton's line widths, as the canvas draws them. */
const SKELETON = [
  'w-[180px]',
  'w-[140px]',
  'w-[210px]',
  'w-[160px]',
  'w-[120px]',
  'w-[190px]',
  'w-[150px]',
  'w-[200px]',
  'w-[130px]',
  'w-[170px]',
  'w-[110px]',
  'w-[185px]',
]

function Hint({ children }: { children: ReactNode }) {
  return (
    <p className="m-0 text-[11px] leading-4 text-(--color-ink-faint)">
      {children}
    </p>
  )
}

/** A label over its control; `meta` says required or optional. */
function Field({
  label,
  htmlFor,
  meta,
  children,
}: {
  label: string
  htmlFor?: string
  meta?: string
  children: ReactNode
}) {
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <div className="flex items-baseline justify-between">
        {htmlFor ? (
          <label htmlFor={htmlFor} className="text-xs leading-4 font-medium">
            {label}
          </label>
        ) : (
          <span className="text-xs leading-4 font-medium">{label}</span>
        )}
        {meta ? (
          <span className="text-[11px] leading-4 text-(--color-ink-faint)">
            {meta}
          </span>
        ) : null}
      </div>
      {children}
    </div>
  )
}

function Stepper({
  label,
  value,
  min,
  max,
  fewer,
  more,
  disabled,
  onChange,
}: {
  label: string
  value: number
  min: number
  max: number
  fewer: string
  more: string
  disabled: boolean
  onChange: (next: number) => void
}) {
  const step = 'max-md:size-[42px]'
  return (
    <fieldset className="m-0 min-w-0 p-0">
      <legend className="mb-1.5 p-0 text-xs leading-4 font-medium">
        {label}
      </legend>
      <div className="flex h-[48px] items-center gap-0.5 rounded-[6px] bg-(--color-surface) px-[3px] md:h-9">
        <Button
          type="button"
          variant="icon"
          size="icon"
          className={step}
          aria-label={fewer}
          disabled={disabled || value <= min}
          onClick={() => onChange(value - 1)}
        >
          <Minus aria-hidden="true" />
        </Button>
        <output className="flex-1 text-center font-mono text-[16px] tabular-nums md:text-[13px]">
          {value}
        </output>
        <Button
          type="button"
          variant="icon"
          size="icon"
          className={step}
          aria-label={more}
          disabled={disabled || value >= max}
          onClick={() => onChange(value + 1)}
        >
          <Plus aria-hidden="true" />
        </Button>
      </div>
    </fieldset>
  )
}

/** The right half: find tests, tick them by family, see the catalog. */
function TestPicker({
  tests,
  selected,
  sequences,
  query,
  filter,
  catalog,
  catalogText,
  error,
  disabled,
  onQuery,
  onFilter,
  onSelect,
  onRefresh,
}: {
  tests: string[]
  selected: string[]
  sequences: string[][]
  query: string
  filter: 'all' | 'selected'
  catalog: 'loading' | 'failed' | 'ready'
  catalogText: string
  error: string | null
  disabled: boolean
  onQuery: (next: string) => void
  onFilter: (next: 'all' | 'selected') => void
  onSelect: (next: string[]) => void
  onRefresh: () => void
}) {
  const visible = visibleTests(tests, query, filter, selected)
  const shown = tickState(visible, selected)
  const hidden = selected.filter((id) => !visible.includes(id)).length
  const families = testFamilies(tests)
    .map((family) => ({
      ...family,
      shown: family.items.filter((id) => visible.includes(id)),
    }))
    .filter((family) => family.shown.length > 0)
  const toggle = (ids: string[]) =>
    onSelect(
      tickState(ids, selected) === 'on'
        ? selected.filter((id) => !ids.includes(id))
        : [...selected, ...ids.filter((id) => !selected.includes(id))],
    )
  const loading = catalog === 'loading' && tests.length === 0
  const onlyTicked = filter === 'selected' && !query.trim()
  const count = (value: number | string) => (
    <span className="font-mono text-[11px] text-(--color-ink-faint) tabular-nums">
      {value}
    </span>
  )
  return (
    <section
      aria-label="Tests"
      className="mx-2 mb-3 flex flex-col overflow-clip rounded-[6px] bg-panel outline outline-1 -outline-offset-1 outline-(--color-edge) md:mx-0 md:mr-3 md:mb-0 md:min-h-0"
    >
      <div className="flex flex-none flex-wrap items-center gap-2 px-2.5 pt-2.5 pb-1 md:flex-nowrap">
        <SearchField
          className="min-w-0 flex-[1_1_220px]"
          aria-label="Filter tests"
          placeholder="Filter by name or id"
          value={query}
          disabled={disabled}
          onChange={onQuery}
        />
        <SegmentedControl
          variant="radio"
          aria-label="Show"
          className="max-md:flex max-md:w-full max-md:[&>button]:flex-1"
          value={filter}
          onChange={onFilter}
          options={[
            {
              value: 'all',
              label: <>All {count(catalog === 'ready' ? tests.length : '–')}</>,
            },
            {
              value: 'selected',
              label: <>Selected {count(selected.length)}</>,
            },
          ]}
        />
      </div>
      <div className="flex min-h-11 flex-none items-center gap-2.5 pr-2.5 pl-4 md:min-h-9">
        <Checkbox
          className="gap-2.5"
          aria-label="Select every test shown"
          checked={shown === 'on'}
          indeterminate={shown === 'some'}
          disabled={disabled || visible.length === 0}
          onChange={() => toggle(visible)}
          label={
            <>
              <span className="text-xs font-semibold">Tests</span>{' '}
              {count(
                tests.length === 0
                  ? ''
                  : visible.length === tests.length
                    ? tests.length
                    : `${visible.length} of ${tests.length}`,
              )}
            </>
          }
        />
        <span className="ml-auto font-mono text-[11px] whitespace-nowrap text-(--color-ink-faint) tabular-nums">
          {selected.length > 0
            ? `${selected.length} selected${hidden > 0 ? ` · ${hidden} hidden` : ''}`
            : 'none selected'}
        </span>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-[26px] px-2 text-xs font-medium text-ink"
          disabled={selected.length === 0}
          onClick={() => onSelect([])}
        >
          Clear
        </Button>
      </div>
      <div
        className="min-h-0 flex-none px-2 pt-0.5 pb-1 md:flex-1 md:overflow-y-auto"
        data-scenario-list
      >
        {catalog === 'failed' ? (
          <div
            role="alert"
            className="mx-1 mt-2 mb-2 flex items-start gap-2.5 rounded-[6px] bg-(--color-alert-muted) p-3"
          >
            <CircleAlert
              className="mt-0.5 size-4 flex-none text-(--color-alert)"
              aria-hidden="true"
            />
            <div className="min-w-0 flex-1">
              <p className="m-0 text-[13px] leading-5 font-semibold">
                Couldn’t load the test catalog
              </p>
              <p className="m-0 mt-0.5 text-xs leading-[18px] text-(--color-ink-faint)">
                {error}
              </p>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="text-xs font-medium text-ink"
              onClick={onRefresh}
            >
              <RefreshCw aria-hidden="true" />
              Retry
            </Button>
          </div>
        ) : null}
        {loading ? (
          <div
            role="status"
            aria-busy="true"
            aria-label="Loading tests"
            className="flex flex-col px-2.5 pt-1.5"
          >
            {SKELETON.map((width) => (
              <div
                key={width}
                className="flex h-11 items-center gap-2.5 md:h-[30px]"
              >
                <Skeleton className="size-[18px] flex-none rounded-[6px]" />
                <Skeleton className={`h-2.5 rounded-[6px] ${width}`} />
              </div>
            ))}
          </div>
        ) : null}
        {families.map((family) => {
          const state = tickState(family.shown, selected)
          const ticked = family.items.filter((id) =>
            selected.includes(id),
          ).length
          return (
            // biome-ignore lint/a11y/useSemanticElements: a fieldset's legend cannot be the sticky header row
            <div
              key={family.key}
              role="group"
              aria-label={family.label}
              className="mb-2 rounded-[6px] bg-(--color-surface) px-1 pb-1"
              data-scenario-group={family.key}
            >
              {/* Opaque, so rows scroll under it: the block's fill over the
                  panel's. */}
              <div className="sticky top-0 z-[1] -mx-1 mb-0.5 rounded-t-[6px] bg-panel">
                <div className="flex min-h-11 items-center gap-2.5 rounded-t-[6px] bg-(--color-surface) pr-3 pl-3.5 md:min-h-8">
                  <Checkbox
                    className="gap-2.5"
                    aria-label={`Select every test in ${family.label}`}
                    checked={state === 'on'}
                    indeterminate={state === 'some'}
                    disabled={disabled}
                    onChange={() => toggle(family.shown)}
                    label={
                      <span
                        className={`text-[13px] font-semibold ${family.mono ? 'font-mono' : ''}`}
                      >
                        {family.label}
                      </span>
                    }
                  />
                  <span className="ml-auto inline-flex h-5 items-center rounded-[6px] bg-(--color-surface-selected) px-[7px] font-mono text-[11px] font-medium tabular-nums">
                    {ticked > 0
                      ? `${ticked}/${family.items.length}`
                      : family.items.length}
                  </span>
                </div>
              </div>
              {family.shown.map((id) => {
                const on = selected.includes(id)
                const step = sequenceStep(id, sequences)
                return (
                  <Checkbox
                    key={id}
                    className="flex min-h-11 w-full gap-2.5 rounded-[6px] px-2.5 hover:bg-(--color-surface-hover) md:min-h-[30px] [&>span:last-child]:flex [&>span:last-child]:min-w-0 [&>span:last-child]:flex-1 [&>span:last-child]:items-center [&>span:last-child]:gap-2.5"
                    aria-label={id}
                    checked={on}
                    disabled={disabled}
                    onChange={() =>
                      onSelect(
                        on
                          ? selected.filter((item) => item !== id)
                          : [...selected, id],
                      )
                    }
                    label={
                      <>
                        <span className="min-w-0 flex-1 truncate font-mono text-sm md:text-xs">
                          {id}
                        </span>
                        {step ? (
                          <span className="inline-flex flex-none items-center gap-1 text-[11px] whitespace-nowrap text-(--color-ink-faint)">
                            <Link2 className="size-4" aria-hidden="true" />
                            {step}
                          </span>
                        ) : null}
                      </>
                    }
                  />
                )
              })}
            </div>
          )
        })}
        {!loading && tests.length > 0 && visible.length === 0 ? (
          <div className="flex flex-col items-center gap-2.5 px-5 py-10 text-center">
            <Search
              className="size-4 text-(--color-ink-faint)"
              aria-hidden="true"
            />
            <p className="m-0 text-[13px] font-semibold">
              {onlyTicked
                ? 'No tests ticked yet.'
                : `No tests match “${query}”.`}
            </p>
            <Button
              type="button"
              variant="pill"
              size="sm"
              className="text-xs font-medium"
              onClick={() => {
                onQuery('')
                onFilter('all')
              }}
            >
              {onlyTicked ? 'Show all tests' : 'Clear filter'}
            </Button>
          </div>
        ) : null}
      </div>
      <div className="flex h-7 flex-none items-center gap-2 bg-(--color-surface) pr-1 pl-3 text-[11px] text-(--color-ink-faint) tabular-nums">
        <StatusDot
          tone={
            catalog === 'loading'
              ? 'accent'
              : catalog === 'failed'
                ? 'alert'
                : 'ok'
          }
          pulse={catalog === 'loading'}
        />
        <span role="status" className="min-w-0 flex-1 truncate">
          {catalogText}
        </span>
        <Button
          type="button"
          variant="icon"
          size="icon"
          className="size-6"
          aria-label="Refresh catalog"
          disabled={catalog === 'loading'}
          onClick={onRefresh}
        >
          <RefreshCw aria-hidden="true" />
        </Button>
      </div>
    </section>
  )
}

/** Run tests and Run again: one form that starts an execution on this
 *  harness or in Docker and then follows it on its page. */
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
  const [stacks, setStacks] = useState<Stack[]>([])
  const [form, setForm] = useState<RunnerForm>(initialForm)
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<'all' | 'selected'>('all')
  const [loadingCatalog, setLoadingCatalog] = useState(false)
  const [catalogError, setCatalogError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // The execution holding this harness, to open or to run beside in Docker.
  const [running, setRunning] = useState<{ id: string; title: string } | null>(
    null,
  )
  // The model taken from the last execution, said so under the field.
  const [lastSubject, setLastSubject] = useState('')

  const refreshCatalog = useCallback(async () => {
    if (!bridge) return
    setLoadingCatalog(true)
    setCatalogError(null)
    try {
      const [next, recent, listed, stackList] = await Promise.all([
        bridge.getCatalog().then(asCatalog),
        bridge
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
      // Running again brings its own model.
      const last = parameters ? null : lastUsedModel(recent, next.models)
      setLastSubject(last ? modelKey(last) : '')
      setRunning(harnessBusy(recent))
      setCatalog(next)
      setSuites(listed)
      setStacks(stackList)
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
      setCatalogError(errorMessage(cause))
    } finally {
      setLoadingCatalog(false)
    }
  }, [bridge, parameters])

  useEffect(() => {
    if (!open) return
    setRunning(null)
    setError(null)
    setQuery('')
    // Running again shows what will run first.
    setFilter(parameters ? 'selected' : 'all')
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
  const picked = pickedSuite(form.suite, choices)
  const suite = namedSuite(form, choices)
  const stackOptions = useMemo(
    () => stackChoices(stacks, parameters),
    [stacks, parameters],
  )
  const stack = pickedStack(form.stack, stackOptions, parameters)
  const docker = form.where === 'docker'
  const sequences = catalog?.groups ?? []
  const runs = Math.max(1, Number(form.runs) || 1)
  const retries = Math.max(0, Number(form.technicalRetries) || 0)
  const tests = form.scenarios.length
  // Nothing to work with until the catalog answers: a fresh Run tests. Run
  // again holds its own tests and model and runs without it.
  const noCatalog = !catalog && scenarios.length === 0
  const catalogState = catalog
    ? loadingCatalog
      ? 'loading'
      : 'ready'
    : catalogError
      ? 'failed'
      : 'loading'
  const pending = pendingText({
    loading: noCatalog || (!catalog && catalogState === 'loading'),
    noStack: docker && !stack,
    noModel: !form.subject,
    tests,
  })
  const busy = running !== null && !docker
  const modelName = form.subject.split('\n')[1] ?? ''
  const summary = runSummary({
    tests,
    runs,
    retries,
    suite: suite?.label ?? null,
    where: form.where,
    stack: stack?.label ?? null,
  })

  const update = <K extends keyof RunnerForm>(key: K, value: RunnerForm[K]) =>
    setForm((current) => ({ ...current, [key]: value }))
  const pickWhere = (value: RunnerForm['where']) => {
    const next = chooseWhere({ form, error }, value, stackOptions)
    setForm(next.form)
    setError(next.error)
  }
  const pickSuite = (value: string) => {
    const chosen = pickedSuite(value, choices)
    if (chosen)
      setForm((current) => ({
        ...current,
        suite: value,
        scenarios: chosen.scenarios,
        runs: String(chosen.repetitions),
        technicalRetries: String(chosen.technical_retries),
      }))
  }
  const select = (next: string[]) =>
    update('scenarios', withSequentialGroups(next, form.scenarios, sequences))

  const suiteOption = (choice: SuiteContent) => ({
    value: choiceValue(choice),
    label: choice.recorded ? `${choice.label} · as recorded` : choice.label,
    description: plural(choice.scenarios.length, 'test'),
  })
  const suiteGroups = [
    {
      label: 'Repository',
      options: suites.filter((c) => c.source === 'repository'),
    },
    {
      label: 'This Console',
      options: suites.filter((c) => c.source === 'local'),
    },
    { label: 'This execution', options: choices.filter((c) => c.recorded) },
  ]
    .filter((group) => group.options.length > 0)
    .map((group) => ({ ...group, options: group.options.map(suiteOption) }))
  const stackGroups = [
    { label: 'Repository', source: 'repository' },
    { label: 'This Console', source: 'local' },
    { label: 'This execution', source: 'recorded' },
  ]
    .map(({ label, source }) => ({
      label,
      options: stackOptions
        .filter((choice) => choice.source === source)
        .map((choice) => ({
          value: choice.value,
          label:
            choice.source === 'recorded'
              ? `${choice.label} · as recorded`
              : choice.label,
          description: [
            choice.declares,
            choice.warnings.length > 0
              ? plural(choice.warnings.length, 'warning')
              : choice.source === 'recorded'
                ? shortSha(choice.sha256)
                : '',
          ]
            .filter(Boolean)
            .join(' · '),
        })),
    }))
    .filter((group) => group.options.length > 0)
  const recordedSha = shortSha(stack?.sha256)
  const stackHint = !stack
    ? 'Its YAML is what the executor assembles.'
    : stack.source === 'recorded'
      ? `As this execution recorded it${recordedSha ? ` · ${recordedSha}` : ''}: the iii release and the runner pinned.`
      : `${stack.declares}. Its YAML is what the executor assembles.`
  const modelOptions = [...models]
    .sort(
      (a, b) =>
        a.provider.localeCompare(b.provider) || a.model.localeCompare(b.model),
    )
    .filter(
      (model, index, all) =>
        all.findIndex((other) => modelKey(other) === modelKey(model)) === index,
    )
    // "provider / model", as the canvas writes it.
    .map((model) => ({
      value: modelKey(model),
      label: providerModel(model).replace('/', ' / '),
    }))
  const modelHint = !form.subject
    ? null
    : parameters
      ? form.subject === modelKey(parameters)
        ? 'The model this execution ran with.'
        : null
      : form.subject === lastSubject
        ? 'The model of your last execution.'
        : null

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (pending || submitting || !bridge) return
    setSubmitting(true)
    setError(null)
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

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="flex h-[min(760px,calc(100dvh-2rem))] max-h-none w-[calc(100vw-2rem)] max-w-[1040px] flex-col overflow-hidden rounded-[6px] p-0 max-md:top-11 max-md:bottom-1.5 max-md:left-1.5 max-md:h-auto max-md:w-[calc(100vw-12px)] max-md:translate-none">
        <div aria-hidden="true" className="flex justify-center pt-2 md:hidden">
          <span className="h-1 w-9 rounded-[6px] bg-(--color-surface-selected)" />
        </div>
        <header className="flex-none pt-3 pr-14 pb-2 pl-4 md:pt-4 md:pr-12 md:pb-3 md:pl-5">
          <DialogTitle className="m-0 text-[16px] leading-[22px] font-semibold tracking-[-0.01em] md:text-[15px]">
            {parameters ? 'Run again' : 'Run tests'}
          </DialogTitle>
          <DialogDescription className="m-0 mt-0.5 text-xs leading-[18px] text-(--color-ink-faint)">
            {parameters
              ? `Starts a new execution with the suite and parameters of ${label ? `“${label}”` : 'this execution'}. Change anything first.`
              : 'Starts a new execution on this harness or in Docker.'}
          </DialogDescription>
        </header>
        <form
          id={FORM_ID}
          onSubmit={submit}
          noValidate
          className="grid min-h-0 flex-1 grid-cols-1 content-start overflow-y-auto md:grid-cols-[340px_minmax(0,1fr)] md:content-stretch md:overflow-hidden"
        >
          <div className="flex min-w-0 flex-col gap-5 px-4 pt-1 pb-5 md:min-h-0 md:overflow-y-auto md:px-5">
            <Field label="Suite" htmlFor="run-tests-suite">
              <Select
                id="run-tests-suite"
                aria-label="Suite"
                // "Custom" is a value, not a hint: ink like any suite (the
                // host paints its placeholder ghost).
                className="w-full font-medium data-[placeholder]:text-ink"
                value={suite ? choiceValue(suite) : undefined}
                groups={suiteGroups}
                allowEmpty
                emptyLabel="Custom"
                placeholder="Custom"
                disabled={noCatalog || submitting}
                onChange={pickSuite}
                onClear={() => update('suite', '')}
              />
              <div className="flex min-h-4 items-baseline gap-2">
                <p className="m-0 flex-1 text-[11px] leading-4 text-(--color-ink-faint)">
                  {suiteHint(suite, picked) ?? (
                    <>
                      Pick a suite, or tick tests{' '}
                      <span className="md:hidden">below.</span>
                      <span className="max-md:hidden">on the right.</span>
                    </>
                  )}
                </p>
                {picked && !suite ? (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    className="h-[22px] px-1.5 text-[11px] font-medium text-ink"
                    onClick={() => pickSuite(choiceValue(picked))}
                  >
                    Reset
                  </Button>
                ) : null}
              </div>
            </Field>

            <Field label="Where">
              <SegmentedControl
                variant="radio"
                aria-label="Where"
                className="flex w-full [&>button]:flex-1"
                value={form.where}
                onChange={pickWhere}
                options={[
                  { value: 'harness', label: 'This harness' },
                  { value: 'docker', label: 'Docker' },
                ]}
              />
              <Hint>{WHERE_HINTS[form.where]}</Hint>
            </Field>

            {docker ? (
              <Field label="Stack" htmlFor="run-tests-stack" meta="required">
                <Select
                  id="run-tests-stack"
                  aria-label="Stack"
                  className="w-full data-[placeholder]:text-(--color-ink-faint)"
                  value={stack?.value}
                  groups={stackGroups}
                  placeholder="Choose a stack"
                  disabled={noCatalog || submitting}
                  onChange={(value) => update('stack', value)}
                />
                <div className="flex flex-col gap-[3px]">
                  <Hint>{stackHint}</Hint>
                  {stack?.warnings.map((warning, index) => (
                    <p
                      // biome-ignore lint/suspicious/noArrayIndexKey: warnings repeat and never reorder
                      key={index}
                      className="m-0 flex items-start gap-1.5 text-[11px] leading-4"
                    >
                      <TriangleAlert
                        className="mt-0.5 size-3 flex-none text-(--color-warn)"
                        aria-hidden="true"
                      />
                      <span>{warning}</span>
                    </p>
                  ))}
                  <a
                    href={hashForStacks()}
                    onClick={onClose}
                    className="self-start text-[11px] leading-4 text-(--color-ink-faint) underline underline-offset-2"
                  >
                    Open in Stacks
                  </a>
                </div>
              </Field>
            ) : null}

            <Field label="Model" htmlFor="run-tests-model" meta="required">
              <Selector
                id="run-tests-model"
                aria-label="Model"
                // The host's ghost placeholder reads at 2.1:1; faint passes.
                className={`[&_button]:font-mono ${form.subject ? '' : '[&_button>span]:text-(--color-ink-faint)'}`}
                value={form.subject || undefined}
                options={modelOptions}
                placeholder={
                  noCatalog && catalogState === 'loading'
                    ? 'Loading models…'
                    : modelOptions.length === 0
                      ? 'Catalog unavailable'
                      : 'Choose a model'
                }
                searchPlaceholder="Find a model"
                emptyMessage="No model matches."
                disabled={submitting || modelOptions.length === 0}
                onChange={(value) => update('subject', value)}
              />
              {modelHint ? <Hint>{modelHint}</Hint> : null}
            </Field>

            <div className="flex flex-col gap-1.5">
              <div className="grid grid-cols-2 gap-3">
                <Stepper
                  label="Runs per test"
                  value={runs}
                  min={1}
                  max={20}
                  fewer="Fewer runs"
                  more="More runs"
                  disabled={submitting}
                  onChange={(next) => update('runs', String(next))}
                />
                <Stepper
                  label="Retries on crash"
                  value={retries}
                  min={0}
                  max={3}
                  fewer="Fewer retries"
                  more="More retries"
                  disabled={submitting}
                  onChange={(next) => update('technicalRetries', String(next))}
                />
              </div>
              <Hint>
                More runs give steadier comparisons. A retry only reruns a
                crashed attempt and adds no sample.
              </Hint>
            </div>

            <Field
              label="Agent profile"
              htmlFor="run-tests-agent"
              meta="optional"
            >
              <Input
                id="run-tests-agent"
                className="font-mono"
                placeholder="Harness default"
                value={form.agent}
                disabled={submitting}
                onChange={(value) => update('agent', value)}
              />
            </Field>

            <Field label="Label" htmlFor="run-tests-label" meta="optional">
              <Input
                id="run-tests-label"
                maxLength={80}
                placeholder="Before system prompt change"
                value={form.label}
                disabled={submitting}
                onChange={(value) => update('label', value)}
              />
              <Hint>Makes this execution easier to find later.</Hint>
            </Field>
          </div>

          <TestPicker
            tests={scenarios}
            selected={form.scenarios}
            sequences={sequences}
            query={query}
            filter={filter}
            catalog={catalogState}
            catalogText={
              catalogState === 'loading'
                ? 'Loading catalog…'
                : catalog
                  ? `Catalog ready · ${plural(catalog.scenarios.length, 'test')} · ${plural(catalog.models.length, 'model')}`
                  : 'Catalog unavailable'
            }
            error={catalogError}
            disabled={noCatalog || submitting}
            onQuery={setQuery}
            onFilter={setFilter}
            onSelect={select}
            onRefresh={() => void refreshCatalog()}
          />
        </form>

        <footer className="flex flex-none flex-col items-stretch gap-3 px-4 pt-3 pb-4 md:flex-row md:items-center md:pr-4 md:pb-3.5 md:pl-5">
          {busy && running ? (
            <div
              role="alert"
              className="flex min-w-0 flex-1 flex-wrap items-center gap-2.5 rounded-[6px] bg-(--color-warn-muted) py-2 pr-1.5 pl-2.5"
            >
              <TriangleAlert
                className="size-4 flex-none text-(--color-warn)"
                aria-hidden="true"
              />
              <div className="min-w-0 flex-1">
                <p className="m-0 text-xs leading-4 font-semibold">
                  “{running.title}” is still running on this harness.
                </p>
                <p className="m-0 text-xs leading-4 text-(--color-ink-faint)">
                  This harness runs one execution at a time. Docker doesn’t wait
                  for it.
                </p>
              </div>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-7 px-2 text-xs font-medium text-ink"
                onClick={() => pickWhere('docker')}
              >
                Run in Docker
              </Button>
              <a
                href={hashForExecution(running.id)}
                aria-label={`Open ${running.title}`}
                onClick={onClose}
                className="inline-flex h-7 flex-none items-center gap-1 rounded-[6px] px-2 text-xs font-medium hover:bg-(--color-surface-hover)"
              >
                Open
                <ArrowRight className="size-4" aria-hidden="true" />
              </a>
            </div>
          ) : (
            <div
              aria-live="polite"
              className="flex min-w-0 flex-1 flex-col gap-0.5"
            >
              <p className="m-0 truncate text-[13px] leading-[18px] font-medium">
                <span className="tabular-nums">{summary.counts}</span>
                {modelName ? (
                  <>
                    <span className="font-normal text-(--color-ink-faint)">
                      {' '}
                      on{' '}
                    </span>
                    <span className="font-mono text-xs">{modelName}</span>
                  </>
                ) : null}
              </p>
              {error ? (
                <p
                  role="alert"
                  className="m-0 flex items-start gap-1.5 text-xs leading-4"
                >
                  <CircleAlert
                    className="size-4 flex-none text-(--color-alert)"
                    aria-hidden="true"
                  />
                  {error}
                </p>
              ) : pending ? (
                <p className="m-0 flex items-center gap-1.5 text-xs leading-4 text-(--color-ink-faint)">
                  <Info className="size-4 flex-none" aria-hidden="true" />
                  {pending}
                </p>
              ) : (
                <p className="m-0 truncate text-xs leading-4 text-(--color-ink-faint) tabular-nums">
                  {summary.detail}
                </p>
              )}
            </div>
          )}
          <div className="flex flex-none items-center gap-2">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="font-medium text-ink max-md:hidden"
              onClick={onClose}
            >
              Cancel
            </Button>
            <Button
              type="submit"
              form={FORM_ID}
              variant="primary"
              size="sm"
              className="flex-1 px-3.5 font-medium max-md:h-12"
              disabled={pending !== null || submitting}
              aria-busy={submitting}
            >
              {submitting ? 'Starting…' : runLabel(tests, form.where)}
            </Button>
          </div>
        </footer>
      </DialogContent>
    </Dialog>
  )
}
