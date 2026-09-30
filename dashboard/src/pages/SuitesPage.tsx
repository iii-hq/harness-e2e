import {
  Button,
  Checkbox,
  ConfirmDialog,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@iii-dev/console-ui'
import { ChevronLeft, Copy, Info, Minus, Plus, Trash2 } from 'lucide-react'
import {
  type MouseEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
  type HeaderAction,
} from '@/components/DashboardPageActions'
import { useDashboardChrome } from '@/components/DashboardShell'
import { asCatalog, LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import {
  buttonClassName,
  Callout,
  EmptyState,
  FactChip,
  FactList,
  Input,
  StatusLabel,
} from '@/design-system'
import {
  hashForExecution,
  hashForSuites,
  hashForTestHistory,
  hashForTests,
  routeParams,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import { useRunChanges } from '@/hooks/use-run-changes'
import {
  type DashboardDataBridge,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
  type Suite,
} from '@/lib/dashboard-data-source'
import {
  changedTests,
  changedWarning,
  draftChanges,
  draftDirty,
  draftProblem,
  EXECUTIONS_READ,
  type ExecutionScope,
  executionScope,
  notRunNote,
  openSuite,
  type SuiteDraft,
  type SuiteListItem,
  type SuiteRun,
  sequenceSteps,
  suiteDigest,
  suiteDraft,
  suiteHolds,
  suiteListItem,
  suiteRuns,
  suitesSummary,
  testSuggestions,
  testsNote,
  tickDraft,
  upsertSuite,
} from '@/lib/suites-view'
import type { TestCatalogRow } from '@/lib/test-catalog'
import { type CatalogRowView, catalogRowView } from '@/lib/test-catalog-view'
import { LastResult, listAllTests, Sparkline } from '@/pages/TestsCatalogPage'
import '@/design-system/styles.css'
import './executions-page.css'
import './tests-catalog.css'
import './suites-page.css'

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** Drafts by suite id. They outlive the page: a navigation it cannot hold
 *  back (the browser's Back, a pasted hash, another section) keeps an edit,
 *  and the suite reopens in it with its unsaved changes. */
const openDrafts = new Map<string, SuiteDraft>()

/** Forgets a draft that holds nothing unsaved (or whose suite is gone). */
function dropCleanDraft(suiteId: string, suites: Suite[] | null) {
  const draft = openDrafts.get(suiteId)
  const suite = suites?.find((entry) => entry.id === suiteId)
  if (draft && (!suite || !draftDirty(draft, suite))) openDrafts.delete(suiteId)
}

/** A link click the page may hold back: `onLeave` calls preventDefault on
 *  it to ask first. */
export type LeaveHandler = (event: MouseEvent, target: string) => void

/** The suite the hash opens: `#/suites?suite=<id>`. */
function suiteParam() {
  return typeof window === 'undefined'
    ? null
    : routeParams(window.location.hash).get('suite')
}

/** The section's actions in the Console header: New suite, which is made
 *  by ticking tests in the catalog and saving them, and, once the worker
 *  answers, Run tests. With unsaved changes open, New suite asks first
 *  (`onNew`) instead of leaving. */
export function suitesHeaderActions(
  onRun?: () => void,
  onNew?: () => void,
): HeaderAction[] {
  return [
    {
      id: 'new',
      label: 'New suite',
      ...(onNew ? { onSelect: onNew } : { href: hashForTests() }),
      title: 'Tick tests in the catalog, then save them as a suite',
    },
    ...(onRun
      ? [{ id: 'run', label: 'Run tests', primary: true, onSelect: onRun }]
      : []),
  ]
}

/** The two blocks of the list: the repository's suites, read-only, then this
 *  Console's, which say how to make one while there are none. */
export function SuiteList({
  suites,
  items,
  selectedId,
  onLeave,
}: {
  suites: Suite[]
  items: ReadonlyMap<string, SuiteListItem>
  selectedId: string | null
  onLeave?: LeaveHandler
}) {
  const local = suites.filter((suite) => suite.source === 'local')
  const groups = [
    {
      key: 'repository',
      label: 'Repository',
      note: 'read-only',
      suites: suites.filter((suite) => suite.source !== 'local'),
    },
    {
      key: 'local',
      label: 'This Console',
      note: local.length ? 'editable' : '',
      suites: local,
    },
  ]
  return (
    <aside className="st-list" aria-label="Suites" data-suites>
      {groups.map((group) => (
        // biome-ignore lint/a11y/useSemanticElements: a labelled block of links, not a form group
        <div
          key={group.key}
          className="st-group"
          role="group"
          aria-labelledby={`st-group-${group.key}`}
          data-suite-group={group.key}
        >
          <div className="st-group-head">
            <span className="ds-label" id={`st-group-${group.key}`}>
              {group.label}
            </span>
            {group.note ? (
              <span className="st-group-note">{group.note}</span>
            ) : null}
          </div>
          {group.suites.map((suite) => {
            const item = items.get(suite.id)
            if (!item) return null
            return (
              <a
                key={suite.id}
                className="st-item"
                href={hashForSuites(suite.id)}
                aria-current={suite.id === selectedId ? 'true' : undefined}
                data-suite={suite.id}
                onClick={(event) =>
                  suite.id === selectedId
                    ? undefined
                    : onLeave?.(event, hashForSuites(suite.id))
                }
              >
                <span className="st-item-head">
                  <span className="st-item-label">{item.label}</span>
                  <span className="st-item-count">{item.count}</span>
                </span>
                <StatusLabel
                  className="st-item-last"
                  state={item.last.state}
                  label={item.last.label}
                />
                {item.changed ? (
                  <span className="st-item-changed">{item.changed}</span>
                ) : null}
              </a>
            )
          })}
          {group.key === 'local' && local.length === 0 ? (
            <p className="st-note">
              Copy a repository suite, or tick tests in{' '}
              <a href={hashForTests()}>Tests</a> and save them, to make one you
              can edit.
            </p>
          ) : null}
        </div>
      ))}
    </aside>
  )
}

/** Ticking tests in and out of a suite being edited. */
type TestsEdit = {
  tests: string[]
  /** While it saves, or while the sequential groups are unknown. */
  disabled: boolean
  onTick: (id: string, on: boolean) => void
}

/** The suite's tests: each with its last result here and its recent scores,
 *  and its step when it runs whole with others, in order. Editing, a box
 *  keeps each in the suite; an unticked one stays listed, faded. */
export function SuiteTests({
  ids,
  views,
  groups,
  narrow,
  edit = null,
}: {
  ids: string[]
  views: ReadonlyMap<string, CatalogRowView>
  groups: string[][]
  narrow: boolean
  edit?: TestsEdit | null
}) {
  const kept = edit ? ids.filter((id) => edit.tests.includes(id)) : ids
  const steps = sequenceSteps(kept, groups)
  return (
    <section className="st-section" aria-labelledby="st-tests">
      <div className="st-section-head">
        <h3 id="st-tests">Tests</h3>
        <span className="st-section-note">
          {testsNote(kept, groups, Boolean(edit))}
        </span>
      </div>
      <table
        className="st-tests"
        aria-labelledby="st-tests"
        data-narrow={narrow || undefined}
        data-editing={edit ? true : undefined}
        data-suite-tests
      >
        <thead className="ds-visually-hidden">
          <tr>
            {edit ? <th scope="col">In the suite</th> : null}
            <th scope="col">Test</th>
            <th scope="col">Last result</th>
            {narrow ? null : <th scope="col">Recent scores</th>}
          </tr>
        </thead>
        <tbody>
          {ids.map((id) => {
            const view = views.get(id)
            const step = steps.get(id)
            const on = !edit || edit.tests.includes(id)
            return (
              <tr
                key={id}
                className="st-test"
                data-test-id={id}
                data-off={on ? undefined : true}
              >
                {edit ? (
                  <td>
                    <Checkbox
                      aria-label={`Keep ${id} in the suite`}
                      checked={on}
                      disabled={edit.disabled}
                      onChange={() => edit.onTick(id, !on)}
                    />
                  </td>
                ) : null}
                <td className="tc-stack">
                  <span className="st-test-line">
                    <a className="tc-id" href={hashForTestHistory(id)}>
                      {id}
                    </a>
                    {step ? (
                      <span
                        className="st-step"
                        title="Runs whole, in this order"
                      >
                        {step}
                      </span>
                    ) : null}
                  </span>
                  {view?.sub ? (
                    <span className="tc-sub" title={view.sub}>
                      {view.sub}
                    </span>
                  ) : null}
                </td>
                {view ? <LastResult view={view} /> : <td />}
                {narrow ? null : (
                  <td>{view ? <Sparkline view={view} /> : null}</td>
                )}
              </tr>
            )
          })}
        </tbody>
      </table>
    </section>
  )
}

/** Search the catalog for tests the suite does not hold, and add them. */
function AddTests({
  catalog,
  draft,
  disabled,
  onAdd,
}: {
  catalog: CatalogRowView[]
  draft: SuiteDraft
  disabled: boolean
  onAdd: (id: string) => void
}) {
  const [query, setQuery] = useState('')
  const field = useRef<HTMLInputElement>(null)
  const found = testSuggestions(query, catalog, draft)
  return (
    <div className="st-add">
      <label className="st-field st-add-field" htmlFor="st-add">
        <span className="st-field-label">Add tests</span>
        <Input
          ref={field}
          id="st-add"
          type="text"
          value={query}
          placeholder="Search the catalog"
          disabled={disabled}
          onChange={(event) => setQuery(event.target.value)}
        />
      </label>
      {query.trim() ? (
        found.length > 0 ? (
          // biome-ignore lint/a11y/useSemanticElements: a labelled set of buttons
          <div
            className="st-suggestions"
            role="group"
            aria-label="Tests to add"
          >
            {found.map((id) => (
              <button
                key={id}
                type="button"
                className="st-suggestion"
                aria-label={`Add ${id}`}
                disabled={disabled}
                onClick={() => {
                  onAdd(id)
                  // The button goes with the suggestion: back to the search.
                  field.current?.focus()
                }}
              >
                <Plus size={16} aria-hidden="true" />
                {id}
              </button>
            ))}
          </div>
        ) : (
          <p className="st-section-note" role="status">
            No test to add matches.
          </p>
        )
      ) : null}
    </div>
  )
}

/** Runs of each test, or retries on crash: − value +. */
function Stepper({
  id,
  label,
  noun,
  value,
  min,
  max,
  disabled,
  onChange,
}: {
  id: string
  label: string
  noun: string
  value: number
  min: number
  max: number
  disabled: boolean
  onChange: (value: number) => void
}) {
  return (
    // biome-ignore lint/a11y/useSemanticElements: a labelled stepper group
    <div className="st-field" role="group" aria-labelledby={`${id}-label`}>
      <span className="st-field-label" id={`${id}-label`}>
        {label}
      </span>
      {/* aria-disabled, not disabled: at a limit the button keeps focus. */}
      <div className="st-stepper">
        <button
          type="button"
          className="st-step-button"
          aria-label={`Fewer ${noun}`}
          aria-disabled={disabled || value <= min || undefined}
          onClick={() => !disabled && value > min && onChange(value - 1)}
        >
          <Minus size={16} aria-hidden="true" />
        </button>
        <output id={id} className="st-stepper-value" aria-live="polite">
          {value}
        </output>
        <button
          type="button"
          className="st-step-button"
          aria-label={`More ${noun}`}
          aria-disabled={disabled || value >= max || undefined}
          onClick={() => !disabled && value < max && onChange(value + 1)}
        >
          <Plus size={16} aria-hidden="true" />
        </button>
      </div>
    </div>
  )
}

/** The executions that ran the suite, newest first. */
export function SuiteExecutions({
  runs,
  local,
  scope,
}: {
  runs: SuiteRun[]
  local: boolean
  scope: ExecutionScope
}) {
  return (
    <section className="st-section" aria-labelledby="st-executions">
      <div className="st-section-head">
        <h3 id="st-executions">Executions of this suite</h3>
      </div>
      {runs.length === 0 ? (
        <p className="st-note">{notRunNote(scope, local)}</p>
      ) : (
        <ul className="st-runs">
          {runs.map((run) => (
            <li key={run.id}>
              <a
                className="st-run"
                href={hashForExecution(run.id)}
                data-suite-run={run.id}
              >
                <span className="st-run-text">
                  <span className="st-run-title">{run.title}</span>
                  <span className="st-run-meta">{run.meta}</span>
                </span>
                <span className="st-run-result">
                  <StatusLabel
                    state={run.result.state}
                    label={run.result.label}
                  />
                  {run.outcome ? <span>· {run.outcome}</span> : null}
                </span>
              </a>
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

/** A suite of this Console being edited: its draft and what changes it. */
export type SuiteEditing = {
  draft: SuiteDraft
  /** Every test of the catalog, to add. */
  catalog: CatalogRowView[]
  saving: boolean
  onChange: (draft: SuiteDraft) => void
  onSave: () => void
  onDiscard: () => void
}

export type SuiteDetailProps = {
  suite: Suite
  runs: SuiteRun[]
  /** How far the executions read reach, for a suite none of them ran. */
  scope?: ExecutionScope
  views: ReadonlyMap<string, CatalogRowView>
  /** Which tests run whole, in order; null until the catalog answers. */
  groups: string[][] | null
  /** Why the catalog did not answer, to try again. */
  groupsError?: string | null
  onRetryGroups?: () => void
  narrow: boolean
  /** The bridge answered: the actions can run. */
  ready: boolean
  /** An action on this suite is under way. */
  busy: boolean
  error: string | null
  /** Set while the suite (one of this Console) is being edited. */
  editing?: SuiteEditing | null
  onLeave?: LeaveHandler
  onCopy: () => void
  onEdit: () => void
  onRun: () => void
  onDelete: () => void
}

/** The open suite: its name, facts and purpose, its actions, its tests and
 *  the executions that ran it. A suite of this Console edits in place. */
export function SuiteDetail({
  suite,
  runs,
  scope = executionScope(runs.length),
  views,
  groups,
  groupsError = null,
  onRetryGroups,
  narrow,
  ready,
  busy,
  error,
  editing = null,
  onLeave,
  onCopy,
  onEdit,
  onRun,
  onDelete,
}: SuiteDetailProps) {
  const local = suite.source === 'local'
  const draft = editing?.draft ?? null
  const tests = draft ? draft.tests : suite.scenarios
  const changed = changedTests(tests, views).length
  const dirty = draft ? draftDirty(draft, suite) : false
  // While it saves, what it sends cannot change under it; without the
  // groups, a tick could split one.
  const locked = Boolean(editing?.saving)
  const known = groups ?? []
  const waiting = groups === null
  const update = (patch: Partial<SuiteDraft>) =>
    draft && editing?.onChange({ ...draft, ...patch })
  const tick = (id: string, on: boolean) =>
    draft && groups && editing?.onChange(tickDraft(draft, id, on, groups))

  // Into the name when editing starts (a copy opens editing); back to Edit
  // when it ends.
  const wasEditing = useRef(false)
  useEffect(() => {
    if (editing && !wasEditing.current)
      document.getElementById('st-name')?.focus()
    if (!editing && wasEditing.current)
      document.getElementById('st-edit')?.focus()
    wasEditing.current = Boolean(editing)
  }, [editing])

  return (
    <section
      className="st-detail"
      aria-labelledby="st-title"
      data-suite-detail={suite.id}
      data-editing={editing ? true : undefined}
    >
      {narrow ? (
        <a
          className="st-back"
          href={hashForSuites()}
          onClick={(event) => onLeave?.(event, hashForSuites())}
        >
          <ChevronLeft size={16} aria-hidden="true" />
          Suites
        </a>
      ) : null}
      <div className="st-head">
        <div className="st-head-main">
          {draft ? (
            <>
              <h2 id="st-title" className="ds-visually-hidden">
                Editing {suite.label}
              </h2>
              <label className="st-field st-name" htmlFor="st-name">
                <span className="st-field-label">Name</span>
                <Input
                  id="st-name"
                  type="text"
                  maxLength={160}
                  value={draft.label}
                  disabled={locked}
                  aria-invalid={draft.label.trim() ? undefined : true}
                  onChange={(event) => update({ label: event.target.value })}
                />
              </label>
            </>
          ) : (
            <h2 id="st-title" className="st-title">
              {suite.label}
            </h2>
          )}
          <FactList aria-label="About this suite">
            <FactChip
              className="st-fact-words"
              label="Source"
              value={local ? 'this Console' : 'repository · read-only'}
            />
            <FactChip
              className="st-fact-words"
              label="Holds"
              value={suiteHolds(
                tests.length,
                draft ? draft.runs : suite.repetitions,
                draft ? draft.retries : suite.technical_retries,
              )}
            />
            <FactChip label="Id" value={suite.id} />
            <FactChip
              label="Digest"
              value={suiteDigest(suite)}
              full={suite.sha256 ?? 'none: this runner cannot materialize it'}
            />
          </FactList>
        </div>
        <div className="st-actions">
          {local ? null : (
            <button
              type="button"
              className={dashboardHeaderActionClassName()}
              disabled={!ready || busy}
              aria-busy={busy || undefined}
              onClick={onCopy}
            >
              <Copy size={16} aria-hidden="true" />
              Copy to edit
            </button>
          )}
          {local && !editing ? (
            <button
              id="st-edit"
              type="button"
              className={dashboardHeaderActionClassName()}
              disabled={!ready || busy || (waiting && !groupsError)}
              title={
                waiting && !groupsError
                  ? 'Waiting for the catalog, which says which tests run together'
                  : undefined
              }
              onClick={onEdit}
            >
              Edit
            </button>
          ) : null}
          {editing ? (
            <>
              <button
                type="button"
                className={dashboardHeaderActionClassName()}
                disabled={editing.saving}
                onClick={editing.onDiscard}
              >
                Discard
              </button>
              <button
                type="button"
                className={dashboardHeaderActionClassName({ primary: true })}
                disabled={editing.saving || waiting}
                aria-busy={editing.saving || undefined}
                aria-describedby={waiting ? 'st-save-blocked' : undefined}
                onClick={editing.onSave}
              >
                {editing.saving ? 'Saving…' : 'Save suite'}
              </button>
            </>
          ) : (
            <button
              type="button"
              className={dashboardHeaderActionClassName({ primary: true })}
              disabled={!ready}
              onClick={onRun}
            >
              Run this suite
            </button>
          )}
          {local ? (
            <button
              type="button"
              className={dashboardHeaderActionClassName({
                className: 'harness-e2e-header-action-icon',
              })}
              aria-label={`Delete ${suite.label}`}
              title="Delete suite"
              disabled={!ready || busy || Boolean(editing?.saving)}
              onClick={onDelete}
            >
              <Trash2 size={16} aria-hidden="true" />
            </button>
          ) : null}
        </div>
      </div>
      {suite.purpose ? <p className="st-purpose">{suite.purpose}</p> : null}
      {error ? (
        <p className="st-error" role="alert">
          {error}
        </p>
      ) : null}
      {draft ? (
        <div className="st-edit">
          <Stepper
            id="st-runs"
            label="Runs of each test"
            noun="runs"
            value={draft.runs}
            min={1}
            max={20}
            disabled={locked}
            onChange={(runs) => update({ runs })}
          />
          <Stepper
            id="st-retries"
            label="Retries on crash"
            noun="retries"
            value={draft.retries}
            min={0}
            max={3}
            disabled={locked}
            onChange={(retries) => update({ retries })}
          />
          <p className="st-edit-note">
            A suite is only what to test. The model and the stack are picked
            when it runs.
          </p>
          <span className="st-dirty" role="status">
            {dirty ? 'Unsaved changes' : ''}
          </span>
        </div>
      ) : null}
      {draft && waiting ? (
        groupsError ? (
          <Callout
            tone="danger"
            title="Which tests run together could not be read"
          >
            <span className="ex-callout-line">
              <span id="st-save-blocked">
                {groupsError}. Save waits for it, so that no sequential group is
                split.
              </span>
              <button
                className={dashboardHeaderActionClassName()}
                type="button"
                onClick={onRetryGroups}
              >
                Try again
              </button>
            </span>
          </Callout>
        ) : (
          <p className="st-section-note" id="st-save-blocked" role="status">
            Reading which tests run together, so that none is split. Save waits
            for it.
          </p>
        )
      ) : null}
      {changed > 0 ? (
        <Callout tone="warning" icon={<Info size={16} />}>
          {changedWarning(changed, tests.length)}
        </Callout>
      ) : null}
      <SuiteTests
        ids={draft ? draft.shown : suite.scenarios}
        views={views}
        groups={known}
        narrow={narrow}
        edit={
          draft
            ? { tests: draft.tests, disabled: locked || waiting, onTick: tick }
            : null
        }
      />
      {draft && editing ? (
        <AddTests
          catalog={editing.catalog}
          draft={draft}
          disabled={locked || waiting}
          onAdd={(id) => tick(id, true)}
        />
      ) : null}
      <SuiteExecutions runs={runs} local={local} scope={scope} />
    </section>
  )
}

/** A hash naming a suite that is not listed: said, with the way back. */
export function MissingSuite() {
  return (
    <section
      className="st-detail"
      aria-labelledby="st-missing"
      data-suite-missing
    >
      <h2 id="st-missing" className="st-title">
        That suite is not here any more
      </h2>
      <p className="st-purpose">
        It was deleted, or the link names a suite of another Console.
      </p>
      <a className="st-back" href={hashForSuites()}>
        <ChevronLeft size={16} aria-hidden="true" />
        All the suites
      </a>
    </section>
  )
}

function DeleteSuiteDialog({
  suite,
  deleting,
  onCancel,
  onConfirm,
  onCloseAutoFocus,
}: {
  suite: Suite | null
  deleting: boolean
  onCancel: () => void
  onConfirm: () => void
  onCloseAutoFocus: (event: Event) => void
}) {
  // The suite stays named while the dialog animates out.
  const shown = useRef<Suite | null>(null)
  if (suite) shown.current = suite
  const label = shown.current?.label ?? 'suite'
  return (
    <Dialog
      open={suite !== null}
      onOpenChange={(open) => {
        if (!open && !deleting) onCancel()
      }}
    >
      <DialogContent
        role="alertdialog"
        className="ex-dialog"
        aria-describedby="st-delete-body"
        onCloseAutoFocus={onCloseAutoFocus}
      >
        <div className="ex-dialog-head">
          <span className="ex-dialog-icon" aria-hidden="true">
            <Trash2 size={16} />
          </span>
          <div>
            <DialogTitle className="ex-dialog-title">
              Delete {label}?
            </DialogTitle>
            <DialogDescription id="st-delete-body" className="ex-dialog-body">
              Executions that ran it keep its name and digest.
            </DialogDescription>
          </div>
        </div>
        <div className="ex-dialog-actions">
          <Button
            type="button"
            variant="pill"
            size="sm"
            disabled={deleting}
            onClick={onCancel}
          >
            Cancel
          </Button>
          <Button
            type="button"
            variant="pill"
            size="sm"
            className="ex-danger"
            disabled={deleting}
            aria-busy={deleting}
            onClick={onConfirm}
          >
            {deleting ? 'Deleting…' : 'Delete suite'}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}

/** Suites: the repository's, read-only, and this Console's. A repository
 *  suite is copied into one of this Console to edit it. */
export function SuitesPage() {
  const narrow = useDashboardChrome()?.narrow ?? false
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [suites, setSuites] = useState<Suite[] | null>(null)
  const [rows, setRows] = useState<TestCatalogRow[]>([])
  const [executions, setExecutions] = useState<DashboardExecutionSummary[]>([])
  // How far the executions read reach; null when they could not be read.
  const [scope, setScope] = useState<ExecutionScope>(executionScope(0))
  // Which tests run whole, in order: the runner's catalog says it. Editing
  // waits for it; its failure is said in the editor.
  const [groups, setGroups] = useState<string[][] | null>(null)
  const [groupsError, setGroupsError] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [param, setParam] = useState<string | null>(suiteParam)
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [deleting, setDeleting] = useState<Suite | null>(null)
  // A deleted suite's button is gone: focus goes to the page's heading.
  const deleted = useRef(false)
  // The drafts of suites being edited, mirrored from openDrafts.
  const [drafts, setDrafts] = useState<ReadonlyMap<string, SuiteDraft>>(
    () => new Map(openDrafts),
  )
  const putDraft = useCallback((suiteId: string, draft: SuiteDraft | null) => {
    if (draft) openDrafts.set(suiteId, draft)
    else openDrafts.delete(suiteId)
    setDrafts(new Map(openDrafts))
  }, [])
  const [saving, setSaving] = useState(false)
  // Where a click would have gone while it asks to discard the changes.
  const [leaving, setLeaving] = useState<string | null>(null)
  // The suite Run tests opens on ('' for none) while it is open, and which
  // opening it is: each one mounts the dialog afresh.
  const [runner, setRunner] = useState<string | null>(null)
  const [opening, setOpening] = useState(0)
  const openRunner = useCallback((suiteId: string) => {
    setOpening((count) => count + 1)
    setRunner(suiteId)
  }, [])
  const beginRequest = useLatestRequest()

  const load = useCallback(async () => {
    const request = beginRequest()
    setError(null)
    try {
      const next = await getDashboardDataBridge()
      if (!request.isCurrent()) return
      setBridge(next)
      const [listed, tests, recent] = await Promise.all([
        next.listSuites(),
        // The last results and the executions add to the suites; they show
        // without them.
        listAllTests(next).catch(() => null),
        next.listExecutions({ limit: EXECUTIONS_READ }).catch(() => null),
      ])
      if (!request.isCurrent()) return
      setSuites(listed.suites)
      if (tests) setRows(tests.rows)
      const read = recent?.executions ?? []
      setExecutions(read)
      setScope(recent ? executionScope(read.length, recent.total) : null)
    } catch (cause) {
      if (request.isCurrent()) setError(errorText(cause))
    }
  }, [beginRequest])

  useEffect(() => {
    void load()
  }, [load])

  const loadGroups = useCallback(async (from: DashboardDataBridge) => {
    setGroupsError(null)
    try {
      setGroups(asCatalog(await from.getCatalog()).groups)
    } catch (cause) {
      setGroupsError(errorText(cause))
    }
  }, [])
  useEffect(() => {
    if (bridge) void loadGroups(bridge)
  }, [bridge, loadGroups])

  // A finished run changes a suite's last execution and its tests' results.
  useRunChanges(bridge, load)

  // The open suite lives in the hash, so Back leaves a suite for the list.
  useEffect(() => {
    const sync = () => {
      setParam(suiteParam())
      setActionError(null)
    }
    window.addEventListener('hashchange', sync)
    return () => window.removeEventListener('hashchange', sync)
  }, [])

  const views = useMemo(
    () =>
      new Map<string, CatalogRowView>(
        rows.map((row) => [row.test_id, catalogRowView(row, [])]),
      ),
    [rows],
  )
  const catalog = useMemo(() => [...views.values()], [views])
  const runsBySuite = useMemo(
    () =>
      new Map(
        (suites ?? []).map((suite) => [
          suite.id,
          suiteRuns(executions, suite.id),
        ]),
      ),
    [suites, executions],
  )
  const items = useMemo(
    () =>
      new Map(
        (suites ?? []).map((suite) => [
          suite.id,
          suiteListItem(suite, runsBySuite.get(suite.id) ?? [], views, scope),
        ]),
      ),
    [suites, runsBySuite, views, scope],
  )

  const { suite: selected, missing } = openSuite(suites, param, narrow)
  const failedFirstLoad = Boolean(error) && suites === null
  const draft = selected ? (drafts.get(selected.id) ?? null) : null
  const dirty = Boolean(selected && draft && draftDirty(draft, selected))

  // Leaving a suite drops its draft when nothing in it is unsaved; so does
  // leaving the page. Unsaved changes stay until the suite opens again.
  const suitesNow = useRef(suites)
  suitesNow.current = suites
  const shownId = selected?.id ?? null
  const lastShown = useRef<string | null>(null)
  useEffect(() => {
    const left = lastShown.current
    lastShown.current = shownId
    if (!left || left === shownId) return
    dropCleanDraft(left, suitesNow.current)
    setDrafts(new Map(openDrafts))
  }, [shownId])
  useEffect(
    () => () => {
      for (const id of [...openDrafts.keys()])
        dropCleanDraft(id, suitesNow.current)
    },
    [],
  )

  /** A click that would leave unsaved changes asks first. */
  const onLeave: LeaveHandler = (event, target) => {
    if (!dirty) return
    event.preventDefault()
    setLeaving(target)
  }

  const headerActions = useMemo(
    () =>
      suitesHeaderActions(
        bridge ? () => openRunner('') : undefined,
        dirty ? () => setLeaving(hashForTests()) : undefined,
      ),
    [bridge, openRunner, dirty],
  )

  const copy = async (suite: Suite) => {
    if (!bridge) return
    setBusy(true)
    setActionError(null)
    try {
      const created = await bridge.createSuite(suite.id)
      // Listed and opened to edit as the worker answered, whatever the
      // reload does: the copy is saved, its changes are not yet.
      setSuites((current) => upsertSuite(current ?? [], created))
      putDraft(created.id, suiteDraft(created))
      window.location.hash = hashForSuites(created.id)
      void load()
    } catch (cause) {
      setActionError(errorText(cause))
    } finally {
      setBusy(false)
    }
  }
  const remove = async (suite: Suite) => {
    if (!bridge) return
    setBusy(true)
    setActionError(null)
    try {
      await bridge.deleteSuite(suite.id)
      deleted.current = true
      setDeleting(null)
      putDraft(suite.id, null)
      await load()
      window.location.hash = hashForSuites()
    } catch (cause) {
      setDeleting(null)
      setActionError(errorText(cause))
    } finally {
      setBusy(false)
    }
  }

  const save = async () => {
    if (!bridge || !selected || !draft) return
    const suiteId = selected.id
    const problem = draftProblem(draft)
    if (problem) {
      setActionError(problem)
      if (!draft.label.trim()) document.getElementById('st-name')?.focus()
      return
    }
    setSaving(true)
    setActionError(null)
    try {
      const saved = await bridge.updateSuite(suiteId, draftChanges(draft))
      // The suite as saved closes the editor; the reload only refreshes.
      setSuites((current) => upsertSuite(current ?? [], saved))
      putDraft(suiteId, null)
      void load()
    } catch (cause) {
      setActionError(errorText(cause))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="ds-root ex-page st-page">
      <DashboardPageActions
        active="suites"
        actionsLabel="Suite actions"
        actions={headerActions}
        context={selected?.label}
      />
      <header className="ex-header">
        <h1 id="st-heading" tabIndex={-1}>
          Suites
        </h1>
        {failedFirstLoad ? null : (
          <p>{suites ? suitesSummary(suites) : 'Loading the suites…'}</p>
        )}
      </header>

      {error && !failedFirstLoad ? (
        <Callout tone="danger" title="The suites could not be reloaded">
          <span className="ex-callout-line">
            {error}
            <button
              className={buttonClassName({
                variant: 'secondary',
                size: 'compact',
              })}
              type="button"
              onClick={() => void load()}
            >
              Try again
            </button>
          </span>
        </Callout>
      ) : null}

      {suites === null ? (
        failedFirstLoad ? (
          <EmptyState
            tone="error"
            title="Suites could not be loaded"
            description={error}
            actions={
              <button
                className={buttonClassName({ variant: 'secondary' })}
                type="button"
                onClick={() => void load()}
              >
                Try again
              </button>
            }
          />
        ) : (
          <div className="ex-loading" aria-busy="true" role="status">
            <span className="ds-visually-hidden">Loading the suites</span>
            {Array.from({ length: 5 }, (_, index) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
              <div key={index} />
            ))}
          </div>
        )
      ) : (
        <div className="st-layout" data-narrow={narrow || undefined}>
          {narrow && (selected || missing) ? null : (
            <SuiteList
              suites={suites}
              items={items}
              selectedId={selected?.id ?? null}
              onLeave={onLeave}
            />
          )}
          {selected ? (
            <SuiteDetail
              key={selected.id}
              suite={selected}
              runs={runsBySuite.get(selected.id) ?? []}
              scope={scope}
              views={views}
              groups={groups}
              groupsError={groupsError}
              onRetryGroups={() => bridge && void loadGroups(bridge)}
              narrow={narrow}
              ready={Boolean(bridge)}
              busy={busy}
              error={actionError}
              editing={
                draft
                  ? {
                      draft,
                      catalog,
                      saving,
                      onChange: (next) => putDraft(selected.id, next),
                      onSave: () => void save(),
                      onDiscard: () => {
                        putDraft(selected.id, null)
                        setActionError(null)
                      },
                    }
                  : null
              }
              onLeave={onLeave}
              onCopy={() => void copy(selected)}
              onEdit={() => putDraft(selected.id, suiteDraft(selected))}
              onRun={() => openRunner(selected.id)}
              onDelete={() => setDeleting(selected)}
            />
          ) : missing ? (
            <MissingSuite />
          ) : null}
        </div>
      )}

      <ConfirmDialog
        open={leaving !== null}
        onOpenChange={(open) => {
          if (!open) setLeaving(null)
        }}
        title={`Discard changes to ${selected?.label ?? 'this suite'}?`}
        description="What you changed in it is not saved. Discarding keeps the suite as it was last saved."
        cancelLabel="Keep editing"
        confirmLabel="Discard changes"
        tone="danger"
        onConfirm={() => {
          if (selected) putDraft(selected.id, null)
          if (leaving) window.location.hash = leaving
        }}
      />
      <DeleteSuiteDialog
        suite={deleting}
        deleting={busy}
        onCancel={() => setDeleting(null)}
        onConfirm={() => deleting && void remove(deleting)}
        onCloseAutoFocus={(event) => {
          if (!deleted.current) return
          deleted.current = false
          event.preventDefault()
          document.getElementById('st-heading')?.focus()
        }}
      />
      <LocalRunnerDialog
        key={opening}
        bridge={bridge}
        open={runner !== null}
        initialSuite={runner || undefined}
        onClose={() => setRunner(null)}
      />
    </div>
  )
}
