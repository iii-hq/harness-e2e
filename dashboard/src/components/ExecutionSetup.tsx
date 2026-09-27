import { RefreshCw, Search, X } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import {
  buttonClassName,
  Field,
  FilterChip,
  FilterChipGroup,
  fieldDescribedBy,
  Input,
} from '@/design-system'
import '@/design-system/styles.css'

/* The suite editor's form: name, runs and retries, and the tests a suite
   runs. (Running tests has its own dialog: components/run-dialog.) */

export type ExecutionSetupField = 'label' | 'scenarios'
export type ExecutionSetupErrors = Partial<Record<ExecutionSetupField, string>>

/** Audit PN-05: validation runs on submit and names each pending item. A
 *  suite needs a name and at least one test. */
export function validateExecutionSetup({
  label,
  selectedScenarios,
}: {
  label: string
  selectedScenarios: string[]
}): ExecutionSetupErrors {
  const errors: ExecutionSetupErrors = {}
  if (label.trim() === '') errors.label = 'Name the suite.'
  if (selectedScenarios.length === 0)
    errors.scenarios = 'Select at least one test.'
  return errors
}

/** Moves focus to the first field the validation named (audit PN-05). */
export function focusFirstInvalid(
  idPrefix: string,
  errors: ExecutionSetupErrors,
) {
  const order: [ExecutionSetupField, string][] = [
    ['label', `${idPrefix}-label`],
    ['scenarios', `${idPrefix}-scenario-search`],
  ]
  for (const [field, id] of order) {
    if (!errors[field]) continue
    const element = document.getElementById(id)
    if (element instanceof HTMLElement) {
      element.focus()
      return
    }
  }
}

export function scenarioDisplayName(scenario: string) {
  return scenario
    .replace(/[_.]+/g, ' ')
    .replace(/\b\w/g, (character) => character.toUpperCase())
}

export type ScenarioGroup = {
  key: string
  label: string
  items: string[]
}

/**
 * Audit PN-09 / RS-05: tests grouped by family (the id's first segment) so
 * a group can be selected at once; singletons gather under "other".
 */
export function groupScenarios(ids: string[]): ScenarioGroup[] {
  const families = new Map<string, string[]>()
  for (const id of ids) {
    const family = id.split(/[_.]/)[0] || id
    families.set(family, [...(families.get(family) ?? []), id])
  }
  const groups: ScenarioGroup[] = []
  const singles: string[] = []
  for (const [family, items] of [...families.entries()].sort(([a], [b]) =>
    a.localeCompare(b),
  )) {
    if (items.length > 1) groups.push({ key: family, label: family, items })
    else singles.push(...items)
  }
  if (singles.length > 0)
    groups.push({ key: 'other', label: 'other tests', items: singles })
  return groups
}

type ExecutionSetupProps = {
  idPrefix: string
  label: string
  availableScenarios: string[]
  selectedScenarios: string[]
  query: string
  runs: string
  technicalRetries: string
  /** Open on the "selected" filter (editing: what the suite runs). */
  initialOnlySelected?: boolean
  disabled?: boolean
  catalogLoading?: boolean
  catalogStatus: { tone: 'ready' | 'loading' | 'unavailable'; text: string }
  errors?: ExecutionSetupErrors
  /** Where the sticky search sits: at the top of a dialog body or below the page navigation. */
  stickyOffset?: 'page' | 'dialog'
  onRefreshCatalog?: () => void
  onLabelChange: (value: string) => void
  onSelectedScenariosChange: (value: string[]) => void
  onQueryChange: (value: string) => void
  onRunsChange: (value: string) => void
  onTechnicalRetriesChange: (value: string) => void
}

function SetupSection({
  id,
  title,
  description,
  children,
}: {
  id: string
  title: string
  description?: string
  children: ReactNode
}) {
  return (
    <section className="grid min-w-0 gap-4" aria-labelledby={`${id}-title`}>
      <div className="min-w-0">
        <h2
          id={`${id}-title`}
          className="m-0 text-sm font-semibold tracking-[-0.015em] text-ink"
        >
          {title}
        </h2>
        {description ? (
          <p className="mt-1 mb-0 max-w-[48rem] text-xs leading-5 text-ink-soft">
            {description}
          </p>
        ) : null}
      </div>
      {children}
    </section>
  )
}

function clampNumber(value: string, min: number, max: number) {
  if (value.trim() === '') return value
  const number = Number(value)
  if (!Number.isFinite(number)) return value
  return String(Math.min(max, Math.max(min, Math.round(number))))
}

export function ExecutionSetup({
  idPrefix,
  label,
  availableScenarios,
  selectedScenarios,
  query,
  runs,
  technicalRetries,
  initialOnlySelected = false,
  disabled = false,
  catalogLoading = false,
  catalogStatus,
  errors = {},
  stickyOffset = 'page',
  onRefreshCatalog,
  onLabelChange,
  onSelectedScenariosChange,
  onQueryChange,
  onRunsChange,
  onTechnicalRetriesChange,
}: ExecutionSetupProps) {
  const [onlySelected, setOnlySelected] = useState(initialOnlySelected)
  const normalizedQuery = query.trim().toLocaleLowerCase()
  const matches = (scenario: string) =>
    (!normalizedQuery ||
      `${scenario} ${scenarioDisplayName(scenario)}`
        .toLocaleLowerCase()
        .includes(normalizedQuery)) &&
    (!onlySelected || selectedScenarios.includes(scenario))
  const visibleScenarios = availableScenarios.filter(matches)
  const groups = groupScenarios(visibleScenarios)
  const runsPerScenario = Math.max(1, Number(runs) || 1)
  const plannedRuns = selectedScenarios.length * runsPerScenario

  const toggleScenario = (scenario: string, checked: boolean) => {
    onSelectedScenariosChange(
      checked
        ? selectedScenarios.includes(scenario)
          ? selectedScenarios
          : [...selectedScenarios, scenario]
        : selectedScenarios.filter((item) => item !== scenario),
    )
  }
  const selectMany = (scenarios: string[]) => {
    onSelectedScenariosChange([
      ...selectedScenarios,
      ...scenarios.filter((scenario) => !selectedScenarios.includes(scenario)),
    ])
  }
  const deselectMany = (scenarios: string[]) => {
    onSelectedScenariosChange(
      selectedScenarios.filter((scenario) => !scenarios.includes(scenario)),
    )
  }

  // Part of a suite; tucked under "Advanced" when running tests.
  const sampling = (
    <>
      <Field
        label="Runs per test"
        htmlFor={`${idPrefix}-runs`}
        hint="Each test runs this many times. More runs make comparisons more reliable. Max 20."
      >
        <Input
          id={`${idPrefix}-runs`}
          className="font-mono"
          type="number"
          min="1"
          max="20"
          inputMode="numeric"
          value={runs}
          onChange={(event) => onRunsChange(event.target.value)}
          onBlur={(event) =>
            onRunsChange(clampNumber(event.target.value, 1, 20))
          }
          disabled={disabled}
        />
      </Field>
      <Field
        label="Technical retries"
        htmlFor={`${idPrefix}-retries`}
        hint="Reruns a test after a crash. Does not add a sample. Max 3."
      >
        <Input
          id={`${idPrefix}-retries`}
          className="font-mono"
          type="number"
          min="0"
          max="3"
          inputMode="numeric"
          value={technicalRetries}
          onChange={(event) => onTechnicalRetriesChange(event.target.value)}
          onBlur={(event) =>
            onTechnicalRetriesChange(clampNumber(event.target.value, 0, 3))
          }
          disabled={disabled}
        />
      </Field>
    </>
  )

  const statusDot =
    catalogStatus.tone === 'ready'
      ? 'bg-success'
      : catalogStatus.tone === 'loading'
        ? 'bg-[var(--ink-decor)]'
        : 'bg-danger'
  const hiddenSelected = selectedScenarios.filter(
    (scenario) => !visibleScenarios.includes(scenario),
  ).length

  return (
    // Audit RS-15: when something else holds the form, park it rather than
    // leaving 68 dead controls at full strength. The reader can still read
    // what they would be configuring, and the callout above says why.
    <div
      className={`grid min-w-0 gap-8 ${disabled ? 'opacity-55' : ''}`}
      data-execution-setup="suite"
      data-parked={disabled || undefined}
    >
      {/* Audit PN-26 / RS-06: catalog status in the status vocabulary. */}
      <div
        className="flex min-w-0 flex-wrap items-center gap-3 font-mono text-xs"
        aria-live="polite"
      >
        <span className="flex min-w-0 items-start gap-2 text-ink-soft">
          <span
            className={`mt-1.5 size-1.5 shrink-0 rounded-full ${statusDot}`}
            aria-hidden="true"
          />
          <span className="min-w-0 break-words">{catalogStatus.text}</span>
        </span>
        {onRefreshCatalog ? (
          <button
            className={buttonClassName({ variant: 'quiet', size: 'compact' })}
            type="button"
            onClick={onRefreshCatalog}
            disabled={disabled || catalogLoading}
            title="Refresh catalog"
            aria-label="Refresh catalog"
          >
            <RefreshCw
              className={catalogLoading ? 'animate-spin' : ''}
              size={13}
              aria-hidden="true"
            />
          </button>
        ) : null}
      </div>

      <SetupSection
        id={`${idPrefix}-details`}
        title="Name the suite"
        description="The name the suite is listed and run by."
      >
        <div className="grid items-start gap-4 sm:grid-cols-2">
          <Field
            label="Suite name"
            htmlFor={`${idPrefix}-label`}
            meta="required"
            error={errors.label}
          >
            <Input
              id={`${idPrefix}-label`}
              value={label}
              maxLength={160}
              placeholder="Regression without the slow tests"
              aria-invalid={errors.label ? true : undefined}
              aria-describedby={fieldDescribedBy(`${idPrefix}-label`, {
                error: Boolean(errors.label),
              })}
              onChange={(event) => onLabelChange(event.target.value)}
              disabled={disabled}
            />
          </Field>
        </div>
      </SetupSection>

      <SetupSection
        id={`${idPrefix}-sampling`}
        title="Runs and retries"
        description="How many times each test runs, and how many times a crash is retried."
      >
        <div className="grid gap-4 sm:grid-cols-2">{sampling}</div>
      </SetupSection>

      <SetupSection
        id={`${idPrefix}-scope`}
        title="Pick the tests"
        description="The tests the suite runs."
      >
        <div
          className={`sticky z-10 grid min-w-0 gap-3 bg-panel py-2 ${
            stickyOffset === 'dialog' ? 'top-0' : 'top-12'
          }`}
        >
          <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
            <div className="relative">
              <Search
                className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-ink-muted"
                size={14}
                aria-hidden="true"
              />
              <Input
                id={`${idPrefix}-scenario-search`}
                style={{ paddingInline: '2.25rem' }}
                type="text"
                value={query}
                placeholder="Search by name or id"
                aria-label="Find a test"
                onChange={(event) => onQueryChange(event.target.value)}
                disabled={disabled}
              />
              {query ? (
                <button
                  className="absolute top-1/2 right-1 inline-grid size-7 -translate-y-1/2 place-items-center rounded-[6px] border-0 bg-transparent text-ink-muted hover:bg-[var(--surface-soft)] hover:text-ink"
                  type="button"
                  onClick={() => onQueryChange('')}
                  aria-label="Clear search"
                >
                  <X size={13} aria-hidden="true" />
                </button>
              ) : null}
            </div>
            <div className="flex flex-wrap gap-2">
              <button
                className={buttonClassName({
                  variant: 'secondary',
                  size: 'compact',
                })}
                type="button"
                onClick={() => selectMany(visibleScenarios)}
                disabled={disabled || visibleScenarios.length === 0}
              >
                select visible ({visibleScenarios.length})
              </button>
              <button
                className={buttonClassName({
                  variant: 'quiet',
                  size: 'compact',
                })}
                type="button"
                onClick={() => onSelectedScenariosChange([])}
                disabled={disabled || selectedScenarios.length === 0}
              >
                clear
              </button>
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-3">
            <FilterChipGroup label="Test filters">
              <FilterChip
                active={!onlySelected}
                count={availableScenarios.length}
                onClick={() => setOnlySelected(false)}
              >
                all
              </FilterChip>
              <FilterChip
                active={onlySelected}
                count={selectedScenarios.length}
                onClick={() => setOnlySelected(true)}
                disabled={selectedScenarios.length === 0 && !onlySelected}
              >
                selected
              </FilterChip>
            </FilterChipGroup>
            <output
              className="ms-auto min-w-0 font-mono text-label text-ink-muted"
              aria-live="polite"
              htmlFor={`${idPrefix}-scenario-search`}
            >
              {visibleScenarios.length} of {availableScenarios.length} shown ·{' '}
              {selectedScenarios.length} selected
              {hiddenSelected > 0 ? ` (${hiddenSelected} hidden)` : ''} ·{' '}
              {plannedRuns} {plannedRuns === 1 ? 'run' : 'runs'} in total
            </output>
          </div>
        </div>
        {errors.scenarios ? (
          <p
            className="m-0 text-xs text-danger"
            role="alert"
            id={`${idPrefix}-scenarios-error`}
          >
            {errors.scenarios}
          </p>
        ) : null}
        <div className="grid gap-5" data-scenario-list>
          {groups.map((group) => {
            const allSelected =
              group.items.length > 0 &&
              group.items.every((scenario) =>
                selectedScenarios.includes(scenario),
              )
            return (
              <div key={group.key} data-scenario-group={group.key}>
                <div className="flex items-center justify-between gap-3 border-b border-line pb-1">
                  <span className="ds-label">
                    {group.label} · {group.items.length}
                  </span>
                  {group.items.length > 0 ? (
                    <button
                      className={buttonClassName({
                        variant: 'quiet',
                        size: 'compact',
                      })}
                      type="button"
                      onClick={() =>
                        allSelected
                          ? deselectMany(group.items)
                          : selectMany(group.items)
                      }
                      disabled={disabled}
                    >
                      {allSelected ? 'clear group' : 'select group'}
                    </button>
                  ) : null}
                </div>
                <ul className="m-0 grid list-none p-0">
                  {group.items.map((scenario) => {
                    const selected = selectedScenarios.includes(scenario)
                    return (
                      <li key={scenario}>
                        <label
                          className={`flex min-h-9 min-w-0 items-center gap-3 rounded-[6px] px-2 text-xs ${
                            selected
                              ? 'cursor-pointer bg-[var(--surface-selected)] text-ink'
                              : 'cursor-pointer text-ink hover:bg-[var(--surface-fill)]'
                          }`}
                        >
                          {/* The native control, sized and shown whatever
                              the host resets: the box, its name and Space all
                              toggle it. */}
                          <input
                            className="m-0 size-4 shrink-0 cursor-pointer appearance-auto accent-[var(--accent)]"
                            type="checkbox"
                            checked={selected}
                            disabled={disabled}
                            onChange={(event) =>
                              toggleScenario(scenario, event.target.checked)
                            }
                          />
                          <span className="min-w-0 flex-1 truncate font-mono">
                            {scenario}
                          </span>
                        </label>
                      </li>
                    )
                  })}
                </ul>
              </div>
            )
          })}
          {visibleScenarios.length === 0 ? (
            <CatalogEmptyState
              query={query}
              onlySelected={onlySelected}
              catalogLoading={catalogLoading}
              catalogEmpty={availableScenarios.length === 0}
              onRefreshCatalog={onRefreshCatalog}
              onShowAll={() => {
                setOnlySelected(false)
                onQueryChange('')
              }}
            />
          ) : null}
        </div>
      </SetupSection>
    </div>
  )
}

/**
 * Audit PN-17: an empty catalog and an empty search are different states.
 * The catalog case names the fix (refresh) instead of asking for another
 * search term.
 */
function CatalogEmptyState({
  query,
  onlySelected,
  catalogLoading,
  catalogEmpty,
  onRefreshCatalog,
  onShowAll,
}: {
  query: string
  onlySelected: boolean
  catalogLoading: boolean
  catalogEmpty: boolean
  onRefreshCatalog?: () => void
  onShowAll: () => void
}) {
  if (catalogEmpty) {
    return (
      <div
        className="grid justify-items-center gap-3 rounded-[6px] bg-[var(--surface-fill)] p-6 text-center text-xs text-ink-soft"
        data-catalog-empty
      >
        <p className="m-0">
          {catalogLoading
            ? 'Loading the test catalog…'
            : 'No tests loaded. Check that the Harness endpoint is reachable, then refresh the catalog.'}
        </p>
        {onRefreshCatalog && !catalogLoading ? (
          <button
            className={buttonClassName({
              variant: 'secondary',
              size: 'compact',
            })}
            type="button"
            onClick={onRefreshCatalog}
          >
            refresh catalog
          </button>
        ) : null}
      </div>
    )
  }
  return (
    <div className="grid justify-items-center gap-3 rounded-[6px] bg-[var(--surface-fill)] p-6 text-center text-xs text-ink-soft">
      <p className="m-0">
        {onlySelected && !query
          ? 'No tests selected yet.'
          : `No tests match “${query}”. Try another name or clear the search.`}
      </p>
      <button
        className={buttonClassName({ variant: 'secondary', size: 'compact' })}
        type="button"
        onClick={onShowAll}
      >
        show all tests
      </button>
    </div>
  )
}

/* ---------------------------------------------------------------- footer */

export type ExecutionSetupSummaryInput = {
  selectedScenarios: number
  runsPerScenario: number
  technicalRetries: number
}

/** Audit RS-07 / PN-20: the review is one sentence, not four tiles. */
export function executionSetupSummary({
  selectedScenarios,
  runsPerScenario,
  technicalRetries,
}: ExecutionSetupSummaryInput) {
  const runs = selectedScenarios * runsPerScenario
  const headline = [
    `${selectedScenarios} test${selectedScenarios === 1 ? '' : 's'}`,
    `${runs} run${runs === 1 ? '' : 's'}`,
  ].join(' · ')
  const detail = [
    `${runsPerScenario} run${runsPerScenario === 1 ? '' : 's'} per test`,
    `${technicalRetries} retr${technicalRetries === 1 ? 'y' : 'ies'}`,
  ].join(' · ')
  return { headline, detail }
}

/**
 * The fixed footer every setup host renders (a dialog footer slot or a
 * sticky bar on the page): the summary sentence, the pending or error line
 * announced politely, and the actions (audit RS-03 / PN-02 / RS-09).
 */
export function ExecutionSetupFooter({
  summary,
  pending = [],
  error = null,
  status = null,
  children,
}: {
  summary: ExecutionSetupSummaryInput
  pending?: string[]
  error?: string | null
  status?: string | null
  children: ReactNode
}) {
  const sentence = executionSetupSummary(summary)
  return (
    <div
      className="grid w-full gap-3 @[720px]:grid-cols-[minmax(0,1fr)_auto] @[720px]:items-center"
      data-execution-setup-footer
    >
      <div className="grid min-w-0 gap-1 font-mono text-xs">
        <span className="truncate text-ink" title={sentence.headline}>
          {sentence.headline}
        </span>
        {/* The detail line yields to the actions in narrow containers. */}
        <span
          className="hidden truncate text-label text-ink-muted @[560px]:block"
          title={sentence.detail}
        >
          {sentence.detail}
        </span>
        <p
          className={`m-0 font-sans text-xs leading-5 ${error ? 'text-danger' : 'text-ink-soft'}`}
          role={error ? 'alert' : 'status'}
          aria-live="polite"
        >
          {error
            ? error
            : pending.length > 0
              ? `Before saving: ${pending.join(' ')}`
              : (status ?? '')}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2 @[720px]:justify-end">
        {children}
      </div>
    </div>
  )
}
