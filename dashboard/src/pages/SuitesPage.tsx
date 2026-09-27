import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@iii-dev/console-ui'
import { ChevronLeft, Copy, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  DashboardPageActions,
  dashboardHeaderActionClassName,
  type HeaderAction,
} from '@/components/DashboardPageActions'
import { useDashboardChrome } from '@/components/DashboardShell'
import { LocalRunnerDialog } from '@/components/LocalRunnerDialog'
import {
  buttonClassName,
  Callout,
  EmptyState,
  FactChip,
  FactList,
  StatusLabel,
} from '@/design-system'
import {
  hashForSuites,
  hashForTests,
  routeParams,
} from '@/hooks/use-hash-route'
import { useLatestRequest } from '@/hooks/use-latest-request'
import {
  type DashboardDataBridge,
  type DashboardExecutionSummary,
  getDashboardDataBridge,
  type Suite,
} from '@/lib/dashboard-data-source'
import {
  type SuiteListItem,
  type SuiteRun,
  suiteDigest,
  suiteHolds,
  suiteListItem,
  suiteRuns,
  suitesSummary,
} from '@/lib/suites-view'
import type { TestCatalogRow } from '@/lib/test-catalog'
import { type CatalogRowView, catalogRowView } from '@/lib/test-catalog-view'
import { listAllTests } from '@/pages/TestsCatalogPage'
import '@/design-system/styles.css'
import './executions-page.css'
import './tests-catalog.css'
import './suites-page.css'

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** The suite the hash opens: `#/suites?suite=<id>`. */
function suiteParam() {
  return typeof window === 'undefined'
    ? null
    : routeParams(window.location.hash).get('suite')
}

/** The two blocks of the list: the repository's suites, read-only, then this
 *  Console's, which say how to make one while there are none. */
export function SuiteList({
  suites,
  items,
  selectedId,
}: {
  suites: Suite[]
  items: ReadonlyMap<string, SuiteListItem>
  selectedId: string | null
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
              >
                <span className="st-item-head">
                  <span className="st-item-label">{item.label}</span>
                  <span className="st-item-count">{item.count}</span>
                </span>
                <StatusLabel
                  className="st-item-last"
                  state={item.last?.state ?? 'never_run'}
                  label={item.last?.label ?? 'not run in this Console'}
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

export type SuiteDetailProps = {
  suite: Suite
  runs: SuiteRun[]
  narrow: boolean
  /** The bridge answered: the actions can run. */
  ready: boolean
  /** An action on this suite is under way. */
  busy: boolean
  error: string | null
  onCopy: () => void
  onRun: () => void
  onDelete: () => void
}

/** The open suite: its name, facts and purpose, and its actions. */
export function SuiteDetail({
  suite,
  narrow,
  ready,
  busy,
  error,
  onCopy,
  onRun,
  onDelete,
}: SuiteDetailProps) {
  const local = suite.source === 'local'
  return (
    <section
      className="st-detail"
      aria-labelledby="st-title"
      data-suite-detail={suite.id}
    >
      {narrow ? (
        <a className="st-back" href={hashForSuites()}>
          <ChevronLeft size={16} aria-hidden="true" />
          Suites
        </a>
      ) : null}
      <div className="st-head">
        <div className="st-head-main">
          <h2 id="st-title" className="st-title">
            {suite.label}
          </h2>
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
                suite.scenarios.length,
                suite.repetitions,
                suite.technical_retries,
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
          <button
            type="button"
            className={dashboardHeaderActionClassName({ primary: true })}
            disabled={!ready}
            onClick={onRun}
          >
            Run this suite
          </button>
          {local ? (
            <button
              type="button"
              className={dashboardHeaderActionClassName({
                className: 'harness-e2e-header-action-icon',
              })}
              aria-label={`Delete ${suite.label}`}
              title="Delete suite"
              disabled={!ready || busy}
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
    </section>
  )
}

function DeleteSuiteDialog({
  suite,
  deleting,
  onCancel,
  onConfirm,
}: {
  suite: Suite | null
  deleting: boolean
  onCancel: () => void
  onConfirm: () => void
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
  const [error, setError] = useState<string | null>(null)
  const [param, setParam] = useState<string | null>(suiteParam)
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [deleting, setDeleting] = useState<Suite | null>(null)
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
        next
          .listExecutions({ limit: 100 })
          .then((manifest) => manifest.executions ?? [])
          .catch(() => [] as DashboardExecutionSummary[]),
      ])
      if (!request.isCurrent()) return
      setSuites(listed.suites)
      if (tests) setRows(tests.rows)
      setExecutions(recent)
    } catch (cause) {
      if (request.isCurrent()) setError(errorText(cause))
    }
  }, [beginRequest])

  useEffect(() => {
    void load()
  }, [load])

  // A finished run changes a suite's last execution and its tests' results.
  useEffect(() => {
    if (!bridge) return
    let cancelled = false
    let dispose: (() => void) | undefined
    let timer: number | undefined
    bridge
      .subscribeRunChanges(() => {
        if (timer) window.clearTimeout(timer)
        timer = window.setTimeout(() => void load(), 400)
      })
      .then((off) => {
        if (cancelled) off()
        else dispose = off
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
      if (timer) window.clearTimeout(timer)
      dispose?.()
    }
  }, [bridge, load])

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
          suiteListItem(suite, runsBySuite.get(suite.id) ?? [], views),
        ]),
      ),
    [suites, runsBySuite, views],
  )

  const named = suites?.find((suite) => suite.id === param) ?? null
  // Wide, a suite is always open (the first by default); narrow, the list
  // shows until one is picked.
  const selected = named ?? (narrow ? null : (suites?.[0] ?? null))
  const failedFirstLoad = Boolean(error) && suites === null

  const headerActions = useMemo<HeaderAction[]>(
    () =>
      bridge
        ? [
            {
              id: 'run',
              label: 'Run tests',
              primary: true,
              onSelect: () => openRunner(''),
            },
          ]
        : [],
    [bridge, openRunner],
  )

  const copy = async (suite: Suite) => {
    if (!bridge) return
    setBusy(true)
    setActionError(null)
    try {
      const created = await bridge.createSuite(suite.id)
      await load()
      window.location.hash = hashForSuites(created.id)
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
      setDeleting(null)
      await load()
      window.location.hash = hashForSuites()
    } catch (cause) {
      setDeleting(null)
      setActionError(errorText(cause))
    } finally {
      setBusy(false)
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
        <h1>Suites</h1>
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
              try again
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
                try again
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
          {narrow && selected ? null : (
            <SuiteList
              suites={suites}
              items={items}
              selectedId={selected?.id ?? null}
            />
          )}
          {selected ? (
            <SuiteDetail
              key={selected.id}
              suite={selected}
              runs={runsBySuite.get(selected.id) ?? []}
              narrow={narrow}
              ready={Boolean(bridge)}
              busy={busy}
              error={actionError}
              onCopy={() => void copy(selected)}
              onRun={() => openRunner(selected.id)}
              onDelete={() => setDeleting(selected)}
            />
          ) : null}
        </div>
      )}

      <DeleteSuiteDialog
        suite={deleting}
        deleting={busy}
        onCancel={() => setDeleting(null)}
        onConfirm={() => deleting && void remove(deleting)}
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
