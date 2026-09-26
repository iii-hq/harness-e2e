import {
  AlertCircle,
  Check,
  Link2,
  Minus,
  RefreshCw,
  Search,
  X,
} from 'lucide-react'
import {
  type CheckState,
  checkState,
  plural,
  selectionText,
  sequenceStep,
  testFamilies,
  toggleAll,
  visibleTests,
} from './run-dialog-model'

export type CatalogStatus = 'loading' | 'failed' | 'ready'

const SKELETON = [180, 140, 210, 160, 120, 190, 150, 200, 130, 170, 110, 185]

function Box({
  state,
  label,
  disabled,
  onToggle,
}: {
  state: CheckState
  label?: string
  disabled?: boolean
  onToggle: () => void
}) {
  return (
    <span className="rd-box">
      <input
        type="checkbox"
        className="rd-box-input"
        aria-label={label}
        checked={state === 'on'}
        disabled={disabled}
        ref={(element) => {
          if (element) element.indeterminate = state === 'some'
        }}
        onChange={onToggle}
      />
      <span className="rd-box-face" data-state={state} aria-hidden="true">
        {state === 'on' ? <Check size={12} strokeWidth={3} /> : null}
        {state === 'some' ? <Minus size={12} strokeWidth={3} /> : null}
      </span>
    </span>
  )
}

/** The catalog's tests in families, with a filter, All/Selected, a
 *  select-all of what is shown and a Clear. */
export function TestsColumn({
  status,
  tests,
  sequences,
  selected,
  onSelect,
  query,
  onQuery,
  onlySelected,
  onOnlySelected,
  modelCount,
  onRefresh,
}: {
  status: CatalogStatus
  tests: string[]
  sequences: string[][]
  selected: string[]
  /** The next selection; the dialog completes sequences. */
  onSelect: (next: string[]) => void
  query: string
  onQuery: (value: string) => void
  onlySelected: boolean
  onOnlySelected: (value: boolean) => void
  modelCount: number
  onRefresh: () => void
}) {
  const ready = status === 'ready'
  const visible = ready
    ? visibleTests(tests, selected, query, onlySelected)
    : []
  const shown = checkState(visible, selected)
  const hidden = selected.filter((id) => !visible.includes(id)).length
  const families = testFamilies(tests)
    .map((family) => ({
      ...family,
      visible: family.items.filter((id) => visible.includes(id)),
      ticked: family.items.filter((id) => selected.includes(id)).length,
    }))
    .filter((family) => family.visible.length > 0)
  const nothingTicked = onlySelected && !query.trim()
  const shownText = !ready
    ? ''
    : visible.length === tests.length
      ? String(tests.length)
      : `${visible.length} of ${tests.length}`

  return (
    <section className="rd-tests" aria-label="Tests">
      <div className="rd-tests-toolbar">
        <div className="rd-search">
          <Search size={16} aria-hidden="true" className="rd-search-icon" />
          <input
            type="text"
            className="rd-control rd-input rd-search-input"
            aria-label="Filter tests"
            placeholder="Filter by name or id"
            value={query}
            disabled={!ready}
            onChange={(event) => onQuery(event.target.value)}
          />
          {query ? (
            <button
              type="button"
              className="rd-ghost rd-search-clear"
              aria-label="Clear filter"
              onClick={() => onQuery('')}
            >
              <X size={16} aria-hidden="true" />
            </button>
          ) : null}
        </div>
        {/* biome-ignore lint/a11y/useSemanticElements: a toggle group, not a fieldset */}
        <div role="group" aria-label="Show" className="rd-control rd-segments">
          <button
            type="button"
            className="rd-segment"
            aria-pressed={!onlySelected}
            disabled={!ready}
            onClick={() => onOnlySelected(false)}
          >
            All <span className="rd-meta">{ready ? tests.length : '–'}</span>
          </button>
          <button
            type="button"
            className="rd-segment"
            aria-pressed={onlySelected}
            disabled={!ready}
            onClick={() => onOnlySelected(true)}
          >
            Selected <span className="rd-meta">{selected.length}</span>
          </button>
        </div>
      </div>

      <div className="rd-tests-head">
        {/* biome-ignore lint/a11y/noLabelWithoutControl: the checkbox is inside Box */}
        <label className="rd-tests-all">
          <Box
            state={shown}
            label="Select every test shown"
            disabled={!ready || visible.length === 0}
            onToggle={() => onSelect(toggleAll(visible, selected))}
          />
          <span className="rd-strong">Tests</span>
          <span className="rd-meta">{shownText}</span>
        </label>
        <span className="rd-meta rd-push">
          {selectionText(selected.length, hidden)}
        </span>
        <button
          type="button"
          className="rd-ghost rd-small"
          disabled={selected.length === 0}
          onClick={() => onSelect([])}
        >
          Clear
        </button>
      </div>

      <div className="rd-tests-list rd-scroll">
        {ready && families.length > 0
          ? families.map((family) => {
              const state = checkState(family.visible, selected)
              return (
                // biome-ignore lint/a11y/useSemanticElements: a group of checkboxes with its own select-all
                <div
                  key={family.key}
                  role="group"
                  aria-label={family.label}
                  className="rd-family"
                >
                  <div className="rd-family-head">
                    {/* biome-ignore lint/a11y/noLabelWithoutControl: the checkbox is inside Box */}
                    <label className="rd-row-label">
                      <Box
                        state={state}
                        label={`Select every test in ${family.label}`}
                        onToggle={() =>
                          onSelect(toggleAll(family.visible, selected))
                        }
                      />
                      <span
                        className="rd-family-name"
                        data-mono={family.mono || undefined}
                      >
                        {family.label}
                      </span>
                    </label>
                    <span className="rd-count">
                      {family.ticked
                        ? `${family.ticked}/${family.items.length}`
                        : family.items.length}
                    </span>
                  </div>
                  {family.visible.map((id) => {
                    const on = selected.includes(id)
                    const step = sequenceStep(id, sequences)
                    return (
                      // biome-ignore lint/a11y/noLabelWithoutControl: the checkbox is inside Box
                      <label key={id} className="rd-test">
                        <Box
                          state={on ? 'on' : 'off'}
                          label={id}
                          onToggle={() =>
                            onSelect(
                              on
                                ? selected.filter((item) => item !== id)
                                : [...selected, id],
                            )
                          }
                        />
                        <span className="rd-test-id rd-ellipsis" title={id}>
                          {id}
                        </span>
                        {step ? (
                          <span className="rd-step">
                            <Link2 size={16} aria-hidden="true" />
                            {step}
                          </span>
                        ) : null}
                      </label>
                    )
                  })}
                </div>
              )
            })
          : null}

        {ready && families.length === 0 ? (
          <div className="rd-empty">
            <Search size={16} aria-hidden="true" className="rd-faint" />
            <p className="rd-strong">
              {nothingTicked
                ? 'No tests ticked yet.'
                : `No tests match “${query}”.`}
            </p>
            <button
              type="button"
              className="rd-control rd-button"
              onClick={() => {
                onQuery('')
                onOnlySelected(false)
              }}
            >
              {nothingTicked ? 'Show all tests' : 'Clear filter'}
            </button>
          </div>
        ) : null}

        {status === 'loading' ? (
          <div
            className="rd-skeleton"
            role="status"
            aria-busy="true"
            aria-label="Loading tests"
          >
            {SKELETON.map((width, index) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: fixed placeholder rows
              <div key={index} className="rd-skeleton-row">
                <span className="rd-skel rd-skel-box" />
                <span className="rd-skel rd-skel-line" style={{ width }} />
              </div>
            ))}
          </div>
        ) : null}

        {status === 'failed' ? (
          <div role="alert" className="rd-alert" data-tone="alert">
            <AlertCircle
              size={16}
              aria-hidden="true"
              className="rd-alert-icon"
            />
            <div className="rd-grow">
              <p className="rd-strong">Couldn’t load the test catalog</p>
              <p className="rd-faint">
                The harness worker did not answer. Check that it is running on
                this stack, then retry.
              </p>
            </div>
            <button
              type="button"
              className="rd-ghost rd-button"
              onClick={onRefresh}
            >
              <RefreshCw size={16} aria-hidden="true" />
              Retry
            </button>
          </div>
        ) : null}
      </div>

      <div className="rd-catalog">
        <span className="rd-dot" data-status={status} aria-hidden="true" />
        <span role="status" className="rd-ellipsis rd-grow">
          {status === 'loading'
            ? 'Loading catalog…'
            : status === 'failed'
              ? 'Catalog unavailable'
              : `Catalog ready · ${plural(tests.length, 'test', 'tests')} · ${plural(modelCount, 'model', 'models')}`}
        </span>
        <button
          type="button"
          className="rd-ghost rd-icon-button"
          aria-label="Refresh catalog"
          disabled={status === 'loading'}
          onClick={onRefresh}
        >
          <RefreshCw size={16} aria-hidden="true" />
        </button>
      </div>
    </section>
  )
}
