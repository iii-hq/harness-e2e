import { ChevronDown, RefreshCw, Search, X } from 'lucide-react'
import { type ReactNode, useMemo, useState } from 'react'
import { ProviderModelDropdown } from '@/components/ProviderModelDropdown'
import {
  buttonClassName,
  Field,
  FilterChip,
  FilterChipGroup,
  fieldDescribedBy,
  Input,
  Textarea,
} from '@/design-system'
import '@/design-system/styles.css'

export type ExecutionSetupMode = 'quick' | 'plan'

export const QUICK_EXECUTION_INTENT_KEY = 'harness-e2e:quick-execution'
export const PLAN_SCOPE_INTENT_KEY = 'harness-e2e:plan-scope'

export function requestQuickExecution() {
  window.sessionStorage.setItem(QUICK_EXECUTION_INTENT_KEY, 'open')
}

export function consumeQuickExecutionRequest() {
  const requested =
    window.sessionStorage.getItem(QUICK_EXECUTION_INTENT_KEY) === 'open'
  if (requested) window.sessionStorage.removeItem(QUICK_EXECUTION_INTENT_KEY)
  return requested
}

/** Audit RS-13: a selection made in the run-suite dialog travels to plans/new. */
export function requestPlanFromSelection(scenarioIds: string[]) {
  window.sessionStorage.setItem(
    PLAN_SCOPE_INTENT_KEY,
    JSON.stringify(scenarioIds),
  )
}

export function consumePlanScopeRequest(): string[] {
  const raw = window.sessionStorage.getItem(PLAN_SCOPE_INTENT_KEY)
  if (!raw) return []
  window.sessionStorage.removeItem(PLAN_SCOPE_INTENT_KEY)
  try {
    const parsed = JSON.parse(raw)
    return Array.isArray(parsed)
      ? parsed.filter((item): item is string => typeof item === 'string')
      : []
  } catch {
    return []
  }
}

export type ExecutionModelGroup = {
  provider: string
  models: { label: string; value: string }[]
}

export type ExecutionSetupField = 'label' | 'subject' | 'scenarios' | 'url'
export type ExecutionSetupErrors = Partial<Record<ExecutionSetupField, string>>

/** Audit PN-05: validation runs on submit and names each pending item. */
export function validateExecutionSetup({
  mode,
  label,
  subject,
  selectedScenarios,
  url,
}: {
  mode: ExecutionSetupMode
  label: string
  subject: string
  selectedScenarios: string[]
  url: string
}): ExecutionSetupErrors {
  const errors: ExecutionSetupErrors = {}
  if (mode === 'plan' && label.trim() === '') errors.label = 'Add a plan label.'
  if (!subject) errors.subject = 'Choose an execution model.'
  if (selectedScenarios.length === 0)
    errors.scenarios = 'Select at least one test.'
  if (url.trim() === '') errors.url = 'The Harness endpoint is missing.'
  return errors
}

/** Moves focus to the first field the validation named (audit PN-05). */
export function focusFirstInvalid(
  idPrefix: string,
  errors: ExecutionSetupErrors,
) {
  const order: [ExecutionSetupField, string][] = [
    ['label', `${idPrefix}-label`],
    ['subject', `${idPrefix}-subject`],
    ['scenarios', `${idPrefix}-scenario-search`],
    ['url', `${idPrefix}-url`],
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
 * a group can be selected at once; local tests form their own group and
 * singletons gather under "other".
 */
export function groupScenarios(
  ids: string[],
  localIds: string[],
): ScenarioGroup[] {
  const local = new Set(localIds)
  const families = new Map<string, string[]>()
  const localItems: string[] = []
  for (const id of ids) {
    if (local.has(id)) {
      localItems.push(id)
      continue
    }
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
  if (localItems.length > 0)
    groups.unshift({ key: 'local', label: 'local', items: localItems })
  return groups
}

type ExecutionSetupProps = {
  idPrefix: string
  mode: ExecutionSetupMode
  label: string
  purpose?: string
  url: string
  subject: string
  judge: string
  modelGroups: ExecutionModelGroup[]
  availableScenarios: string[]
  localScenarioIds?: string[]
  scenarioTitles?: Record<string, string>
  /** Tests listed but not selectable, with the reason (audit PN-06). */
  unavailableScenarios?: { ids: string[]; reason: string }
  selectedScenarios: string[]
  query: string
  runs: string
  technicalRetries: string
  seed: string
  disabled?: boolean
  catalogLoading?: boolean
  catalogStatus: { tone: 'ready' | 'loading' | 'unavailable'; text: string }
  errors?: ExecutionSetupErrors
  /** Where the sticky search sits: at the top of a dialog body or below the page navigation. */
  stickyOffset?: 'page' | 'dialog'
  onRefreshCatalog?: () => void
  onLabelChange: (value: string) => void
  onPurposeChange?: (value: string) => void
  onUrlChange: (value: string) => void
  onSubjectChange: (value: string) => void
  onJudgeChange: (value: string) => void
  onSelectedScenariosChange: (value: string[]) => void
  onQueryChange: (value: string) => void
  onRunsChange: (value: string) => void
  onTechnicalRetriesChange: (value: string) => void
  onSeedChange: (value: string) => void
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
  mode,
  label,
  purpose = '',
  url,
  subject,
  judge,
  modelGroups,
  availableScenarios,
  localScenarioIds = [],
  scenarioTitles = {},
  unavailableScenarios,
  selectedScenarios,
  query,
  runs,
  technicalRetries,
  seed,
  disabled = false,
  catalogLoading = false,
  catalogStatus,
  errors = {},
  stickyOffset = 'page',
  onRefreshCatalog,
  onLabelChange,
  onPurposeChange,
  onUrlChange,
  onSubjectChange,
  onJudgeChange,
  onSelectedScenariosChange,
  onQueryChange,
  onRunsChange,
  onTechnicalRetriesChange,
  onSeedChange,
}: ExecutionSetupProps) {
  const [onlySelected, setOnlySelected] = useState(false)
  const normalizedQuery = query.trim().toLocaleLowerCase()
  const unavailable = new Set(unavailableScenarios?.ids ?? [])
  const allScenarios = useMemo(
    () => [
      ...availableScenarios,
      ...(unavailableScenarios?.ids ?? []).filter(
        (id) => !availableScenarios.includes(id),
      ),
    ],
    [availableScenarios, unavailableScenarios],
  )
  const matches = (scenario: string) =>
    (!normalizedQuery ||
      `${scenario} ${scenarioTitles[scenario] ?? scenarioDisplayName(scenario)}`
        .toLocaleLowerCase()
        .includes(normalizedQuery)) &&
    (!onlySelected || selectedScenarios.includes(scenario))
  const visibleScenarios = allScenarios.filter(matches)
  const groups = groupScenarios(visibleScenarios, [
    ...localScenarioIds,
    ...(unavailableScenarios?.ids ?? []),
  ])
  const runsPerScenario = Math.max(1, Number(runs) || 1)
  const retries = Math.max(0, Number(technicalRetries) || 0)
  const plannedRuns = selectedScenarios.length * runsPerScenario
  const selectable = (scenario: string) => !unavailable.has(scenario)

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
      ...scenarios.filter(
        (scenario) =>
          selectable(scenario) && !selectedScenarios.includes(scenario),
      ),
    ])
  }
  const deselectMany = (scenarios: string[]) => {
    onSelectedScenariosChange(
      selectedScenarios.filter((scenario) => !scenarios.includes(scenario)),
    )
  }

  const statusDot =
    catalogStatus.tone === 'ready'
      ? 'bg-success'
      : catalogStatus.tone === 'loading'
        ? 'bg-[var(--ink-decor)]'
        : 'bg-danger'
  const visibleSelectable = visibleScenarios.filter(selectable)
  const hiddenSelected = selectedScenarios.filter(
    (scenario) => !visibleScenarios.includes(scenario),
  ).length

  return (
    <div className="grid min-w-0 gap-8" data-execution-setup={mode}>
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
        title={mode === 'plan' ? 'Name the plan' : 'Name this run'}
        description={
          mode === 'plan'
            ? 'The label identifies this baseline and candidate workflow in the plans list.'
            : 'An optional label makes the result easier to find later.'
        }
      >
        <div className="grid items-start gap-4 sm:grid-cols-2">
          <Field
            label={mode === 'plan' ? 'Plan label' : 'Execution label'}
            htmlFor={`${idPrefix}-label`}
            meta={mode === 'plan' ? 'required' : 'optional'}
            error={errors.label}
          >
            <Input
              id={`${idPrefix}-label`}
              value={label}
              maxLength={120}
              placeholder={
                mode === 'plan'
                  ? 'Validate prompt routing change'
                  : 'Before system prompt change'
              }
              aria-invalid={errors.label ? true : undefined}
              aria-describedby={fieldDescribedBy(`${idPrefix}-label`, {
                error: Boolean(errors.label),
              })}
              onChange={(event) => onLabelChange(event.target.value)}
              disabled={disabled}
            />
          </Field>
          {mode === 'plan' && onPurposeChange ? (
            <Field
              label="Purpose"
              htmlFor={`${idPrefix}-purpose`}
              meta="optional"
            >
              <Textarea
                id={`${idPrefix}-purpose`}
                className="min-h-[4.5rem]"
                value={purpose}
                rows={2}
                placeholder="Describe the behavior or change under test"
                onChange={(event) => onPurposeChange(event.target.value)}
                disabled={disabled}
              />
            </Field>
          ) : null}
        </div>
      </SetupSection>

      <SetupSection
        id={`${idPrefix}-models`}
        title="Choose the model and judge"
        description="The model and judge are saved with the result."
      >
        <div className="grid items-start gap-4 sm:grid-cols-2">
          <Field
            label="Execution model"
            htmlFor={`${idPrefix}-subject`}
            meta="required"
            error={errors.subject}
          >
            <ProviderModelDropdown
              id={`${idPrefix}-subject`}
              ariaLabel="Execution model"
              required
              value={subject}
              onChange={onSubjectChange}
              disabled={disabled || modelGroups.length === 0}
              groups={modelGroups}
              placeholder={
                modelGroups.length === 0
                  ? 'No models in the catalog'
                  : 'Choose a model'
              }
            />
          </Field>
          <Field
            label="Judge model"
            htmlFor={`${idPrefix}-judge`}
            meta="optional"
          >
            <ProviderModelDropdown
              id={`${idPrefix}-judge`}
              ariaLabel="Judge model"
              value={judge}
              onChange={onJudgeChange}
              disabled={disabled || modelGroups.length === 0}
              groups={modelGroups}
              clearLabel="Default judge (automatic)"
              placeholder="Default judge (automatic)"
            />
          </Field>
        </div>
        {/* Audit PN-13 / PN-21: advanced controls with a real chevron; the
            endpoint is read-only here and editable inside. */}
        <details className="group min-w-0 rounded-[6px] bg-[var(--surface-fill)]">
          <summary className="flex min-h-9 min-w-0 cursor-pointer list-none items-center gap-3 px-3 text-xs marker:hidden">
            <ChevronDown
              className="size-4 shrink-0 -rotate-90 text-ink-muted transition-transform duration-[var(--ds-duration-fast)] group-open:rotate-0 motion-reduce:transition-none"
              aria-hidden="true"
            />
            <span className="font-semibold text-ink">
              Advanced · sampling, retries and seed
            </span>
            <span className="ml-auto hidden min-w-0 truncate font-mono text-label text-ink-muted @[560px]:block">
              {runsPerScenario} per test · {retries} retr
              {retries === 1 ? 'y' : 'ies'} ·{' '}
              {seed.trim() ? `seed ${seed.trim()}` : 'canonical seed'} ·{' '}
              {url || 'endpoint not loaded'}
            </span>
          </summary>
          <div className="grid gap-4 px-3 pt-1 pb-4 sm:grid-cols-3">
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
                onChange={(event) =>
                  onTechnicalRetriesChange(event.target.value)
                }
                onBlur={(event) =>
                  onTechnicalRetriesChange(
                    clampNumber(event.target.value, 0, 3),
                  )
                }
                disabled={disabled}
              />
            </Field>
            <Field
              label="Seed"
              htmlFor={`${idPrefix}-seed`}
              hint="Leave blank for the canonical case set."
            >
              <Input
                id={`${idPrefix}-seed`}
                className="font-mono"
                type="number"
                min="0"
                step="1"
                inputMode="numeric"
                value={seed}
                placeholder="canonical"
                onChange={(event) => onSeedChange(event.target.value)}
                disabled={disabled}
              />
            </Field>
            <Field
              label="Harness endpoint"
              htmlFor={`${idPrefix}-url`}
              className="sm:col-span-3"
              hint="Refresh the catalog after changing it."
              error={errors.url}
            >
              <Input
                id={`${idPrefix}-url`}
                className="font-mono text-xs"
                value={url}
                placeholder="ws://127.0.0.1:49134"
                aria-invalid={errors.url ? true : undefined}
                onChange={(event) => onUrlChange(event.target.value)}
                disabled={disabled}
              />
            </Field>
          </div>
        </details>
      </SetupSection>

      <SetupSection
        id={`${idPrefix}-scope`}
        title="Pick the tests"
        description={
          mode === 'plan'
            ? 'Choose the smallest useful set. The scope freezes when the baseline starts.'
            : 'Only the tests selected here run.'
        }
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
                className="pr-9 pl-9"
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
                onClick={() => selectMany(visibleSelectable)}
                disabled={disabled || visibleSelectable.length === 0}
              >
                select visible ({visibleSelectable.length})
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
                count={allScenarios.length}
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
              {visibleScenarios.length} of {allScenarios.length} shown ·{' '}
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
            const groupSelectable = group.items.filter(selectable)
            const allSelected =
              groupSelectable.length > 0 &&
              groupSelectable.every((scenario) =>
                selectedScenarios.includes(scenario),
              )
            return (
              <div key={group.key} data-scenario-group={group.key}>
                <div className="flex items-center justify-between gap-3 border-b border-line pb-1">
                  <span className="ds-label">
                    {group.label} · {group.items.length}
                  </span>
                  {groupSelectable.length > 0 ? (
                    <button
                      className={buttonClassName({
                        variant: 'quiet',
                        size: 'compact',
                      })}
                      type="button"
                      onClick={() =>
                        allSelected
                          ? deselectMany(groupSelectable)
                          : selectMany(groupSelectable)
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
                    const local = localScenarioIds.includes(scenario)
                    const blocked = unavailable.has(scenario)
                    const title = scenarioTitles[scenario]
                    return (
                      <li key={scenario}>
                        <label
                          className={`flex min-h-9 min-w-0 items-center gap-3 rounded-[6px] px-2 text-xs ${
                            blocked
                              ? 'cursor-not-allowed text-ink-muted'
                              : selected
                                ? 'cursor-pointer bg-[var(--surface-selected)] text-ink'
                                : 'cursor-pointer text-ink hover:bg-[var(--surface-fill)]'
                          }`}
                          title={
                            blocked ? unavailableScenarios?.reason : undefined
                          }
                        >
                          <input
                            className="size-4 shrink-0 accent-[var(--accent)]"
                            type="checkbox"
                            checked={selected}
                            disabled={disabled || blocked}
                            onChange={(event) =>
                              toggleScenario(scenario, event.target.checked)
                            }
                          />
                          <span className="min-w-0 flex-1 truncate font-mono">
                            {title ?? scenario}
                            {title ? (
                              <span className="ml-2 text-label text-ink-muted">
                                {scenario}
                              </span>
                            ) : null}
                          </span>
                          {local ? (
                            <span className="rounded-[6px] bg-[var(--surface-fill)] px-1.5 py-0.5 font-mono text-label text-ink-soft">
                              local
                            </span>
                          ) : null}
                          {blocked ? (
                            <span className="font-mono text-label text-ink-muted">
                              not available in plans
                            </span>
                          ) : null}
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
              catalogEmpty={allScenarios.length === 0}
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
  mode: ExecutionSetupMode
  selectedScenarios: number
  runsPerScenario: number
  technicalRetries: number
  seed: string
  subject: string
  judge: string
  url: string
}

/** Audit RS-07 / PN-20: the review is one sentence, not four tiles. */
export function executionSetupSummary({
  selectedScenarios,
  runsPerScenario,
  technicalRetries,
  seed,
  subject,
  judge,
  url,
}: ExecutionSetupSummaryInput) {
  const runs = selectedScenarios * runsPerScenario
  const headline = [
    `${selectedScenarios} test${selectedScenarios === 1 ? '' : 's'}`,
    `${runs} run${runs === 1 ? '' : 's'}`,
    subject || 'no model',
    judge ? `judge ${judge}` : 'default judge',
  ].join(' · ')
  const detail = [
    `${runsPerScenario} run${runsPerScenario === 1 ? '' : 's'} per test`,
    `${technicalRetries} retr${technicalRetries === 1 ? 'y' : 'ies'}`,
    seed.trim() ? `seed ${seed.trim()}` : 'canonical seed',
    url || 'endpoint not loaded',
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
              ? `Before ${summary.mode === 'plan' ? 'creating' : 'running'}: ${pending.join(' ')}`
              : (status ?? '')}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2 @[720px]:justify-end">
        {children}
      </div>
    </div>
  )
}
