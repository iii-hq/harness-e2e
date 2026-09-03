import { RefreshCw, Search } from 'lucide-react'
import type { ReactNode } from 'react'
import { ProviderModelDropdown } from '@/components/ProviderModelDropdown'
import { buttonClassName } from '@/design-system'
import '@/design-system/styles.css'

export type ExecutionSetupMode = 'quick' | 'plan'

export const QUICK_EXECUTION_INTENT_KEY = 'harness-e2e:quick-execution'

export function requestQuickExecution() {
  window.sessionStorage.setItem(QUICK_EXECUTION_INTENT_KEY, 'open')
}

export function consumeQuickExecutionRequest() {
  const requested =
    window.sessionStorage.getItem(QUICK_EXECUTION_INTENT_KEY) === 'open'
  if (requested) window.sessionStorage.removeItem(QUICK_EXECUTION_INTENT_KEY)
  return requested
}

export type ExecutionModelGroup = {
  provider: string
  models: { label: string; value: string }[]
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
  selectedScenarios: string[]
  query: string
  runs: string
  technicalRetries: string
  seed: string
  disabled?: boolean
  catalogLoading?: boolean
  catalogSummary: string
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

type ExecutionSetupReviewProps = {
  mode: ExecutionSetupMode
  status: string
  subject: string
  judge: string
  url: string
  selectedScenarios: number
  plannedRuns: number
  runsPerScenario: number
  technicalRetries: number
  ready: boolean
  /**
   * Audit RS-02: inside a dialog the review column sticks to the dialog's
   * own scroller, so the page header offset must not apply.
   */
  stickyOffset?: 'page' | 'dialog'
  children: ReactNode
}

const fieldLabel =
  'grid min-w-0 gap-2 text-xs font-semibold text-[var(--color-ink-faint)]'
const fieldControl =
  'min-h-11 w-full rounded-[6px] border border-[var(--color-rule)] bg-panel-raised px-3 text-sm text-ink transition-colors duration-[var(--ds-duration-fast)] placeholder:text-ink-muted hover:border-[var(--color-edge)] focus-visible:border-[var(--color-rule-focus)] focus-visible:[outline:2px_solid_var(--color-rule-focus)] focus-visible:[outline-offset:3px] disabled:cursor-not-allowed disabled:opacity-50'
const quietButton = buttonClassName({ variant: 'secondary', size: 'compact' })

function retryCount(value: string) {
  return Math.max(0, Number(value) || 0)
}

/**
 * Audit PN-17: an empty catalog and an empty search are different states.
 * The catalog case names the fix (refresh) instead of asking for another
 * search term.
 */
function CatalogEmptyState({
  query,
  catalogLoading,
  catalogEmpty,
  onRefreshCatalog,
}: {
  query: string
  catalogLoading: boolean
  catalogEmpty: boolean
  onRefreshCatalog?: () => void
}) {
  if (catalogEmpty) {
    return (
      <div
        className="col-span-full grid justify-items-center gap-3 p-6 text-center text-xs text-ink-muted"
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
    <p className="col-span-full m-0 p-6 text-center text-xs text-ink-muted">
      No tests match “{query}”. Try another name or clear the search.
    </p>
  )
}

export function scenarioDisplayName(scenario: string) {
  return scenario
    .replace(/[_.]+/g, ' ')
    .replace(/\b\w/g, (character) => character.toUpperCase())
}

// Audit RS-04: the sections are not a sequence, so they carry no numerals.
function SetupSection({
  title,
  description,
  children,
}: {
  title: string
  description: string
  children: ReactNode
}) {
  return (
    <section className="border-b border-[var(--color-rule)] p-5 last:border-b-0 sm:p-6">
      <div className="min-w-0">
        <h2 className="m-0 text-sm font-semibold tracking-[-0.015em] text-ink">
          {title}
        </h2>
        <p className="mt-1 mb-0 max-w-2xl text-xs leading-5 text-ink-muted">
          {description}
        </p>
      </div>
      <div className="mt-5">{children}</div>
    </section>
  )
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
  selectedScenarios,
  query,
  runs,
  technicalRetries,
  seed,
  disabled = false,
  catalogLoading = false,
  catalogSummary,
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
  const normalizedQuery = query.trim().toLocaleLowerCase()
  const visibleScenarios = normalizedQuery
    ? availableScenarios.filter((scenario) =>
        `${scenario} ${scenarioTitles[scenario] ?? scenarioDisplayName(scenario)}`
          .toLocaleLowerCase()
          .includes(normalizedQuery),
      )
    : availableScenarios
  const runsPerScenario = Math.max(1, Number(runs) || 1)
  const plannedRuns = selectedScenarios.length * runsPerScenario
  const labelRequired = mode === 'plan'

  const toggleScenario = (scenario: string, checked: boolean) => {
    onSelectedScenariosChange(
      checked
        ? selectedScenarios.includes(scenario)
          ? selectedScenarios
          : [...selectedScenarios, scenario]
        : selectedScenarios.filter((item) => item !== scenario),
    )
  }

  const selectVisible = () => {
    onSelectedScenariosChange([
      ...selectedScenarios,
      ...visibleScenarios.filter(
        (scenario) => !selectedScenarios.includes(scenario),
      ),
    ])
  }

  return (
    <div className="min-w-0 bg-panel">
      {/* Audit RS-03: the status bar stacks below sm instead of squeezing the
          summary into a 60px column next to the endpoint and the button. */}
      <div
        className="grid gap-2 border-b border-[var(--color-rule)] bg-panel-raised px-5 py-3 sm:flex sm:flex-wrap sm:items-center sm:gap-3 sm:px-6"
        aria-live="polite"
      >
        <span className="flex min-w-0 items-center gap-2 sm:flex-1">
          <span
            className={`h-2 w-2 shrink-0 rounded-full ${availableScenarios.length > 0 ? 'bg-[var(--color-ok)]' : 'bg-[var(--color-warn)]'}`}
            aria-hidden="true"
          />
          <span className="min-w-0 text-xs text-[var(--color-ink-faint)]">
            {catalogSummary}
          </span>
        </span>
        <code className="max-w-full truncate font-mono text-label text-ink-muted sm:max-w-[16rem]">
          {url || 'Endpoint not loaded'}
        </code>
        {onRefreshCatalog && (
          <button
            className={quietButton}
            type="button"
            onClick={onRefreshCatalog}
            disabled={disabled || catalogLoading}
          >
            <RefreshCw
              className={catalogLoading ? 'animate-spin' : ''}
              size={14}
              aria-hidden="true"
            />
            {catalogLoading ? 'refreshing' : 'refresh'}
          </button>
        )}
      </div>

      <SetupSection
        title={mode === 'plan' ? 'Name the plan' : 'Name this run'}
        description={
          mode === 'plan'
            ? 'The label identifies this baseline and candidate workflow in the plans list.'
            : 'An optional label makes the result easier to find later.'
        }
      >
        <div className="grid gap-4 sm:grid-cols-2">
          <label className={fieldLabel} htmlFor={`${idPrefix}-label`}>
            <span className="flex items-baseline justify-between gap-2">
              {mode === 'plan' ? 'Plan label' : 'Execution label'}
              <small className="text-xs font-normal text-ink-muted">
                {labelRequired ? 'required' : 'optional'}
              </small>
            </span>
            <input
              id={`${idPrefix}-label`}
              className={fieldControl}
              value={label}
              required={labelRequired}
              maxLength={120}
              placeholder={
                mode === 'plan'
                  ? 'Validate prompt routing change'
                  : 'Before system prompt change'
              }
              onChange={(event) => onLabelChange(event.target.value)}
              disabled={disabled}
            />
          </label>
          {mode === 'plan' && onPurposeChange && (
            <label className={fieldLabel} htmlFor={`${idPrefix}-purpose`}>
              <span className="flex items-baseline justify-between gap-2">
                Purpose
                <small className="text-xs font-normal text-ink-muted">
                  optional
                </small>
              </span>
              <textarea
                id={`${idPrefix}-purpose`}
                className={`${fieldControl} min-h-[4.75rem] resize-y py-3`}
                value={purpose}
                rows={2}
                placeholder="Describe the behavior or change under test"
                onChange={(event) => onPurposeChange(event.target.value)}
                disabled={disabled}
              />
            </label>
          )}
        </div>
      </SetupSection>

      <SetupSection
        title="Choose the model and judge"
        description="The endpoint, execution model and judge are saved with the result."
      >
        <div className="grid gap-4 sm:grid-cols-2">
          <label
            className={`${fieldLabel} sm:col-span-2`}
            htmlFor={`${idPrefix}-url`}
          >
            Harness endpoint
            <input
              id={`${idPrefix}-url`}
              className={`${fieldControl} font-mono text-xs`}
              value={url}
              required
              placeholder="ws://127.0.0.1:49134"
              onChange={(event) => onUrlChange(event.target.value)}
              disabled={disabled}
            />
          </label>
          <div className={fieldLabel}>
            <span
              className="flex items-baseline justify-between gap-2"
              id={`${idPrefix}-subject-label`}
            >
              Execution model
              <small className="text-xs font-normal text-ink-muted">
                required
              </small>
            </span>
            <ProviderModelDropdown
              ariaLabel="Execution model"
              labelledBy={`${idPrefix}-subject-label`}
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
          </div>
          <div className={fieldLabel}>
            <span
              className="flex items-baseline justify-between gap-2"
              id={`${idPrefix}-judge-label`}
            >
              Judge model
              <small className="text-xs font-normal text-ink-muted">
                optional
              </small>
            </span>
            <ProviderModelDropdown
              ariaLabel="Judge model"
              labelledBy={`${idPrefix}-judge-label`}
              value={judge}
              onChange={onJudgeChange}
              disabled={disabled || modelGroups.length === 0}
              groups={modelGroups}
              clearLabel="Default judge (automatic)"
              placeholder="Default judge (automatic)"
            />
          </div>
        </div>
      </SetupSection>

      <SetupSection
        title="Pick the tests"
        description={
          mode === 'plan'
            ? 'Choose the smallest useful set. The scope freezes when the baseline starts.'
            : 'Only the tests selected here run.'
        }
      >
        <div className="grid gap-4">
          <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-end">
            <label
              className={fieldLabel}
              htmlFor={`${idPrefix}-scenario-search`}
            >
              Find a test
              <span className="relative">
                <Search
                  className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-ink-muted"
                  size={15}
                  aria-hidden="true"
                />
                <input
                  id={`${idPrefix}-scenario-search`}
                  className={`${fieldControl} pl-9`}
                  type="search"
                  value={query}
                  placeholder="Search by name or id"
                  onChange={(event) => onQueryChange(event.target.value)}
                  disabled={disabled}
                />
              </span>
            </label>
            <div className="flex flex-wrap gap-2">
              <button
                className={quietButton}
                type="button"
                onClick={selectVisible}
                disabled={disabled || visibleScenarios.length === 0}
              >
                select visible ({visibleScenarios.length})
              </button>
              <button
                className={quietButton}
                type="button"
                onClick={() => onSelectedScenariosChange([])}
                disabled={disabled || selectedScenarios.length === 0}
              >
                clear
              </button>
            </div>
          </div>

          <output className="flex min-h-10 flex-wrap items-center justify-between gap-2 border-y border-[var(--color-rule)] py-2 font-mono text-label text-ink-muted">
            <span>
              <strong className="text-ink">{selectedScenarios.length}</strong>{' '}
              {selectedScenarios.length === 1
                ? 'test selected'
                : 'tests selected'}
            </span>
            <span>
              {plannedRuns} {plannedRuns === 1 ? 'run' : 'runs'} in total
            </span>
          </output>

          <div className="grid max-h-[25rem] grid-cols-1 gap-2 overflow-auto rounded-[6px] border border-[var(--color-rule)] bg-panel-raised p-2 sm:grid-cols-2">
            {visibleScenarios.map((scenario) => {
              const selected = selectedScenarios.includes(scenario)
              const local = localScenarioIds.includes(scenario)
              return (
                <label
                  className={`flex min-h-16 min-w-0 cursor-pointer items-start gap-3 rounded-[6px] border p-3 transition-colors duration-[var(--ds-duration-fast)] ${
                    selected
                      ? 'border-[color-mix(in_srgb,var(--color-accent)_60%,var(--color-rule))] bg-[color-mix(in_srgb,var(--color-accent)_7%,var(--surface))]'
                      : 'border-[var(--color-rule)] bg-panel hover:border-[var(--color-edge)]'
                  }`}
                  key={scenario}
                >
                  <input
                    className="mt-0.5 h-4 w-4 shrink-0 accent-[var(--color-accent)]"
                    type="checkbox"
                    checked={selected}
                    disabled={disabled}
                    onChange={(event) =>
                      toggleScenario(scenario, event.target.checked)
                    }
                  />
                  <span className="grid min-w-0 gap-1">
                    <span className="flex flex-wrap items-center gap-2">
                      <strong className="text-xs font-semibold leading-4 text-ink">
                        {scenarioTitles[scenario] ??
                          scenarioDisplayName(scenario)}
                      </strong>
                      {local && (
                        <span className="rounded-full border border-[color-mix(in_srgb,var(--color-accent)_35%,var(--color-rule))] px-2 py-0.5 font-mono text-label font-semibold uppercase tracking-[0.05em] text-[var(--color-accent)]">
                          Local
                        </span>
                      )}
                    </span>
                    <code className="break-all font-mono text-label leading-4 text-ink-muted">
                      {scenario}
                    </code>
                  </span>
                </label>
              )
            })}
            {visibleScenarios.length === 0 && (
              <CatalogEmptyState
                query={query}
                catalogLoading={catalogLoading}
                catalogEmpty={availableScenarios.length === 0}
                onRefreshCatalog={onRefreshCatalog}
              />
            )}
          </div>
        </div>
      </SetupSection>

      <details className="group bg-[color-mix(in_srgb,var(--color-panel-raised)_60%,transparent)]">
        <summary className="flex min-h-20 cursor-pointer list-none items-center justify-between gap-4 px-5 py-4 marker:content-none sm:px-6 [&::-webkit-details-marker]:hidden">
          <span className="grid min-w-0 gap-1">
            <span className="font-mono text-label font-semibold uppercase tracking-[0.06em] text-[var(--color-accent)]">
              Optional controls
            </span>
            <strong className="text-sm font-semibold text-ink">
              Sampling, retries and seed
            </strong>
            <small className="text-xs leading-4 text-ink-muted">
              Retries only recover crashes. They never add samples.
            </small>
          </span>
          <span className="shrink-0 font-mono text-xs text-[var(--color-ink-faint)]">
            {runsPerScenario} per test · {retryCount(technicalRetries)}{' '}
            {retryCount(technicalRetries) === 1 ? 'retry' : 'retries'}
          </span>
        </summary>
        <div className="grid gap-px border-t border-[var(--color-rule)] bg-[var(--color-rule)] sm:grid-cols-3">
          <label className={`${fieldLabel} bg-panel p-5`}>
            Runs per test
            <span className="text-xs font-normal leading-4 text-ink-muted">
              Each test runs this many times. More runs make comparisons more
              reliable.
            </span>
            <input
              className={`${fieldControl} font-mono`}
              type="number"
              min="1"
              max="20"
              value={runs}
              onChange={(event) => onRunsChange(event.target.value)}
              disabled={disabled}
            />
          </label>
          <label className={`${fieldLabel} bg-panel p-5`}>
            Technical retries
            <span className="text-xs font-normal leading-4 text-ink-muted">
              Reruns a test after a crash. Does not add a sample.
            </span>
            <input
              className={`${fieldControl} font-mono`}
              type="number"
              min="0"
              max="3"
              value={technicalRetries}
              onChange={(event) => onTechnicalRetriesChange(event.target.value)}
              disabled={disabled}
            />
          </label>
          <label className={`${fieldLabel} bg-panel p-5`}>
            Seed
            <span className="text-xs font-normal leading-4 text-ink-muted">
              Leave blank for the canonical case set.
            </span>
            <input
              className={`${fieldControl} font-mono`}
              type="number"
              min="0"
              step="1"
              value={seed}
              placeholder="Canonical"
              onChange={(event) => onSeedChange(event.target.value)}
              disabled={disabled}
            />
          </label>
        </div>
      </details>
    </div>
  )
}

export function ExecutionSetupReview({
  mode,
  status,
  subject,
  judge,
  url,
  selectedScenarios,
  plannedRuns,
  runsPerScenario,
  technicalRetries,
  ready,
  stickyOffset = 'page',
  children,
}: ExecutionSetupReviewProps) {
  const facts = [
    { label: 'Tests', value: String(selectedScenarios) },
    { label: 'Runs', value: String(plannedRuns) },
    { label: 'Runs per test', value: String(runsPerScenario) },
    { label: 'Retries', value: String(technicalRetries) },
  ]

  return (
    <aside
      className={`grid content-start bg-panel-raised lg:sticky ${
        stickyOffset === 'dialog'
          ? 'lg:top-0'
          : 'lg:top-20 lg:max-h-[calc(100dvh-6rem)] lg:overflow-auto'
      }`}
    >
      <div className="border-b border-[var(--color-rule)] p-5 sm:p-6">
        <p className="m-0 font-mono text-label font-semibold uppercase tracking-[0.07em] text-[var(--color-accent)]">
          {mode === 'plan' ? 'Reusable workflow' : 'One-off result'}
        </p>
        <div className="mt-3 flex items-start justify-between gap-3">
          <div>
            <h2 className="m-0 text-base font-semibold text-ink">
              Review setup
            </h2>
            <p className="mt-1 mb-0 text-xs leading-5 text-ink-muted">
              {mode === 'plan'
                ? 'Creates a draft. The scope remains editable until the baseline starts.'
                : 'Starts immediately and saves an independent execution result.'}
            </p>
          </div>
          <span
            className={`inline-flex min-h-7 shrink-0 items-center rounded-full border px-2.5 font-mono text-label font-semibold ${
              ready
                ? 'border-[color-mix(in_srgb,var(--color-ok)_35%,transparent)] text-[var(--color-ok)]'
                : 'border-[var(--color-edge)] text-ink-muted'
            }`}
          >
            {status}
          </span>
        </div>
      </div>

      <dl className="grid grid-cols-2 gap-px bg-[var(--color-rule)]">
        {facts.map((fact) => (
          <div
            className="grid min-h-24 content-between gap-3 bg-panel p-4"
            key={fact.label}
          >
            <dt className="font-mono text-label uppercase tracking-[0.05em] text-ink-muted">
              {fact.label}
            </dt>
            <dd className="m-0 font-mono text-xl font-semibold tracking-[-0.04em] text-ink">
              {fact.value}
            </dd>
          </div>
        ))}
      </dl>

      <dl className="grid gap-4 border-t border-[var(--color-rule)] p-5 text-xs sm:p-6">
        <div className="grid gap-1">
          <dt className="font-mono text-label uppercase tracking-[0.05em] text-ink-muted">
            Execution model
          </dt>
          <dd className="m-0 break-all text-ink">
            {subject || 'Not selected'}
          </dd>
        </div>
        <div className="grid gap-1">
          <dt className="font-mono text-label uppercase tracking-[0.05em] text-ink-muted">
            Judge
          </dt>
          <dd className="m-0 break-all text-[var(--color-ink-faint)]">
            {judge || 'Default judge'}
          </dd>
        </div>
        <div className="grid gap-1">
          <dt className="font-mono text-label uppercase tracking-[0.05em] text-ink-muted">
            Harness endpoint
          </dt>
          <dd className="m-0 break-all font-mono text-label text-[var(--color-ink-faint)]">
            {url || 'Not loaded'}
          </dd>
        </div>
      </dl>

      <div className="grid gap-3 border-t border-[var(--color-rule)] p-5 sm:p-6">
        {children}
      </div>
    </aside>
  )
}
