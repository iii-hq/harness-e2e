import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  ExecutionSetup,
  ExecutionSetupReview,
} from '@/components/ExecutionSetup'

const sharedProps = {
  idPrefix: 'test-setup',
  label: '',
  url: 'ws://127.0.0.1:49134',
  subject: 'openai\ngpt-5',
  judge: '',
  modelGroups: [
    {
      provider: 'openai',
      models: [{ label: 'gpt-5', value: 'openai\ngpt-5' }],
    },
  ],
  availableScenarios: ['security_review.scan_commit'],
  selectedScenarios: ['security_review.scan_commit'],
  query: '',
  runs: '2',
  technicalRetries: '1',
  seed: '',
  catalogSummary: '1 registered model · 1 scenario',
  onLabelChange: () => undefined,
  onUrlChange: () => undefined,
  onSubjectChange: () => undefined,
  onJudgeChange: () => undefined,
  onSelectedScenariosChange: () => undefined,
  onQueryChange: () => undefined,
  onRunsChange: () => undefined,
  onTechnicalRetriesChange: () => undefined,
  onSeedChange: () => undefined,
}

describe('execution setup workspace', () => {
  it('uses the same operational structure for plans and quick executions', () => {
    const plan = renderToStaticMarkup(
      <ExecutionSetup
        {...sharedProps}
        mode="plan"
        purpose="Measure prompt routing"
        onPurposeChange={() => undefined}
      />,
    )
    const quick = renderToStaticMarkup(
      <ExecutionSetup {...sharedProps} mode="quick" />,
    )

    for (const html of [plan, quick]) {
      expect(html).toContain('Configure the execution environment')
      expect(html).toContain('Select the benchmark scope')
      expect(html).toContain('Sampling, retries and seed')
      expect(html).toContain('Search by name or id')
      expect(html).toContain('2 logical runs')
    }
    expect(plan).toContain('Plan label')
    expect(plan).toContain('Define the evidence intent')
    expect(plan).toContain('Purpose')
    expect(quick).toContain('Execution label')
    expect(quick).toContain('Name this run')
    expect(quick).not.toContain('Purpose')
  })

  it('keeps the review metrics visible next to either action', () => {
    const html = renderToStaticMarkup(
      <ExecutionSetupReview
        mode="plan"
        status="Ready"
        subject="openai / gpt-5"
        judge=""
        url="ws://127.0.0.1:49134"
        selectedScenarios={3}
        plannedRuns={6}
        runsPerScenario={2}
        technicalRetries={1}
        ready
      >
        <button type="button">Create draft plan</button>
      </ExecutionSetupReview>,
    )

    expect(html).toContain('Reusable workflow')
    expect(html).toContain('Logical runs')
    expect(html).toContain('Runs / test')
    expect(html).toContain('Execution model')
    expect(html).toContain('Create draft plan')
  })

  it('marks local scenarios without mixing authoring into execution setup', () => {
    const html = renderToStaticMarkup(
      <ExecutionSetup
        {...sharedProps}
        mode="quick"
        availableScenarios={['markdown_console_draft']}
        localScenarioIds={['markdown_console_draft']}
        selectedScenarios={[]}
      />,
    )

    expect(html).not.toContain('New local scenario')
    expect(html).toContain('Markdown Console Draft')
    expect(html).toContain('Local')
  })
})
