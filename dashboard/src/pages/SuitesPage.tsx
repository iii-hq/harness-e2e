import { useCallback, useEffect, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import {
  ExecutionSetup,
  ExecutionSetupFooter,
  focusFirstInvalid,
  validateExecutionSetup,
} from '@/components/ExecutionSetup'
import {
  asCatalog,
  type RunnerCatalog,
  withSequentialGroups,
} from '@/components/LocalRunnerDialog'
import {
  buttonClassName,
  DataTable,
  Dialog,
  EmptyState,
  PageHeader,
} from '@/design-system'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
  type Suite,
} from '@/lib/dashboard-data-source'

function errorText(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

function plural(count: number, one: string, many: string) {
  return `${count} ${count === 1 ? one : many}`
}

/** What a suite holds, in one line. */
function suiteScope(suite: Suite) {
  return [
    plural(suite.scenarios.length, 'test', 'tests'),
    `${plural(suite.repetitions, 'run', 'runs')} each`,
    plural(suite.technical_retries, 'retry', 'retries'),
  ].join(' · ')
}

type Draft = {
  label: string
  scenarios: string[]
  runs: string
  technicalRetries: string
}

/** Edits a suite of this Console: its name, tests, runs and retries. */
function SuiteEditor({
  bridge,
  suite,
  onClose,
  onSaved,
}: {
  bridge: DashboardDataBridge
  suite: Suite
  onClose: () => void
  onSaved: () => void
}) {
  const [draft, setDraft] = useState<Draft>({
    label: suite.label,
    scenarios: suite.scenarios,
    runs: String(suite.repetitions),
    technicalRetries: String(suite.technical_retries),
  })
  const [catalog, setCatalog] = useState<RunnerCatalog | null>(null)
  const [loading, setLoading] = useState(false)
  const [query, setQuery] = useState('')
  const [attempted, setAttempted] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const loadCatalog = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setCatalog(asCatalog(await bridge.getCatalog()))
    } catch (cause) {
      setCatalog(null)
      setError(errorText(cause))
    } finally {
      setLoading(false)
    }
  }, [bridge])
  useEffect(() => {
    void loadCatalog()
  }, [loadCatalog])

  const listed = catalog?.scenarios ?? []
  // Without the catalog the suite's own tests stay listed.
  const available = [
    ...listed,
    ...suite.scenarios.filter((id) => !listed.includes(id)),
  ]
  const validation = () =>
    validateExecutionSetup({
      mode: 'suite',
      label: draft.label,
      selectedScenarios: draft.scenarios,
    })
  const errors = attempted ? validation() : {}
  const update = <K extends keyof Draft>(key: K, value: Draft[K]) =>
    setDraft((current) => ({ ...current, [key]: value }))
  const save = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const pending = validation()
    if (Object.keys(pending).length > 0) {
      setAttempted(true)
      focusFirstInvalid('suite-editor', pending)
      return
    }
    setSaving(true)
    setError(null)
    try {
      await bridge.updateSuite(suite.id, {
        label: draft.label.trim(),
        scenarios: draft.scenarios,
        repetitions: Math.max(1, Number(draft.runs) || 1),
        technical_retries: Math.max(0, Number(draft.technicalRetries) || 0),
      })
      onSaved()
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setSaving(false)
    }
  }
  const runsPerScenario = Math.max(1, Number(draft.runs) || 1)
  const technicalRetries = Math.max(0, Number(draft.technicalRetries) || 0)
  return (
    <Dialog
      open
      onClose={() => !saving && onClose()}
      size="lg"
      tall
      kicker="Suite"
      title={`Edit ${suite.label}`}
      description="A suite is only what to test. The model and the stack are chosen when it runs."
      closeLabel="Close suite editor"
      className="ds-root"
      footer={
        <ExecutionSetupFooter
          summary={{
            mode: 'suite',
            selectedScenarios: draft.scenarios.length,
            runsPerScenario,
            technicalRetries,
          }}
          pending={Object.values(errors)}
          error={error}
        >
          <button
            className={buttonClassName({ variant: 'secondary' })}
            type="button"
            onClick={onClose}
            disabled={saving}
          >
            cancel
          </button>
          <button
            className={buttonClassName({ variant: 'primary' })}
            type="submit"
            form="suite-editor-form"
            disabled={saving}
            aria-busy={saving}
          >
            {saving ? 'saving…' : 'save suite'}
          </button>
        </ExecutionSetupFooter>
      }
    >
      <form
        id="suite-editor-form"
        className="grid min-w-0 gap-6"
        onSubmit={save}
        noValidate
      >
        <ExecutionSetup
          idPrefix="suite-editor"
          mode="suite"
          stickyOffset="dialog"
          initialOnlySelected
          label={draft.label}
          availableScenarios={available}
          selectedScenarios={draft.scenarios}
          query={query}
          runs={draft.runs}
          technicalRetries={draft.technicalRetries}
          disabled={saving}
          catalogLoading={loading}
          catalogStatus={
            loading
              ? { tone: 'loading', text: 'loading catalog…' }
              : catalog
                ? {
                    tone: 'ready',
                    text: `catalog ready · ${plural(catalog.scenarios.length, 'test', 'tests')}`,
                  }
                : { tone: 'unavailable', text: 'catalog unavailable' }
          }
          errors={errors}
          onRefreshCatalog={() => void loadCatalog()}
          onLabelChange={(value) => update('label', value)}
          onSelectedScenariosChange={(value) =>
            update(
              'scenarios',
              withSequentialGroups(
                value,
                draft.scenarios,
                catalog?.groups ?? [],
              ),
            )
          }
          onQueryChange={setQuery}
          onRunsChange={(value) => update('runs', value)}
          onTechnicalRetriesChange={(value) =>
            update('technicalRetries', value)
          }
        />
      </form>
    </Dialog>
  )
}

/** Suites: the master plan's, read-only, and this Console's. Any suite can
 *  be copied into one of this Console to edit. */
export function SuitesPage() {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [suites, setSuites] = useState<Suite[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [editing, setEditing] = useState<Suite | null>(null)
  const [deleting, setDeleting] = useState<Suite | null>(null)

  const load = useCallback(async () => {
    try {
      const next = await getDashboardDataBridge()
      setBridge(next)
      const listed = (await next.listSuites()).suites
      setSuites(listed)
      setError(null)
      return listed
    } catch (cause) {
      setError(errorText(cause))
      return null
    }
  }, [])
  useEffect(() => {
    void load()
  }, [load])

  const copy = async (suite: Suite) => {
    if (!bridge) return
    setBusy(suite.id)
    setError(null)
    try {
      const created = await bridge.createSuite(suite.id)
      await load()
      setEditing(created)
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setBusy(null)
    }
  }
  const remove = async (suite: Suite) => {
    if (!bridge) return
    setBusy(suite.id)
    setError(null)
    try {
      await bridge.deleteSuite(suite.id)
      setDeleting(null)
      await load()
    } catch (cause) {
      setError(errorText(cause))
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="ds-root min-h-dvh bg-canvas text-ink">
      <DashboardPageActions active="suites" />
      <div className="page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <PageHeader
          title="suites"
          summary="What to test: scenarios, runs of each and technical retries. The repository's suites are read-only; copy one to edit it here."
        />
        {error ? (
          <p className="mt-4 text-sm text-danger" role="alert">
            {error}
          </p>
        ) : null}
        {suites === null ? (
          error ? (
            <EmptyState
              className="mt-6"
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
            <div className="mt-6 grid gap-2" aria-busy="true" role="status">
              <span className="ds-visually-hidden">Loading suites</span>
              {['first', 'second', 'third'].map((placeholder) => (
                <div
                  key={placeholder}
                  className="h-12 animate-pulse rounded-[6px] bg-[var(--surface-fill)] motion-reduce:animate-none"
                />
              ))}
            </div>
          )
        ) : (
          <div className="mt-6" data-suites>
            <DataTable caption="Suites" collapse minWidth="52rem">
              <thead>
                <tr>
                  <th scope="col">Suite</th>
                  <th scope="col">Source</th>
                  <th scope="col">Holds</th>
                  <th scope="col">Digest</th>
                  <th scope="col">
                    <span className="ds-visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {suites.map((suite) => (
                  <tr key={suite.id} data-suite={suite.id}>
                    <td data-label="Suite">
                      <span className="grid gap-0.5">
                        <strong className="text-sm font-semibold text-ink">
                          {suite.label}
                        </strong>
                        <span className="font-mono text-xs text-ink-muted">
                          {suite.id}
                        </span>
                        {suite.purpose ? (
                          <span className="max-w-[32rem] text-xs leading-5 text-ink-soft">
                            {suite.purpose}
                          </span>
                        ) : null}
                      </span>
                    </td>
                    <td data-label="Source" className="text-xs text-ink-soft">
                      {suite.source === 'local' ? 'this Console' : 'repository'}
                    </td>
                    <td data-label="Holds" className="text-xs text-ink">
                      {suiteScope(suite)}
                    </td>
                    <td
                      data-label="Digest"
                      className="font-mono text-xs text-ink-muted"
                      title={suite.sha256 ?? undefined}
                    >
                      {suite.sha256
                        ? suite.sha256.replace(/^sha256:/, '').slice(0, 12)
                        : '—'}
                    </td>
                    <td className="text-right">
                      <span className="inline-flex flex-wrap justify-end gap-2">
                        <button
                          className={buttonClassName({
                            variant: 'secondary',
                            size: 'compact',
                          })}
                          type="button"
                          aria-label={`Copy ${suite.label}`}
                          disabled={!bridge || busy !== null}
                          onClick={() => void copy(suite)}
                        >
                          copy
                        </button>
                        {suite.source === 'local' ? (
                          <>
                            <button
                              className={buttonClassName({
                                variant: 'secondary',
                                size: 'compact',
                              })}
                              type="button"
                              aria-label={`Edit ${suite.label}`}
                              disabled={!bridge || busy !== null}
                              onClick={() => setEditing(suite)}
                            >
                              edit
                            </button>
                            <button
                              className={buttonClassName({
                                variant: 'quiet',
                                size: 'compact',
                              })}
                              type="button"
                              aria-label={`Delete ${suite.label}`}
                              disabled={!bridge || busy !== null}
                              onClick={() => setDeleting(suite)}
                            >
                              delete
                            </button>
                          </>
                        ) : null}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          </div>
        )}
      </div>
      {editing && bridge ? (
        <SuiteEditor
          key={editing.id}
          bridge={bridge}
          suite={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null)
            void load()
          }}
        />
      ) : null}
      <Dialog
        open={deleting !== null}
        onClose={() => busy === null && setDeleting(null)}
        size="sm"
        title={`Delete ${deleting?.label ?? 'suite'}?`}
        description="Executions that ran it keep its name and digest."
        footer={
          <div className="flex justify-end gap-2">
            <button
              type="button"
              className={buttonClassName({ variant: 'secondary' })}
              disabled={busy !== null}
              onClick={() => setDeleting(null)}
            >
              cancel
            </button>
            <button
              type="button"
              className={buttonClassName({ variant: 'primary' })}
              disabled={busy !== null}
              aria-busy={busy !== null}
              onClick={() => deleting && void remove(deleting)}
            >
              {busy !== null ? 'deleting…' : 'delete suite'}
            </button>
          </div>
        }
      />
    </div>
  )
}
