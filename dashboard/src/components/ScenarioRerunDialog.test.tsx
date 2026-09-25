import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { ScenarioRerunDialog } from '@/components/ScenarioRerunDialog'
import type { PlanExecution } from '@/lib/plan-execution'

const slot = (round: number, scenario_id: string, execution_id: string) => ({
  round,
  group_id: scenario_id,
  scenario_id,
  execution_id,
  state: 'finished',
  observed: 1,
  completed: 1,
  passed: 1,
  technical_valid: 1,
  result_path: null,
  error: null,
})

const execution = {
  id: 'plan-local',
  source: { kind: 'local' },
  slots: [1, 2].flatMap((round) => [
    slot(round, 'registry_implementation', `group-${round}`),
    slot(round, 'registry_verification', `group-${round}`),
    slot(round, 'minimal_path', `minimal-${round}`),
  ]),
} as unknown as PlanExecution

function render(value: PlanExecution, scenarioId: string) {
  return renderToStaticMarkup(
    <ScenarioRerunDialog
      bridge={null}
      execution={value}
      scenarioId={scenarioId}
      onClose={() => {}}
      onStarted={() => {}}
    />,
  )
}

describe('run this scenario again', () => {
  it('says the last attempt counts, which rounds run and that a group runs whole', () => {
    const grouped = render(execution, 'registry_verification')
    expect(grouped).toContain('Run registry_verification again')
    expect(grouped).toContain('The last attempt counts')
    expect(grouped).toContain('All 2 of its rounds run again.')
    expect(grouped).toContain(
      'registry_implementation then registry_verification run only together, in this order; the whole group runs again.',
    )
    expect(render(execution, 'minimal_path')).not.toContain('whole group')
  })

  it('re-runs an imported execution’s job on GitHub', () => {
    const imported = {
      ...execution,
      source: {
        kind: 'github',
        repository: 'iii-hq/harness-e2e',
        run_id: 42,
        run_attempt: 1,
        url: 'https://github.com/iii-hq/harness-e2e/actions/runs/42',
        release_control_execution_id: '12d5f973-aaaa',
      },
    } as PlanExecution
    const html = render(imported, 'minimal_path')
    expect(html).toContain('Run minimal_path again on GitHub')
    expect(html).toContain('Re-runs its group’s job on GitHub')
    expect(html).toContain('imports the run again when it ends')
    expect(html).toContain(
      'href="https://github.com/iii-hq/harness-e2e/actions/runs/42"',
    )
    expect(html).toContain('Release Control execution 12d5f973')
    expect(html).toContain('>run again<')
  })

  it('says a Docker test runs again as the next attempt', () => {
    const docker = {
      ...execution,
      source: { kind: 'docker', attempt: 1, phase: 'done', groups: [] },
    } as unknown as PlanExecution
    const html = render(docker, 'minimal_path')
    expect(html).toContain('Run minimal_path again in Docker')
    expect(html).toContain('as attempt 2')
  })
})
