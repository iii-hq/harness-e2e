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
      expect(html).toContain('Choose the model and judge')
      expect(html).toContain('Pick the tests')
      expect(html).toContain('Sampling, retries and seed')
      expect(html).toContain('Search by name or id')
      expect(html).toContain('2 runs in total')
      expect(html).not.toContain('logical')
      // Audit RS-04: no 01/02/03 numerals.
      expect(html).not.toContain('>01<')
      expect(html).toContain('Harness endpoint')
      expect(html).toContain('aria-labelledby="test-setup-subject-label"')
    }
    expect(plan).toContain('Plan label')
    expect(plan).toContain('Name the plan')
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
    expect(html).toContain('>Runs<')
    expect(html).toContain('Runs per test')
    expect(html).toContain('Execution model')
    expect(html).toContain('Default judge')
    expect(html).toContain('Create draft plan')
    expect(html).toContain('lg:top-20')
  })

  // Audit RS-02: inside a dialog the review column sticks to the top of the
  // dialog scroller, not 5rem below it.
  it('drops the page header offset when the review sits in a dialog', () => {
    const html = renderToStaticMarkup(
      <ExecutionSetupReview
        mode="quick"
        status="Ready"
        subject="openai / gpt-5"
        judge=""
        url="ws://127.0.0.1:49134"
        selectedScenarios={1}
        plannedRuns={1}
        runsPerScenario={1}
        technicalRetries={0}
        ready
        stickyOffset="dialog"
      >
        <button type="button">run 1 test</button>
      </ExecutionSetupReview>,
    )
    expect(html).toContain('lg:top-0')
    expect(html).not.toContain('lg:top-20')
  })

  // Audit PN-17: an empty catalog is not an empty search.
  it('explains an empty catalog and offers a refresh instead of a search hint', () => {
    const html = renderToStaticMarkup(
      <ExecutionSetup
        {...sharedProps}
        mode="quick"
        modelGroups={[]}
        availableScenarios={[]}
        selectedScenarios={[]}
        catalogSummary="Catalog unavailable"
        onRefreshCatalog={() => undefined}
      />,
    )
    expect(html).toContain('data-catalog-empty')
    expect(html).toContain('No tests loaded')
    expect(html).toContain('refresh catalog')
    expect(html).not.toContain('No tests match')
    expect(html).toContain('No models in the catalog')
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
