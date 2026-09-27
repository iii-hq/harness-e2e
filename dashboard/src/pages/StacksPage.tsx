import {
  ConfirmDialog,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@iii-dev/console-ui'
import {
  ArrowUpRight,
  CircleX,
  Copy,
  Lock,
  Trash2,
  TriangleAlert,
} from 'lucide-react'
import {
  type CSSProperties,
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
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import { ProviderCredentials } from '@/components/ProviderCredentials'
import {
  buttonClassName,
  Callout,
  EmptyState,
  FactChip,
  FactList,
} from '@/design-system'
import { useLatestRequest } from '@/hooks/use-latest-request'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
  type Stack,
} from '@/lib/dashboard-data-source'
import { formatDateTime } from '@/lib/format'
import {
  BASE_STACK,
  iiiHint,
  pinOf,
  savedStatus,
  stackDeclares,
  stackDiff,
  stackFile,
  stackSub,
  stacksSummary,
  templateHint,
  templateUrl,
  warningsTitle,
  workerWarns,
  yamlLine,
  yamlLines,
  yamlMeta,
} from '@/lib/stacks-view'
import '@/design-system/styles.css'
import './executions-page.css'
import './stacks-page.css'

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** The list with a stack as the worker answered it, in its place or last. */
function upsertStack(stacks: Stack[] | null, stack: Stack) {
  const listed = stacks ?? []
  return listed.some((entry) => entry.id === stack.id)
    ? listed.map((entry) => (entry.id === stack.id ? stack : entry))
    : [...listed, stack]
}

const CREATED = 'Created. Change what you need, then save.'

/** A stack opened from the list: what it installs, its workers and warnings
 *  beside its YAML exactly as written. A repository stack is read-only and
 *  copied to edit; a stack of this Console is edited here, its name and its
 *  YAML. Saving keeps it open with the warnings of what was saved; YAML the
 *  runner refuses is said next to the editor, which keeps what was typed. */
export function StackSheet({
  stack,
  mode,
  narrow,
  bridge,
  busy,
  copyError,
  created = false,
  onCopy,
  onSaved,
  onClose,
}: {
  stack: Stack
  mode: 'view' | 'edit'
  narrow: boolean
  bridge: DashboardDataBridge | null
  busy: boolean
  /** Why Copy to edit did not go through. */
  copyError: string | null
  /** Opened right after it was created. */
  created?: boolean
  onCopy: () => void
  onSaved: (stack: Stack) => void
  onClose: () => void
}) {
  const editing = mode === 'edit'
  const [draft, setDraft] = useState({ label: stack.label, yaml: stack.yaml })
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [status, setStatus] = useState(created ? CREATED : '')
  const [copied, setCopied] = useState(false)
  const [asking, setAsking] = useState(false)
  const code = useRef<HTMLElement>(null)
  const dirty =
    editing && (draft.label.trim() !== stack.label || draft.yaml !== stack.yaml)
  const lines = yamlLines(stack.yaml)
  const editorLines = draft.yaml.split('\n').length
  const warnings = stack.warnings

  const close = () => (dirty ? setAsking(true) : onClose())
  const save = async () => {
    if (!bridge) return
    if (draft.label.trim() === '') {
      setError(
        'Name the stack (up to 160 characters, without control characters).',
      )
      document.getElementById('sk-name')?.focus()
      return
    }
    setSaving(true)
    setError(null)
    try {
      const next = await bridge.updateStack(stack.id, {
        label: draft.label.trim(),
        yaml: draft.yaml,
      })
      setDraft({ label: next.label, yaml: next.yaml })
      setStatus(savedStatus(next))
      onSaved(next)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSaving(false)
    }
  }
  const copyYaml = () => {
    void navigator.clipboard
      ?.writeText(editing ? draft.yaml : stack.yaml)
      .then(() => setCopied(true))
      .catch(() => setCopied(false))
  }

  const [tone, said] = saving
    ? ['faint', 'Saving…']
    : copyError && !editing
      ? ['alert', copyError]
      : !editing
        ? ['faint', 'Read-only. Copies you make appear under This Console.']
        : dirty
          ? [
              'warn',
              'Unsaved changes. The summary and warnings refresh when you save.',
            ]
          : [
              'faint',
              status ||
                (stack.updated_at
                  ? `Saved ${formatDateTime(stack.updated_at)}.`
                  : ''),
            ]

  return (
    <>
      <Dialog
        open
        onOpenChange={(open) => {
          if (!open && !saving) close()
        }}
      >
        <DialogContent
          className="sk-sheet"
          data-narrow={narrow || undefined}
          data-stack-sheet={mode}
          onOpenAutoFocus={(event) => {
            if (editing) return
            event.preventDefault()
            code.current?.focus()
          }}
        >
          <header className="sk-sheet-head">
            {editing ? (
              <>
                <DialogTitle className="ds-visually-hidden">
                  Edit {stack.label}
                </DialogTitle>
                <label className="sk-field-label" htmlFor="sk-name">
                  Stack name
                </label>
                <input
                  id="sk-name"
                  className="sk-input sk-name-input"
                  value={draft.label}
                  maxLength={160}
                  autoComplete="off"
                  disabled={saving}
                  onChange={(event) =>
                    setDraft({ ...draft, label: event.target.value })
                  }
                />
              </>
            ) : (
              <div className="sk-sheet-title-row">
                <DialogTitle className="sk-sheet-title">
                  {stack.label}
                </DialogTitle>
                <span className="sk-badge">
                  <Lock size={14} aria-hidden="true" />
                  Repository · read-only
                </span>
              </div>
            )}
            <DialogDescription className="sk-sheet-desc">
              {editing
                ? 'Kept in this Console. Only YAML that does not parse, or has no containers, is refused. Everything else is a warning.'
                : `${stackFile(stack)}, built into this runner. Copy it to change a version, a worker or the template.`}
            </DialogDescription>
          </header>

          <div className="sk-sheet-body">
            <aside className="sk-aside" aria-label="What it declares">
              {editing && error ? (
                <div className="sk-alert" role="alert" id="sk-error">
                  <CircleX size={16} aria-hidden="true" />
                  <span>
                    <strong>Not saved.</strong> {error}
                  </span>
                </div>
              ) : null}
              {warnings.length ? (
                <div className="sk-warnbox" role="status" data-stack-warnings>
                  <span className="sk-warnbox-title">
                    <TriangleAlert size={16} aria-hidden="true" />
                    {warningsTitle(warnings.length)}
                    {editing ? ' as last saved' : ''}
                  </span>
                  <ul>
                    {warnings.map((warning, index) => (
                      // biome-ignore lint/suspicious/noArrayIndexKey: warnings repeat and never reorder
                      <li key={index}>{warning}</li>
                    ))}
                  </ul>
                  <span className="sk-warnbox-note">
                    Warnings never stop a run.
                  </span>
                </div>
              ) : null}
              <section className="sk-section" aria-labelledby="sk-installs">
                <h3 id="sk-installs">Installs</h3>
                <dl className="sk-installs">
                  <dt>iii release</dt>
                  <dd>
                    <span className="sk-mono">{stack.iii ?? '—'}</span>
                    {iiiHint(stack.iii) ? (
                      <span className="sk-faint">{iiiHint(stack.iii)}</span>
                    ) : null}
                  </dd>
                  <dt>Template</dt>
                  <dd>
                    {stack.template ? (
                      <>
                        <a
                          className="sk-mono sk-link"
                          href={templateUrl(stack.template)}
                          target="_blank"
                          rel="noreferrer"
                        >
                          {stack.template}
                          <ArrowUpRight size={12} aria-hidden="true" />
                        </a>
                        <span className="sk-faint">
                          {templateHint(stack.template)}
                        </span>
                      </>
                    ) : (
                      <span className="sk-faint">
                        None. The groups start from an empty project.
                      </span>
                    )}
                  </dd>
                </dl>
              </section>
              <section className="sk-section" aria-labelledby="sk-workers">
                <h3 id="sk-workers">
                  Workers{' '}
                  <span className="sk-count">{stack.containers.length}</span>
                </h3>
                {stack.containers.length ? (
                  // biome-ignore lint/a11y/noRedundantRoles: Safari drops the list role under list-style none
                  <ul role="list" className="sk-workers">
                    {stack.containers.map((container) => (
                      <li key={container.name} data-worker={container.name}>
                        <span className="sk-worker-name">
                          <span>{container.name}</span>
                          <span
                            className="sk-worker-ref"
                            data-warn={workerWarns(container) || undefined}
                          >
                            {container.worker ?? 'no worker'}
                          </span>
                        </span>
                        <span
                          className="sk-worker-pin"
                          data-set={
                            container.version || container.commit
                              ? true
                              : undefined
                          }
                        >
                          {pinOf(container)}
                        </span>
                      </li>
                    ))}
                  </ul>
                ) : null}
              </section>
              <section className="sk-section" aria-labelledby="sk-added">
                <h3 id="sk-added">Added when it runs</h3>
                <p className="sk-faint">
                  The workers these depend on (state, llm-router,
                  session-manager and the rest) come from their packages. Each
                  group gets its own namespace, data directory and provider
                  credentials. Credentials never go in a stack.
                </p>
              </section>
            </aside>

            <div className="sk-yaml">
              <div className="sk-yaml-bar">
                <span className="sk-yaml-file">
                  {editing ? 'YAML' : stackFile(stack)}
                </span>
                <span className="sk-faint">
                  {editing
                    ? 'kept exactly as written, comments included'
                    : yamlMeta(stack.yaml)}
                </span>
                <button
                  type="button"
                  className="sk-btn sk-btn-small"
                  onClick={copyYaml}
                >
                  <Copy size={14} aria-hidden="true" />
                  {copied ? 'Copied' : 'Copy YAML'}
                </button>
              </div>
              {editing ? (
                <div className="sk-editor">
                  <div className="sk-gutter" aria-hidden="true">
                    {Array.from(
                      { length: editorLines },
                      (_, index) => index + 1,
                    ).join('\n')}
                  </div>
                  <textarea
                    id="sk-yaml"
                    className="sk-textarea"
                    aria-label="Stack YAML"
                    aria-invalid={error ? true : undefined}
                    aria-describedby={error ? 'sk-error' : undefined}
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    wrap="off"
                    value={draft.yaml}
                    disabled={saving}
                    style={{ '--sk-lines': editorLines } as CSSProperties}
                    onChange={(event) => {
                      setCopied(false)
                      setDraft({ ...draft, yaml: event.target.value })
                    }}
                  />
                </div>
              ) : (
                <section
                  ref={code}
                  className="sk-code"
                  aria-label="Stack YAML, read-only"
                  // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrolling region reads by keyboard
                  tabIndex={0}
                >
                  {lines.map((line, index) => {
                    const part = yamlLine(line)
                    return (
                      // biome-ignore lint/suspicious/noArrayIndexKey: the lines of a text that does not change here
                      <div key={index} className="sk-line">
                        <span className="sk-ln" aria-hidden="true">
                          {index + 1}
                        </span>
                        <span
                          className="sk-text"
                          data-comment={part.comment || undefined}
                        >
                          {part.key ? (
                            <span className="sk-key">{part.key}</span>
                          ) : null}
                          {part.rest || ' '}
                        </span>
                      </div>
                    )
                  })}
                </section>
              )}
            </div>
          </div>

          <footer className="sk-sheet-foot">
            <span className="sk-status" role="status" data-tone={tone}>
              {said}
            </span>
            {editing ? (
              <>
                <button
                  type="button"
                  className="sk-btn"
                  disabled={saving}
                  onClick={() => {
                    if (!dirty) return onClose()
                    setDraft({ label: stack.label, yaml: stack.yaml })
                    setError(null)
                  }}
                >
                  {dirty ? 'Discard changes' : 'Close'}
                </button>
                <button
                  type="button"
                  className="sk-btn sk-btn-primary"
                  disabled={saving || !bridge}
                  aria-busy={saving || undefined}
                  onClick={() => void save()}
                >
                  {saving ? 'Saving…' : 'Save stack'}
                </button>
              </>
            ) : (
              <>
                <button type="button" className="sk-btn" onClick={onClose}>
                  Close
                </button>
                <button
                  type="button"
                  className="sk-btn sk-btn-primary"
                  disabled={busy || !bridge}
                  aria-busy={busy || undefined}
                  onClick={onCopy}
                >
                  {busy ? 'Copying…' : 'Copy to edit'}
                </button>
              </>
            )}
          </footer>
        </DialogContent>
      </Dialog>
      <ConfirmDialog
        open={asking}
        onOpenChange={(open) => {
          if (!open) setAsking(false)
        }}
        title={`Discard changes to ${stack.label}?`}
        description="What you changed in it is not saved. Discarding keeps the stack as it was last saved."
        cancelLabel="Keep editing"
        confirmLabel="Discard changes"
        tone="danger"
        onConfirm={onClose}
      />
    </>
  )
}

/** The section's actions in the Console header: New stack and, once the
 *  worker answers, Run tests. */
export function stacksHeaderActions(
  onNew?: () => void,
  onRun?: () => void,
): HeaderAction[] {
  return [
    {
      id: 'new',
      label: 'New stack',
      disabled: !onNew,
      title: 'A stack of this Console, as a copy of another',
      onSelect: onNew,
    },
    ...(onRun
      ? [{ id: 'run', label: 'Run tests', primary: true, onSelect: onRun }]
      : []),
  ]
}

export type StackListProps = {
  stacks: Stack[]
  ready: boolean
  busy: boolean
  onOpen: (stack: Stack) => void
  onCopy: (stack: Stack) => void
  onDelete: (stack: Stack) => void
  onNew: () => void
}

/** One stack: its name and file or id, what it installs and pins, how it
 *  differs from the default, its warnings, and what can be done with it. */
function StackRow({
  stack,
  base,
  ready,
  busy,
  onOpen,
  onCopy,
  onDelete,
}: Omit<StackListProps, 'stacks' | 'onNew'> & {
  stack: Stack
  base: Stack | undefined
}) {
  const local = stack.source === 'local'
  const diff = stackDiff(stack, base)
  const warned = stack.warnings.length > 0
  return (
    <div className="sk-row" data-stack={stack.id}>
      <div className="sk-row-name">
        <button
          type="button"
          className="sk-name"
          disabled={!ready}
          onClick={() => onOpen(stack)}
        >
          {stack.label}
        </button>
        <span className="sk-sub">{stackSub(stack)}</span>
      </div>
      <div className="sk-row-decl">
        <FactList aria-label="Installs">
          <FactChip label="iii" value={stack.iii ?? '—'} />
          <FactChip
            label="template"
            value={stack.template ?? 'none'}
            className={stack.template ? undefined : 'sk-fact-none'}
          />
          <FactChip label="workers" value={String(stack.containers.length)} />
        </FactList>
        {stack.containers.length ? (
          // biome-ignore lint/a11y/noRedundantRoles: Safari drops the list role under list-style none
          <ul role="list" className="sk-pins" aria-label="Pins">
            {stack.containers.map((container) => (
              <li
                key={container.name}
                data-commit={container.commit ? true : undefined}
              >
                <span>{container.name}</span>
                <span className="sk-pin">{pinOf(container)}</span>
              </li>
            ))}
          </ul>
        ) : null}
        {diff ? <p className="sk-diff">{diff}</p> : null}
      </div>
      <div className="sk-row-warn" data-stack-warnings={warned || undefined}>
        <span className="sk-warn-title">
          <span className="sk-dot" data-tone={warned ? 'warn' : 'ok'} />
          {warningsTitle(stack.warnings.length)}
        </span>
        {warned ? (
          // biome-ignore lint/a11y/noRedundantRoles: Safari drops the list role under list-style none
          <ul role="list" className="sk-warnings">
            {stack.warnings.map((warning, index) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: warnings repeat and never reorder
              <li key={index}>{warning}</li>
            ))}
          </ul>
        ) : null}
      </div>
      <div className="sk-row-actions">
        <button
          type="button"
          className={dashboardHeaderActionClassName()}
          aria-label={`${local ? 'Edit' : 'View'} ${stack.label}`}
          disabled={!ready}
          onClick={() => onOpen(stack)}
        >
          {local ? 'Edit' : 'View'}
        </button>
        <button
          type="button"
          className={dashboardHeaderActionClassName({
            className: 'harness-e2e-header-action-icon',
          })}
          aria-label={`Copy ${stack.label}`}
          title="Copy into this Console"
          disabled={!ready || busy}
          onClick={() => onCopy(stack)}
        >
          <Copy size={16} aria-hidden="true" />
        </button>
        {local ? (
          <button
            type="button"
            className={dashboardHeaderActionClassName({
              className: 'harness-e2e-header-action-icon',
            })}
            aria-label={`Delete ${stack.label}`}
            title="Delete stack"
            disabled={!ready || busy}
            onClick={() => onDelete(stack)}
          >
            <Trash2 size={16} aria-hidden="true" />
          </button>
        ) : null}
      </div>
    </div>
  )
}

/** The repository's stacks, read-only, then this Console's, which say how
 *  to make one while there are none. */
export function StackList({ stacks, onNew, ...row }: StackListProps) {
  const base = stacks.find(
    (stack) => stack.id === BASE_STACK && stack.source !== 'local',
  )
  const local = stacks.filter((stack) => stack.source === 'local')
  const groups = [
    {
      key: 'repository',
      label: 'Repository',
      note: 'read-only · stacks/*.yaml, built into this runner',
      stacks: stacks.filter((stack) => stack.source !== 'local'),
    },
    {
      key: 'local',
      label: 'This Console',
      note: 'editable · copies of another stack',
      stacks: local,
    },
  ]
  return groups.map((group) => (
    <section
      key={group.key}
      className="sk-block"
      aria-labelledby={`sk-group-${group.key}`}
      data-stack-group={group.key}
    >
      <div className="sk-block-head">
        <h2 className="ds-label" id={`sk-group-${group.key}`}>
          {group.label}
        </h2>
        <span className="sk-block-note">{group.note}</span>
      </div>
      {group.stacks.map((stack) => (
        <StackRow key={stack.id} stack={stack} base={base} {...row} />
      ))}
      {group.key === 'local' && local.length === 0 ? (
        <div className="sk-empty">
          <p>
            None yet. Copy a repository stack to change a version or a worker.
          </p>
          <button
            type="button"
            className={dashboardHeaderActionClassName()}
            disabled={!row.ready}
            onClick={onNew}
          >
            New stack
          </button>
        </div>
      ) : null}
    </section>
  ))
}

/** New stack: a stack of this Console starts as a copy of another, named
 *  after it unless a name is given; its YAML opens next to edit. */
export function NewStackDialog({
  stacks,
  narrow,
  busy,
  error,
  onCreate,
  onClose,
}: {
  stacks: Stack[]
  narrow: boolean
  busy: boolean
  /** Why the last try did not go through. */
  error: string | null
  onCreate: (from: string, label: string) => void
  onClose: () => void
}) {
  const [from, setFrom] = useState(
    () =>
      (stacks.find((stack) => stack.id === BASE_STACK) ?? stacks[0])?.id ?? '',
  )
  const [name, setName] = useState('')
  const source = stacks.find((stack) => stack.id === from)
  const groups = [
    {
      key: 'repository',
      label: 'Repository',
      stacks: stacks.filter((stack) => stack.source !== 'local'),
    },
    {
      key: 'local',
      label: 'This Console',
      stacks: stacks.filter((stack) => stack.source === 'local'),
    },
  ]
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose()
      }}
    >
      <DialogContent className="sk-new" data-narrow={narrow || undefined}>
        <header className="sk-sheet-head">
          <DialogTitle className="sk-new-title">New stack</DialogTitle>
          <DialogDescription className="sk-sheet-desc">
            A stack of this Console starts as a copy. Its YAML opens next, to
            change what you need.
          </DialogDescription>
        </header>
        <form
          id="sk-new-form"
          className="sk-new-body"
          noValidate
          onSubmit={(event) => {
            event.preventDefault()
            if (source && !busy) onCreate(source.id, name.trim())
          }}
        >
          <fieldset className="sk-from">
            <legend className="sk-field-label">Copy of</legend>
            {groups.map((group) => (
              // biome-ignore lint/a11y/useSemanticElements: a labelled part of one radio group, inside its fieldset
              <div
                key={group.key}
                role="group"
                aria-labelledby={`sk-from-${group.key}`}
                className="sk-from-group"
              >
                <span className="ds-label" id={`sk-from-${group.key}`}>
                  {group.label}
                </span>
                {group.stacks.map((stack) => (
                  <label
                    key={stack.id}
                    className="sk-option"
                    data-selected={stack.id === from || undefined}
                  >
                    <input
                      type="radio"
                      className="sk-radio"
                      name="sk-from"
                      value={stack.id}
                      checked={stack.id === from}
                      disabled={busy}
                      onChange={() => setFrom(stack.id)}
                    />
                    <span className="sk-option-text">
                      <span className="sk-option-label">{stack.label}</span>
                      <span className="sk-faint">{stackDeclares(stack)}</span>
                    </span>
                  </label>
                ))}
                {group.stacks.length === 0 ? (
                  <span className="sk-from-empty">None yet.</span>
                ) : null}
              </div>
            ))}
          </fieldset>
          <div className="sk-field">
            <label className="sk-field-label" htmlFor="sk-new-name">
              Name <span className="sk-optional">optional</span>
            </label>
            <input
              id="sk-new-name"
              className="sk-input"
              value={name}
              maxLength={160}
              autoComplete="off"
              placeholder={source ? `${source.label} copy` : ''}
              aria-describedby="sk-new-name-hint"
              disabled={busy}
              onChange={(event) => setName(event.target.value)}
            />
            <span id="sk-new-name-hint" className="sk-hint">
              Empty names it after what it copies. Rename it any time.
            </span>
          </div>
        </form>
        <footer className="sk-new-foot">
          <span
            className="sk-status"
            aria-live="polite"
            role={error ? 'alert' : undefined}
            data-tone={error ? 'alert' : 'faint'}
          >
            {error ??
              (source ? `Copies ${source.label} into this Console.` : '')}
          </span>
          <button
            type="button"
            className="sk-btn"
            disabled={busy}
            onClick={onClose}
          >
            Cancel
          </button>
          <button
            type="submit"
            form="sk-new-form"
            className="sk-btn sk-btn-primary"
            disabled={busy || !source}
            aria-busy={busy || undefined}
          >
            {busy ? 'Creating…' : 'Create and edit'}
          </button>
        </footer>
      </DialogContent>
    </Dialog>
  )
}

/** Stacks: the repository's, read-only, and this Console's. Any stack is
 *  copied into one of this Console to edit it. Below them, the provider
 *  credentials a Docker execution's stack receives. */
export function StacksPage() {
  const narrow = useDashboardChrome()?.narrow ?? false
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [stacks, setStacks] = useState<Stack[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  // The stack open in the sheet, and whether it was just created.
  const [sheet, setSheet] = useState<{
    id: string
    mode: 'view' | 'edit'
    created?: boolean
  } | null>(null)
  const [deleting, setDeleting] = useState<Stack | null>(null)
  const [creating, setCreating] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)
  const [running, setRunning] = useState(false)
  const beginRequest = useLatestRequest()

  const load = useCallback(async () => {
    const request = beginRequest()
    setError(null)
    try {
      const next = await getDashboardDataBridge()
      if (!request.isCurrent()) return
      setBridge(next)
      const listed = await next.listStacks()
      if (request.isCurrent()) setStacks(listed.stacks)
    } catch (cause) {
      if (request.isCurrent()) setError(errorText(cause))
    }
  }, [beginRequest])
  useEffect(() => {
    void load()
  }, [load])

  const openNew = useCallback(() => {
    setCreateError(null)
    setCreating(true)
  }, [])
  const headerActions = useMemo(
    () =>
      stacksHeaderActions(
        bridge ? openNew : undefined,
        bridge ? () => setRunning(true) : undefined,
      ),
    [bridge, openNew],
  )

  const copy = async (stack: Stack) => {
    if (!bridge) return
    setBusy(true)
    setActionError(null)
    try {
      const created = await bridge.createStack(stack.id)
      setStacks((current) => upsertStack(current, created))
      setSheet({ id: created.id, mode: 'edit', created: true })
      void load()
    } catch (cause) {
      setActionError(errorText(cause))
    } finally {
      setBusy(false)
    }
  }
  const create = async (from: string, label: string) => {
    if (!bridge) return
    setBusy(true)
    setCreateError(null)
    try {
      const created = await bridge.createStack(from, label)
      setStacks((current) => upsertStack(current, created))
      setCreating(false)
      setSheet({ id: created.id, mode: 'edit', created: true })
      void load()
    } catch (cause) {
      setCreateError(errorText(cause))
    } finally {
      setBusy(false)
    }
  }
  const remove = async (stack: Stack) => {
    if (!bridge) return
    setBusy(true)
    setActionError(null)
    try {
      await bridge.deleteStack(stack.id)
      // Its row is gone: focus goes to the page's heading.
      setStacks(
        (current) => current?.filter((entry) => entry.id !== stack.id) ?? null,
      )
      document.getElementById('sk-heading')?.focus()
      void load()
    } catch (cause) {
      setActionError(errorText(cause))
    } finally {
      setBusy(false)
    }
  }

  const failedFirstLoad = Boolean(error) && stacks === null
  const open = sheet ? stacks?.find((stack) => stack.id === sheet.id) : null
  return (
    <div className="ds-root ex-page sk-page" data-narrow={narrow || undefined}>
      <DashboardPageActions
        active="stacks"
        actionsLabel="Stack actions"
        actions={headerActions}
      />
      <header className="ex-header">
        <h1 id="sk-heading" tabIndex={-1}>
          Stacks
        </h1>
        {failedFirstLoad ? null : (
          <p>{stacks ? stacksSummary(stacks) : 'Loading the stacks…'}</p>
        )}
      </header>

      {error && !failedFirstLoad ? (
        <Callout tone="danger" title="The stacks could not be reloaded">
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
              try again
            </button>
          </span>
        </Callout>
      ) : null}
      {actionError && !open ? (
        <Callout tone="danger" title="That did not go through">
          {actionError}
        </Callout>
      ) : null}

      {stacks === null ? (
        failedFirstLoad ? (
          <EmptyState
            tone="error"
            title="Stacks could not be loaded"
            description={error}
            actions={
              <button
                className={buttonClassName({ variant: 'secondary' })}
                type="button"
                onClick={() => void load()}
              >
                try again
              </button>
            }
          />
        ) : (
          <div className="ex-loading" aria-busy="true" role="status">
            <span className="ds-visually-hidden">Loading the stacks</span>
            {Array.from({ length: 4 }, (_, index) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
              <div key={index} />
            ))}
          </div>
        )
      ) : (
        <StackList
          stacks={stacks}
          ready={Boolean(bridge)}
          busy={busy}
          onOpen={(stack) =>
            setSheet({
              id: stack.id,
              mode: stack.source === 'local' ? 'edit' : 'view',
            })
          }
          onCopy={(stack) => void copy(stack)}
          onDelete={setDeleting}
          onNew={openNew}
        />
      )}
      <ProviderCredentials bridge={bridge} narrow={narrow} />

      {open ? (
        <StackSheet
          key={`${open.id}:${sheet?.mode}`}
          stack={open}
          mode={sheet?.mode ?? 'view'}
          narrow={narrow}
          bridge={bridge}
          busy={busy}
          copyError={actionError}
          created={sheet?.created}
          onCopy={() => void copy(open)}
          onSaved={(saved) => {
            setStacks((current) => upsertStack(current, saved))
            void load()
          }}
          onClose={() => setSheet(null)}
        />
      ) : null}
      {creating && stacks ? (
        <NewStackDialog
          stacks={stacks}
          narrow={narrow}
          busy={busy}
          error={createError}
          onCreate={(from, label) => void create(from, label)}
          onClose={() => setCreating(false)}
        />
      ) : null}
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null)
        }}
        title={`Delete “${deleting?.label ?? 'this stack'}”?`}
        description="Only this Console’s copy goes, YAML and all. The repository’s stacks stay, and so does what past executions recorded about the stack they ran on. This can’t be undone."
        confirmLabel="Delete stack"
        tone="danger"
        onConfirm={() => deleting && void remove(deleting)}
      />
      <LocalRunnerDialog
        bridge={bridge}
        open={running}
        onClose={() => setRunning(false)}
      />
    </div>
  )
}
