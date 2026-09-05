import { buttonClassName, Panel } from '@/design-system'
import type {
  MasterTestPlan,
  MasterTestProfile,
} from '@/lib/dashboard-data-source'

export function profileExport(
  plan: MasterTestPlan,
  profile: MasterTestProfile,
) {
  return {
    schema: 'harness-e2e-profile-campaigns/v1',
    plan_id: plan.plan_id,
    version: plan.version,
    definition_sha256: plan.definition_sha256,
    profile,
  }
}

function download(plan: MasterTestPlan, profile: MasterTestProfile) {
  const content = `${JSON.stringify(profileExport(plan, profile), null, 2)}\n`
  const url = URL.createObjectURL(
    new Blob([content], { type: 'application/json' }),
  )
  const link = document.createElement('a')
  link.href = url
  link.download = `harness-${profile.id}-v${plan.version}.json`
  link.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

export function MasterTestProfiles({ plan }: { plan: MasterTestPlan }) {
  return (
    <Panel
      padding="none"
      className="my-6 overflow-hidden"
      aria-label="Master test profiles"
    >
      <div className="p-4">
        <h2 className="m-0 text-sm font-semibold text-ink">
          Master test plan · v{plan.version}
        </h2>
        <p className="mt-1 mb-0 text-xs leading-5 text-ink-soft">
          Choose the purpose of the evaluation. Exports preserve cases,
          independent repetitions and retry policy. All profiles report advisory
          evidence.
        </p>
      </div>
      <div className="divide-y divide-line">
        {plan.profiles.map((profile) => (
          <div
            key={profile.id}
            className="grid gap-3 p-4 md:grid-cols-[9rem_1fr_12rem_auto] md:items-start"
          >
            <strong className="text-sm text-ink">{profile.label}</strong>
            <div className="min-w-0 text-xs leading-5 text-ink-soft">
              <p className="m-0">{profile.purpose}</p>
              <details className="mt-1">
                <summary className="cursor-pointer text-ink-muted">
                  Coverage and measures
                </summary>
                <p className="my-2 break-words">
                  {profile.scenario_ids.join(', ')}
                </p>
                <p className="my-2 break-words">
                  {profile.metrics.join(' · ')}
                </p>
                <p className="my-2">
                  Subject token ceiling:{' '}
                  {profile.budget.subject_token_limit === null
                    ? 'not available for all cases'
                    : profile.budget.subject_token_limit.toLocaleString(
                        'en-US',
                      )}
                  . Setup, evaluation and cleanup use additional resources.
                </p>
              </details>
            </div>
            <div className="text-xs leading-5 text-ink-muted">
              <span className="block">
                {profile.scenario_ids.length} cases · {profile.repetitions} run
                {profile.repetitions === 1 ? '' : 's'} each
              </span>
              <span className="block">
                {profile.budget.planned_runs} planned slots
                {profile.budget.fault_runs > 0
                  ? ` · ${profile.budget.fault_runs} fault runs`
                  : ''}
              </span>
              {profile.protected_supervisor_required ? (
                <span className="block">Protected fault executor</span>
              ) : null}
            </div>
            <button
              type="button"
              className={buttonClassName({
                variant: 'secondary',
                size: 'compact',
              })}
              onClick={() => download(plan, profile)}
              aria-label={`Export ${profile.label} profile`}
            >
              export profile
            </button>
          </div>
        ))}
      </div>
    </Panel>
  )
}
