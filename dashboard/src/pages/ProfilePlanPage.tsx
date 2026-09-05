import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { DashboardPageActions } from '@/components/DashboardPageActions'
import { MasterTestProfiles } from '@/components/MasterTestProfiles'
import { ProviderModelDropdown } from '@/components/ProviderModelDropdown'
import {
  buttonClassName,
  Callout,
  DataTable,
  Field,
  Input,
  PageHeader,
  Panel,
} from '@/design-system'
import {
  hashForExecution,
  hashForNewPlan,
  hashForPlan,
  hashForPlans,
} from '@/hooks/use-hash-route'
import {
  type DashboardDataBridge,
  getDashboardDataBridge,
  type MasterTestPlan,
  type MasterTestProfile,
} from '@/lib/dashboard-data-source'
import {
  downloadJson,
  type PlanAdmission,
  type PlanConfiguration,
  type PlanExecution,
  type PlanRequirements,
  type ProfileComparison,
  type ProfilePlan,
  planErrors,
  profileAction,
  running,
} from '@/lib/profile-plan'

const message = (error: unknown) =>
  error instanceof Error ? error.message : String(error)
const blank: PlanConfiguration = {
  label: '',
  profile_id: '',
  url: '',
  model: '',
  provider: '',
  judge_model: '',
  judge_provider: '',
}
const secondary = buttonClassName({ variant: 'secondary' })
const primary = buttonClassName({ variant: 'primary' })

export function ProfilePlanShell({
  title,
  summary,
  children,
}: {
  title: string
  summary: string
  children: React.ReactNode
}) {
  return (
    <>
      <DashboardPageActions active="plans" />
      <div className="ds-root page-shell w-[calc(100%_-_1.5rem)] max-w-[1420px] pt-5 pb-16 md:w-[calc(100%_-_3rem)]">
        <a
          className="text-xs text-ink-soft hover:underline"
          href={hashForPlans()}
        >
          My plans
        </a>
        <PageHeader title={title} summary={summary} />
        <div className="mt-5 grid gap-5">{children}</div>
      </div>
    </>
  )
}

export function Requirements({ value }: { value: PlanRequirements }) {
  const active = value.active_execution
  return (
    <Panel aria-label="Execution requirements">
      <h2 className="mt-0 text-sm font-semibold text-ink">
        Execution requirements
      </h2>
      {active ? (
        <Callout tone="warning" title="Another execution is active">
          Your saved draft is preserved.{' '}
          <a
            className="underline"
            href={
              active.plan_id
                ? hashForPlan(active.plan_id)
                : hashForExecution(active.id)
            }
          >
            Follow active execution
          </a>
        </Callout>
      ) : null}
      <ul className="m-0 grid gap-2 pl-5 text-xs leading-5 text-ink-soft">
        {value.checks.map((check) => (
          <li key={check.id}>
            <strong
              className={
                check.status === 'blocked' ? 'text-danger' : 'text-ink'
              }
            >
              {check.status === 'pending'
                ? 'Pending'
                : check.status === 'blocked'
                  ? 'Blocked'
                  : 'Ready'}
            </strong>{' '}
            · {check.message}
          </li>
        ))}
      </ul>
    </Panel>
  )
}

function Coverage({
  profile,
  scenarios,
  version,
  digest,
}: {
  profile: MasterTestProfile
  scenarios: string[]
  version?: number
  digest?: string
}) {
  return (
    <Panel>
      <p className="mt-0 text-sm text-ink">
        {scenarios.length} scenarios · {profile.repetitions} repetition
        {profile.repetitions === 1 ? '' : 's'} ·{' '}
        {profile.budget?.planned_runs ?? scenarios.length * profile.repetitions}{' '}
        planned slots
      </p>
      <details className="text-xs leading-6 text-ink-soft">
        <summary className="cursor-pointer text-ink">
          Coverage and technical configuration
        </summary>
        <p>{scenarios.join(', ')}</p>
        <p>
          Up to {profile.technical_retries} technical retries per replay-safe
          scenario. The profile owns the cases, seeds and execution limits.
        </p>
        <p>{profile.metrics.join(' · ')}</p>
        {version ? <p>Profile revision {version}</p> : null}
        {digest ? <p className="break-all font-mono">{digest}</p> : null}
      </details>
    </Panel>
  )
}

export function ProfilePlanCreatePage({
  profileId,
  duplicateId,
  editId,
}: {
  profileId?: string
  duplicateId?: string
  editId?: string
}) {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [master, setMaster] = useState<MasterTestPlan | null>(null)
  const [source, setSource] = useState<ProfilePlan | null>(null)
  const [models, setModels] = useState<
    Array<{ model: string; provider: string }>
  >([])
  const [configuration, setConfiguration] = useState<PlanConfiguration>(blank)
  const [errors, setErrors] = useState<Record<string, string>>({})
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [loaded, setLoaded] = useState(false)
  const [requirements, setRequirements] = useState<PlanRequirements | null>(
    null,
  )
  const [saved, setSaved] = useState<ProfilePlan | null>(null)
  const draftKey = `harness-profile-draft:${editId ?? duplicateId ?? profileId ?? 'chooser'}`
  const startKey = useRef<string | null>(null)
  const formRef = useRef<HTMLFormElement>(null)
  const profile = useMemo(
    () =>
      source
        ? {
            ...source.snapshot.profile,
            budget: source.snapshot.budget,
            judge_required:
              source.snapshot.protected_supervisor_required ||
              source.snapshot.cases.some((c) => c.judge_required),
            protected_supervisor_required:
              source.snapshot.protected_supervisor_required,
          }
        : master?.profiles.find((p) => p.id === profileId),
    [source, master, profileId],
  )
  const judgeRequired = profile?.judge_required ?? false
  useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const next = await getDashboardDataBridge()
        const [plans, catalog, original] = await Promise.all([
          next.listPlans(),
          next.getCatalog(),
          duplicateId || editId
            ? profileAction<ProfilePlan>(next, {
                action: 'get',
                plan_id: duplicateId ?? editId,
              })
            : Promise.resolve(null),
        ])
        if (!alive) return
        setBridge(next)
        setMaster(plans.master_plan ?? null)
        setSource(original)
        setModels(
          (catalog.models ?? []) as Array<{ model: string; provider: string }>,
        )
        let initial = original
          ? {
              ...original.configuration,
              label: editId
                ? original.configuration.label
                : `${original.configuration.label} (copy)`,
              model: editId ? original.configuration.model : '',
              provider: editId ? original.configuration.provider : '',
            }
          : {
              ...blank,
              profile_id: profileId ?? '',
              label:
                plans.master_plan?.profiles.find((p) => p.id === profileId)
                  ?.label ?? '',
              url: String(catalog.url ?? ''),
            }
        const retained = sessionStorage.getItem(draftKey)
        if (retained) {
          try {
            initial = { ...initial, ...JSON.parse(retained) }
          } catch {
            /* Ignore an obsolete browser draft. */
          }
        }
        setConfiguration(initial)
        setLoaded(true)
      } catch (cause) {
        if (alive) setError(message(cause))
      }
    })()
    return () => {
      alive = false
    }
  }, [duplicateId, editId, profileId, draftKey])
  useEffect(() => {
    if (loaded && profile)
      sessionStorage.setItem(draftKey, JSON.stringify(configuration))
  }, [configuration, draftKey, loaded, profile])
  useEffect(() => {
    if (
      !bridge ||
      !profile ||
      Object.keys(planErrors(configuration, judgeRequired)).length
    )
      return
    let alive = true
    const timer = setTimeout(() => {
      void profileAction<PlanRequirements>(bridge, {
        action: 'requirements',
        configuration,
      })
        .then((value) => {
          if (alive) setRequirements(value)
        })
        .catch((cause) => {
          if (alive) setError(message(cause))
        })
    }, 500)
    return () => {
      alive = false
      clearTimeout(timer)
    }
  }, [bridge, configuration, judgeRequired, profile])
  const groups = useMemo(
    () =>
      [...new Set(models.map((m) => m.provider))].map((provider) => ({
        provider,
        models: models
          .filter((m) => m.provider === provider)
          .map((m) => m.model),
      })),
    [models],
  )
  const change = (values: Partial<PlanConfiguration>) => {
    setConfiguration((current) => ({ ...current, ...values }))
    setErrors({})
    setRequirements(null)
    setError('')
  }
  async function save(run: boolean) {
    if (!bridge || !profile) return
    const validation = planErrors(configuration, judgeRequired)
    setErrors(validation)
    if (Object.keys(validation).length) {
      document.getElementById(`profile-${Object.keys(validation)[0]}`)?.focus()
      return
    }
    setBusy(true)
    setError('')
    try {
      const plan = saved
        ? await profileAction<ProfilePlan>(bridge, {
            action: 'update',
            plan_id: saved.id,
            configuration,
          })
        : duplicateId
          ? await profileAction<ProfilePlan>(bridge, {
              action: 'duplicate',
              plan_id: duplicateId,
              label: configuration.label,
              model: configuration.model,
              provider: configuration.provider,
            })
          : await profileAction<ProfilePlan>(
              bridge,
              editId
                ? { action: 'update', plan_id: editId, configuration }
                : { action: 'create', configuration },
            )
      setSaved(plan)
      if (run) {
        startKey.current ??= crypto.randomUUID()
        const admission = await profileAction<PlanAdmission>(bridge, {
          action: 'start',
          plan_id: plan.id,
          idempotency_key: startKey.current,
          role:
            plan.configuration.profile_id === 'evolution' ? 'baseline' : 'run',
        })
        if (admission.blocked) {
          setRequirements(admission.requirements ?? null)
          return
        }
      }
      sessionStorage.removeItem(draftKey)
      window.location.hash = hashForPlan(plan.id)
    } catch (cause) {
      setError(message(cause))
    } finally {
      setBusy(false)
    }
  }
  if (!profileId && !duplicateId && !editId)
    return (
      <ProfilePlanShell
        title="New plan"
        summary="Choose what you want to evaluate, then configure one execution model."
      >
        {error ? (
          <Callout tone="warning" title="Profiles unavailable">
            {error}
          </Callout>
        ) : master ? (
          <MasterTestProfiles plan={master} />
        ) : (
          <p role="status">Loading profiles…</p>
        )}
        <a className={secondary} href={`${hashForNewPlan()}/manual`}>
          Create a custom plan manually
        </a>
      </ProfilePlanShell>
    )
  return (
    <ProfilePlanShell
      title={
        duplicateId
          ? 'Duplicate plan'
          : editId
            ? 'Edit draft'
            : 'Configure plan'
      }
      summary={profile?.purpose ?? 'Loading the selected profile…'}
    >
      {error ? (
        <Callout tone="warning" title="Plan could not be saved">
          {error}
        </Callout>
      ) : null}
      {profile && loaded ? (
        <form
          ref={formRef}
          className="grid gap-5"
          onSubmit={(event) => {
            event.preventDefault()
            void save(false)
          }}
          noValidate
        >
          <Panel>
            <div className="grid gap-5 md:grid-cols-2">
              <div className="md:col-span-2">
                <Field
                  label="Plan name"
                  htmlFor="profile-label"
                  error={errors.label}
                >
                  <Input
                    id="profile-label"
                    value={configuration.label}
                    maxLength={160}
                    aria-invalid={!!errors.label}
                    aria-describedby={
                      errors.label ? 'profile-label-error' : undefined
                    }
                    onChange={(event) => change({ label: event.target.value })}
                  />
                </Field>
              </div>
              <Field
                label="Execution model"
                htmlFor="profile-model"
                error={errors.model}
              >
                <ProviderModelDropdown
                  id="profile-model"
                  invalid={!!errors.model}
                  describedBy={errors.model ? 'profile-model-error' : undefined}
                  groups={groups}
                  value={
                    configuration.model
                      ? `${configuration.provider}\n${configuration.model}`
                      : ''
                  }
                  onChange={(value) => {
                    const [provider, model] = value.split('\n')
                    change({ provider, model })
                  }}
                  ariaLabel="Execution model"
                  placeholder="Select an execution model"
                  required
                />
              </Field>
              <Field
                label={
                  judgeRequired
                    ? 'Evaluator (required)'
                    : 'Evaluator (optional)'
                }
                htmlFor="profile-judge"
                error={errors.judge}
              >
                <ProviderModelDropdown
                  id="profile-judge"
                  invalid={!!errors.judge}
                  describedBy={errors.judge ? 'profile-judge-error' : undefined}
                  groups={groups}
                  value={
                    configuration.judge_model
                      ? `${configuration.judge_provider}\n${configuration.judge_model}`
                      : ''
                  }
                  onChange={(value) => {
                    const [judge_provider = '', judge_model = ''] =
                      value.split('\n')
                    change({ judge_provider, judge_model })
                  }}
                  ariaLabel="Evaluator"
                  placeholder="Select an evaluator"
                  required={judgeRequired}
                  disabled={!!duplicateId}
                  clearLabel={judgeRequired ? undefined : 'No evaluator'}
                />
              </Field>
            </div>
            {duplicateId ? (
              <p className="mb-0 text-xs text-ink-soft">
                The copy preserves coverage, evaluator and policy. Select its
                execution model. It starts with an empty history.
              </p>
            ) : null}
          </Panel>
          <Coverage
            profile={profile}
            scenarios={source?.snapshot.scenario_ids ?? profile.scenario_ids}
            version={source?.snapshot.version ?? master?.version}
            digest={source?.snapshot.profile_sha256 ?? profile.profile_sha256}
          />
          {profile.protected_supervisor_required ? (
            <Callout title="Protected executor required" tone="warning">
              Save this Resilience plan to export it for Release Control.
              Protected fault execution is unavailable in this dashboard.
            </Callout>
          ) : null}
          {requirements ? <Requirements value={requirements} /> : null}
          {saved ? (
            <p className="text-sm text-ink-soft">
              Draft saved.{' '}
              <a className="underline" href={hashForPlan(saved.id)}>
                Open saved plan
              </a>
            </p>
          ) : null}
          <div className="flex flex-wrap gap-3">
            <button className={secondary} type="submit" disabled={busy}>
              {busy ? 'Saving…' : 'Save draft'}
            </button>
            {!profile.protected_supervisor_required ? (
              <button
                className={primary}
                type="button"
                disabled={busy}
                onClick={() => void save(true)}
              >
                Save and run
              </button>
            ) : null}
            <a className={secondary} href={hashForPlans()}>
              Back to plans
            </a>
          </div>
        </form>
      ) : (
        <p role="status">Loading configuration…</p>
      )}
    </ProfilePlanShell>
  )
}

export function PlanProgress({ execution }: { execution: PlanExecution }) {
  const planned = execution.slots.length
  const finished = execution.slots.filter((s) => s.state === 'finished').length
  const observed = execution.slots.reduce((sum, s) => sum + s.observed, 0)
  const active = execution.slots.find(
    (s) => s.state === 'running' || s.state === 'admitting',
  )
  const total = (field: 'passed' | 'completed' | 'technical_valid') =>
    execution.slots.reduce((sum, s) => sum + s[field], 0)
  return (
    <Panel aria-label="Plan execution progress">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="my-0 text-sm font-semibold text-ink">
          {execution.role === 'run'
            ? 'Execution'
            : execution.role === 'baseline'
              ? 'Reference execution'
              : 'Candidate execution'}{' '}
          · {execution.state}
        </h2>
        <span className="text-xs text-ink-soft">
          {finished} / {planned} slots finished
        </span>
      </div>
      <progress
        className="mt-4 h-2 w-full accent-current text-ink"
        value={finished}
        max={planned || 1}
        aria-label="Finished planned slots"
      />
      <p className="text-xs text-ink-soft" aria-live="polite">
        {active
          ? `Round ${active.round} · ${active.scenario_id}`
          : running(execution.state)
            ? 'Preparing the next scenario…'
            : `${planned - observed} slots without observations. ${execution.state === 'interrupted' ? 'Run again starts a new complete execution.' : ''}`}
      </p>
      <dl className="grid grid-cols-2 gap-4 text-xs md:grid-cols-4">
        {[
          ['Execution completion', total('completed')],
          ['Objective correctness', total('passed')],
          ['Technical validity', total('technical_valid')],
          ['Observation coverage', observed],
        ].map(([label, value]) => (
          <div key={label}>
            <dt className="text-ink-soft">{label}</dt>
            <dd className="mx-0 mt-1 text-lg font-semibold text-ink">
              {value} / {planned}
            </dd>
          </div>
        ))}
      </dl>
      {execution.error ? (
        <Callout tone="warning" title="Execution evidence">
          {execution.error}
        </Callout>
      ) : null}
    </Panel>
  )
}

export function ProfilePlanDetailPage({
  planId,
  initialExecutionId,
}: {
  planId: string
  initialExecutionId?: string
}) {
  const [bridge, setBridge] = useState<DashboardDataBridge | null>(null)
  const [plan, setPlan] = useState<ProfilePlan | null>(null)
  const [selected, setSelected] = useState<string | undefined>(
    initialExecutionId,
  )
  const [execution, setExecution] = useState<PlanExecution | null>(null)
  const [requirements, setRequirements] = useState<PlanRequirements | null>(
    null,
  )
  const [comparison, setComparison] = useState<ProfileComparison | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const startKey = useRef<string | null>(null)
  const load = useCallback(async () => {
    const next = await getDashboardDataBridge()
    const value = await profileAction<ProfilePlan>(next, {
      action: 'get',
      plan_id: planId,
    })
    setBridge(next)
    setPlan(value)
    const id = selected ?? value.last_execution?.id
    if (id)
      setExecution(
        await profileAction<PlanExecution>(next, {
          action: 'execution',
          execution_id: id,
        }),
      )
  }, [planId, selected])
  useEffect(() => {
    void load().catch((cause) => setError(message(cause)))
    const timer = setInterval(() => {
      void load().catch((cause) => setError(message(cause)))
    }, 2500)
    return () => clearInterval(timer)
  }, [load])
  async function run() {
    if (!bridge || !plan) return
    setBusy(true)
    setError('')
    try {
      const checked = await profileAction<PlanRequirements>(bridge, {
        action: 'requirements',
        plan_id: planId,
      })
      setRequirements(checked)
      if (!checked.ready) return
      startKey.current ??= crypto.randomUUID()
      const admission = await profileAction<PlanAdmission>(bridge, {
        action: 'start',
        plan_id: planId,
        idempotency_key: startKey.current,
        role:
          plan.configuration.profile_id === 'evolution'
            ? plan.baseline_execution_id
              ? 'candidate'
              : 'baseline'
            : 'run',
      })
      if (admission.blocked) {
        setRequirements(admission.requirements ?? null)
        return
      }
      startKey.current = null
      setSelected(admission.execution_id)
      setComparison(null)
      setRequirements(null)
      await load()
    } catch (cause) {
      setError(message(cause))
    } finally {
      setBusy(false)
    }
  }
  async function cancel() {
    if (!bridge || !execution) return
    setBusy(true)
    try {
      await profileAction(bridge, {
        action: 'cancel',
        execution_id: execution.id,
      })
      await load()
    } catch (cause) {
      setError(message(cause))
    } finally {
      setBusy(false)
    }
  }
  async function exportPlan() {
    if (!bridge || !plan) return
    try {
      downloadJson(
        await profileAction(bridge, { action: 'export', plan_id: planId }),
        `${plan.configuration.profile_id}-${planId}.json`,
      )
    } catch (cause) {
      setError(message(cause))
    }
  }
  async function compare() {
    if (!bridge || !execution) return
    try {
      setComparison(
        await profileAction(bridge, {
          action: 'compare',
          plan_id: planId,
          candidate_id: execution.id,
        }),
      )
    } catch (cause) {
      setError(message(cause))
    }
  }
  const active = plan?.history.find((item) => running(item.state))
  const protectedExecutor = plan?.snapshot.protected_supervisor_required
  const runLabel =
    plan?.configuration.profile_id === 'evolution'
      ? plan.baseline_execution_id
        ? 'Run candidate'
        : 'Capture reference'
      : plan?.history.length
        ? 'Run again'
        : 'Run plan'
  return (
    <ProfilePlanShell
      title={plan?.configuration.label ?? 'Plan'}
      summary={plan?.snapshot.profile.purpose ?? 'Loading saved configuration…'}
    >
      {error ? (
        <Callout tone="warning" title="Plan action unavailable">
          {error}
        </Callout>
      ) : null}
      {plan ? (
        <>
          <Panel>
            <div className="flex flex-wrap items-start justify-between gap-4">
              <div className="min-w-0 text-sm text-ink">
                <strong>{plan.snapshot.profile.label}</strong>
                <p className="break-words">
                  {plan.configuration.model}{' '}
                  <span className="text-ink-muted">
                    · {plan.configuration.provider}
                  </span>
                </p>
                <p className="break-words text-xs text-ink-soft">
                  Evaluator: {plan.configuration.judge_model || 'Not required'}
                </p>
                <p className="text-xs text-ink-soft">
                  {plan.locked
                    ? 'Configuration fixed at first admission.'
                    : 'Draft · configuration can still be edited.'}
                </p>
              </div>
              <div className="flex flex-wrap gap-2">
                {protectedExecutor ? (
                  <button
                    className={primary}
                    type="button"
                    onClick={() => void exportPlan()}
                  >
                    Export for protected executor
                  </button>
                ) : (
                  <button
                    className={primary}
                    type="button"
                    disabled={busy || !!active || !plan.compatible}
                    onClick={() => void run()}
                  >
                    {busy
                      ? 'Checking…'
                      : active
                        ? 'Execution running'
                        : runLabel}
                  </button>
                )}
                <a
                  className={secondary}
                  href={`${hashForNewPlan()}/duplicate/${encodeURIComponent(planId)}`}
                >
                  Duplicate plan
                </a>
                {!plan.locked ? (
                  <a
                    className={secondary}
                    href={`${hashForNewPlan()}/edit/${encodeURIComponent(planId)}`}
                  >
                    Edit draft
                  </a>
                ) : null}
              </div>
            </div>
          </Panel>
          {!plan.compatible ? (
            <Callout tone="warning" title="Pinned revision unavailable">
              The saved profile or contracts differ from this runner. This plan
              remains available for consultation, duplication and export.
            </Callout>
          ) : null}
          {protectedExecutor ? (
            <Callout tone="warning" title="Protected executor required">
              Resilience runs fault injection and requires the protected Release
              Control executor. Export the configured plan to use that executor.
            </Callout>
          ) : null}
          {requirements ? <Requirements value={requirements} /> : null}
          {active && selected && active.id !== selected ? (
            <a className={secondary} href={hashForExecution(active.id)}>
              Follow current execution
            </a>
          ) : null}
          {execution ? (
            <>
              <PlanProgress execution={execution} />
              {running(execution.state) ? (
                <button
                  className={`${secondary} justify-self-start`}
                  type="button"
                  disabled={busy || execution.state === 'cancelling'}
                  onClick={() => void cancel()}
                >
                  {execution.state === 'cancelling'
                    ? 'Cancelling…'
                    : 'Cancel execution'}
                </button>
              ) : null}
              <DataTable
                caption="Planned scenario slots and native evidence"
                collapse
              >
                <thead>
                  <tr>
                    <th>Round</th>
                    <th>Scenario</th>
                    <th>State</th>
                    <th>Objective</th>
                    <th>Technical validity</th>
                    <th>Evidence</th>
                  </tr>
                </thead>
                <tbody>
                  {execution.slots.map((slot) => (
                    <tr key={slot.execution_id}>
                      <td data-label="Round">{slot.round}</td>
                      <td data-label="Scenario" className="break-words">
                        {slot.scenario_id}
                      </td>
                      <td data-label="State">
                        {slot.state.replaceAll('_', ' ')}
                        {slot.error ? (
                          <p className="text-xs text-danger">{slot.error}</p>
                        ) : null}
                      </td>
                      <td data-label="Objective">
                        {slot.observed === 0 || slot.technical_valid === 0 ? (
                          'Unavailable'
                        ) : slot.passed === 1 ? (
                          'Passed'
                        ) : (
                          <span className="text-danger">Failed</span>
                        )}
                      </td>
                      <td data-label="Technical validity">
                        {slot.observed === 0
                          ? 'Unavailable'
                          : slot.technical_valid === 1
                            ? 'Valid'
                            : 'Not valid'}
                      </td>
                      <td data-label="Evidence">
                        {slot.state !== 'pending' &&
                        slot.state !== 'not_run' ? (
                          <a
                            className="text-xs text-ink underline"
                            href={hashForExecution(slot.execution_id)}
                          >
                            Open native execution
                          </a>
                        ) : (
                          'Not produced'
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </DataTable>
              {execution.measurements ? (
                <Panel>
                  <h2 className="mt-0 text-sm font-semibold">
                    Measurements by compatible cohort
                  </h2>
                  <p className="text-xs text-ink-soft">
                    All observations and failed attempts are included in
                    consumption. Missing telemetry is unavailable. Comparisons
                    remain descriptive.
                  </p>
                  {execution.measurements.cohorts.map((cohort) => (
                    <details
                      key={cohort.cohort_sha256}
                      className="py-2 text-xs"
                    >
                      <summary className="cursor-pointer text-ink">
                        {cohort.scenario_id} · {cohort.aggregate.observed_runs}{' '}
                        observations
                      </summary>
                      <dl className="grid gap-2 p-2 sm:grid-cols-2">
                        {Object.entries(cohort.consumption).map(
                          ([key, value]) => (
                            <div key={key}>
                              <dt className="text-ink-soft">
                                {key.replaceAll('_', ' ')}
                              </dt>
                              <dd className="mx-0 break-words font-mono">
                                {value === null
                                  ? 'Unavailable'
                                  : typeof value === 'object'
                                    ? JSON.stringify(value)
                                    : String(value)}
                              </dd>
                            </div>
                          ),
                        )}
                      </dl>
                    </details>
                  ))}
                </Panel>
              ) : null}
              {execution.role === 'candidate' &&
              !running(execution.state) &&
              plan.baseline_execution_id ? (
                <button
                  className={secondary}
                  type="button"
                  onClick={() => void compare()}
                >
                  Compare with reference
                </button>
              ) : null}
              {comparison ? (
                <Panel>
                  <h2 className="mt-0 text-sm">Descriptive comparison</h2>
                  <p className="text-xs text-ink-soft">
                    {comparison.comparisons.length} compatible cohorts ·{' '}
                    {comparison.excluded.length} excluded cohorts. No promotion
                    gates.
                  </p>
                  {comparison.unavailable ? (
                    <Callout title="Incomplete comparison" tone="warning">
                      {comparison.unavailable}
                    </Callout>
                  ) : null}
                  <DataTable
                    caption="Reference and candidate consumption by compatible scenario"
                    collapse
                  >
                    <thead>
                      <tr>
                        <th>Scenario</th>
                        <th>Reference samples</th>
                        <th>Candidate samples</th>
                        <th>Reference tokens / completion</th>
                        <th>Candidate tokens / completion</th>
                        <th>Change</th>
                      </tr>
                    </thead>
                    <tbody>
                      {comparison.comparisons.map((cohort) => {
                        const delta =
                          cohort.metrics.delta?.consumption
                            .tokens_per_completion
                        const number = (value: number | null | undefined) =>
                          value == null
                            ? 'Unavailable'
                            : value.toLocaleString(undefined, {
                                maximumFractionDigits: 1,
                              })
                        return (
                          <tr key={cohort.cohort_sha256}>
                            <td data-label="Scenario">{cohort.scenario_id}</td>
                            <td data-label="Reference samples">
                              {cohort.metrics.from.included_runs}
                            </td>
                            <td data-label="Candidate samples">
                              {cohort.metrics.to.included_runs}
                            </td>
                            <td data-label="Reference tokens / completion">
                              {number(
                                cohort.metrics.from.consumption
                                  .tokens_per_completion,
                              )}
                            </td>
                            <td data-label="Candidate tokens / completion">
                              {number(
                                cohort.metrics.to.consumption
                                  .tokens_per_completion,
                              )}
                            </td>
                            <td data-label="Change">
                              {delta
                                ? `${delta.absolute > 0 ? '+' : ''}${number(delta.absolute)}`
                                : 'Unavailable'}
                            </td>
                          </tr>
                        )
                      })}
                    </tbody>
                  </DataTable>
                  {comparison.excluded.length ? (
                    <details className="mt-3 text-xs">
                      <summary className="cursor-pointer">
                        Excluded cohorts
                      </summary>
                      <ul>
                        {comparison.excluded.map((entry) => (
                          <li key={`${entry.side}-${entry.cohort_sha256}`}>
                            {entry.side}: {entry.reason}
                          </li>
                        ))}
                      </ul>
                    </details>
                  ) : null}
                  <button
                    className={secondary}
                    type="button"
                    onClick={() =>
                      downloadJson(comparison, `comparison-${planId}.json`)
                    }
                  >
                    Export comparison evidence
                  </button>
                </Panel>
              ) : null}
            </>
          ) : (
            <Panel>
              <p className="text-sm text-ink-soft">
                No executions yet.{' '}
                {protectedExecutor
                  ? 'Export this plan to use the protected executor.'
                  : 'Run this plan to collect its first results.'}
              </p>
            </Panel>
          )}
          <Panel>
            <h2 className="mt-0 text-sm font-semibold">Execution history</h2>
            {plan.history.length ? (
              <ul className="m-0 grid gap-2 p-0">
                {plan.history.map((item) => (
                  <li
                    key={item.id}
                    className="flex list-none flex-wrap items-center justify-between gap-3 text-xs text-ink-soft"
                  >
                    <span>
                      {new Date(item.started_at).toLocaleString()} · {item.role}{' '}
                      · {item.state} · {item.finished}/{item.planned} slots
                      {item.id === plan.baseline_execution_id
                        ? ' · Reference'
                        : ''}
                    </span>
                    <button
                      type="button"
                      className={secondary}
                      onClick={() => {
                        setSelected(item.id)
                        setComparison(null)
                      }}
                    >
                      View execution
                    </button>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-xs text-ink-soft">
                This plan has an empty history.
              </p>
            )}
          </Panel>
          <Coverage
            profile={{ ...plan.snapshot.profile, budget: plan.snapshot.budget }}
            scenarios={plan.snapshot.scenario_ids}
            version={plan.snapshot.version}
            digest={plan.snapshot.profile_sha256}
          />
          <div>
            <button
              className={secondary}
              type="button"
              onClick={() => void exportPlan()}
            >
              Export plan
            </button>
          </div>
        </>
      ) : (
        <p role="status">Loading plan…</p>
      )}
    </ProfilePlanShell>
  )
}

export function ProfileExecutionPage({ executionId }: { executionId: string }) {
  const [execution, setExecution] = useState<PlanExecution | null>(null)
  const [error, setError] = useState('')
  useEffect(() => {
    void getDashboardDataBridge()
      .then((bridge) =>
        profileAction<PlanExecution>(bridge, {
          action: 'execution',
          execution_id: executionId,
        }),
      )
      .then(setExecution)
      .catch((cause) => setError(message(cause)))
  }, [executionId])
  return execution ? (
    <ProfilePlanDetailPage
      key={executionId}
      planId={execution.plan_id}
      initialExecutionId={executionId}
    />
  ) : (
    <ProfilePlanShell
      title="Plan execution"
      summary={error || 'Loading composed execution…'}
    >
      <p role="status">{error || 'Loading…'}</p>
    </ProfilePlanShell>
  )
}
