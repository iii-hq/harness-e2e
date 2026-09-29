import { ConfirmDialog } from '@iii-dev/console-ui'
import { Copy, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
  type HeaderAction,
} from '@/components/DashboardPageActions'
import { useDashboardChrome } from '@/components/DashboardShell'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import { ProviderCredentials } from '@/components/ProviderCredentials'
import { StackSheet } from '@/components/stacks/StackSheet'
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
import {
  BASE_STACK,
  pinOf,
  stackDiff,
  stackSub,
  stacksSummary,
  warningsTitle,
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
      title:
        'A stack of this Console, from a template, a copy or the workers you pick',
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
          disabled={!ready}
          aria-disabled={busy || undefined}
          onClick={() => {
            if (!busy) onCopy(stack)
          }}
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
            disabled={!ready}
            aria-disabled={busy || undefined}
            onClick={() => {
              if (!busy) onDelete(stack)
            }}
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
  // The stack open in the sheet, as last seen, and whether it was just
  // created; a new one has none yet. Kept so a reload that no longer lists
  // it does not close it.
  const [sheet, setSheet] = useState<
    | { mode: 'new' }
    | { mode: 'view' | 'edit'; id: string; stack: Stack; created?: boolean }
    | null
  >(null)
  const [deleting, setDeleting] = useState<Stack | null>(null)
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
    setActionError(null)
    setSheet({ mode: 'new' })
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
      setSheet({ id: created.id, mode: 'edit', stack: created, created: true })
      void load()
    } catch (cause) {
      setActionError(errorText(cause))
    } finally {
      setBusy(false)
    }
  }
  // A delete in flight: a second confirmation of it is ignored.
  const removing = useRef(false)
  const remove = async (stack: Stack) => {
    if (!bridge || removing.current) return
    removing.current = true
    setDeleting(null)
    setBusy(true)
    setActionError(null)
    try {
      await bridge.deleteStack(stack.id)
      // Its row is gone, and its sheet if it was open: focus goes to the
      // page's heading.
      setStacks(
        (current) => current?.filter((entry) => entry.id !== stack.id) ?? null,
      )
      setSheet((current) =>
        current && 'id' in current && current.id === stack.id ? null : current,
      )
      document.getElementById('sk-heading')?.focus()
      void load()
    } catch (cause) {
      setActionError(errorText(cause))
    } finally {
      removing.current = false
      setBusy(false)
    }
  }

  const failedFirstLoad = Boolean(error) && stacks === null
  const listed =
    sheet && 'id' in sheet
      ? stacks?.find((stack) => stack.id === sheet.id)
      : undefined
  const open = sheet && 'id' in sheet ? (listed ?? sheet.stack) : null
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
      {actionError && !sheet ? (
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
          onOpen={(stack) => {
            // What an earlier action said belongs to that action.
            setActionError(null)
            setSheet({
              id: stack.id,
              mode: stack.source === 'local' ? 'edit' : 'view',
              stack,
            })
          }}
          onCopy={(stack) => void copy(stack)}
          onDelete={setDeleting}
          onNew={openNew}
        />
      )}
      <ProviderCredentials bridge={bridge} narrow={narrow} />

      {sheet && stacks && (sheet.mode === 'new' || open) ? (
        <StackSheet
          key={open ? `${sheet.mode}:${open.id}` : 'new'}
          mode={sheet.mode}
          stack={open}
          stacks={stacks}
          narrow={narrow}
          bridge={bridge}
          busy={busy}
          copyError={actionError}
          created={'created' in sheet ? sheet.created : false}
          gone={sheet.mode === 'edit' && !listed}
          onCreated={(made) => {
            setStacks((current) => upsertStack(current, made))
            setSheet({ mode: 'edit', id: made.id, stack: made, created: true })
            void load()
          }}
          onSaved={(saved) => {
            setStacks((current) => upsertStack(current, saved))
            setSheet((current) =>
              current && 'id' in current && current.id === saved.id
                ? { ...current, stack: saved }
                : current,
            )
            void load()
          }}
          onCopy={() => open && void copy(open)}
          onDelete={() => open && setDeleting(open)}
          onClose={() => setSheet(null)}
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
