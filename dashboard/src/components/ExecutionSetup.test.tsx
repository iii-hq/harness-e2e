import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  ExecutionSetup,
  ExecutionSetupFooter,
  executionSetupSummary,
  groupScenarios,
  validateExecutionSetup,
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
  catalogStatus: {
    tone: 'ready' as const,
    text: 'catalog ready · 1 model · 1 test',
  },
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

describe('execution setup sheet', () => {
  it('uses the same one-column structure for plans and quick executions', () => {
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
      expect(html).toContain('Advanced · sampling, retries and seed')
      expect(html).toContain('Search by name or id')
      expect(html).toContain('2 runs in total')
      expect(html).toContain('catalog ready · 1 model · 1 test')
      expect(html).not.toContain('logical')
      // Audit RS-04: no 01/02/03 numerals.
      expect(html).not.toContain('>01<')
      // Audit PN-21: the endpoint lives under advanced, read from the summary.
      expect(html).toContain('Harness endpoint')
      expect(html).toContain('ws://127.0.0.1:49134')
      // Audit PN-12: the model trigger is a labelled 36px control.
      expect(html).toContain('for="test-setup-subject"')
      expect(html).toContain('id="test-setup-subject"')
      // Audit PN-13: the disclosure carries a chevron.
      expect(html).toContain('group-open:rotate-0')
      // Audit PN-09: a 36px row per test inside a family group.
      expect(html).toContain('data-scenario-group="other"')
      expect(html).toContain('min-h-9')
      // The only test is selected, so the group control offers to clear it.
      expect(html).toContain('clear group')
      expect(html).not.toContain('max-h-[25rem]')
      // Audit PN-24: text input with its own clear control, no native ×.
      expect(html).not.toContain('type="search"')
      expect(html).toContain('1 of 1 shown · 1 selected · 2 runs in total')
    }
    expect(plan).toContain('Plan label')
    expect(plan).toContain('Name the plan')
    expect(plan).toContain('Purpose')
    expect(quick).toContain('Execution label')
    expect(quick).toContain('Name this run')
    expect(quick).not.toContain('Purpose')
  })

  // Audit PN-05: validation names each pending item and marks the field.
  it('shows the submit-time errors inline', () => {
    const errors = validateExecutionSetup({
      mode: 'plan',
      label: ' ',
      subject: '',
      selectedScenarios: [],
      url: '',
    })
    expect(errors).toEqual({
      label: 'Add a plan label.',
      subject: 'Choose an execution model.',
      scenarios: 'Select at least one test.',
      url: 'The Harness endpoint is missing.',
    })
    expect(
      validateExecutionSetup({
        mode: 'quick',
        label: '',
        subject: 'openai\ngpt-5',
        selectedScenarios: ['a'],
        url: 'ws://x',
      }),
    ).toEqual({})
    const html = renderToStaticMarkup(
      <ExecutionSetup
        {...sharedProps}
        mode="plan"
        label=""
        selectedScenarios={[]}
        errors={{
          label: 'Add a plan label.',
          scenarios: 'Select at least one test.',
        }}
      />,
    )
    expect(html).toContain('aria-invalid="true"')
    expect(html).toContain('id="test-setup-label-error"')
    expect(html).toContain('Select at least one test.')
  })

  // Audit RS-07 / PN-20: the review is one sentence plus a detail line.
  it('summarises the setup in one sentence for the footer', () => {
    const summary = executionSetupSummary({
      mode: 'quick',
      selectedScenarios: 2,
      runsPerScenario: 1,
      technicalRetries: 1,
      seed: '',
      subject: 'anthropic / claude-fable-5',
      judge: '',
      url: 'ws://127.0.0.1:49134',
    })
    expect(summary.headline).toBe(
      '2 tests · 2 runs · anthropic / claude-fable-5 · default judge',
    )
    expect(summary.detail).toBe(
      '1 run per test · 1 retry · canonical seed · ws://127.0.0.1:49134',
    )
    const html = renderToStaticMarkup(
      <ExecutionSetupFooter
        summary={{
          mode: 'plan',
          selectedScenarios: 0,
          runsPerScenario: 2,
          technicalRetries: 0,
          seed: '7',
          subject: '',
          judge: 'openai / gpt-5',
          url: 'ws://x',
        }}
        pending={['Add a plan label.', 'Select at least one test.']}
      >
        <button type="submit">create draft plan</button>
      </ExecutionSetupFooter>,
    )
    expect(html).toContain('0 tests · 0 runs · no model · judge openai / gpt-5')
    expect(html).toContain('2 runs per test · 0 retries · seed 7 · ws://x')
    expect(html).toContain(
      'Before creating: Add a plan label. Select at least one test.',
    )
    expect(html).toContain('role="status"')
    expect(html).toContain('data-execution-setup-footer')
    expect(html).not.toContain('>Runs<')
  })

  it('reports the footer error as an alert', () => {
    const html = renderToStaticMarkup(
      <ExecutionSetupFooter
        summary={{
          mode: 'quick',
          selectedScenarios: 1,
          runsPerScenario: 1,
          technicalRetries: 0,
          seed: '',
          subject: 'openai / gpt-5',
          judge: '',
          url: 'ws://x',
        }}
        error="Runner unavailable"
      >
        <button type="submit">run 1 test</button>
      </ExecutionSetupFooter>,
    )
    expect(html).toContain('role="alert"')
    expect(html).toContain('Runner unavailable')
  })

  // Audit PN-17: an empty catalog names the fix instead of asking for another search.
  it('distinguishes an empty catalog from an empty search', () => {
    const html = renderToStaticMarkup(
      <ExecutionSetup
        {...sharedProps}
        mode="quick"
        modelGroups={[]}
        availableScenarios={[]}
        selectedScenarios={[]}
        catalogStatus={{ tone: 'unavailable', text: 'catalog unavailable' }}
        onRefreshCatalog={() => undefined}
      />,
    )
    expect(html).toContain('data-catalog-empty')
    expect(html).toContain('No tests loaded')
    expect(html).toContain('refresh catalog')
    expect(html).not.toContain('No tests match')
    expect(html).toContain('No models in the catalog')
    expect(html).toContain('bg-danger')
  })

  // Audit PN-09 / PN-06: families group the rows; local tests are their own
  // group and can be listed as not available.
  it('groups tests by family with local tests first', () => {
    expect(
      groupScenarios(
        ['chess_build', 'chess_play', 'engineering_review', 'markdown_x'],
        ['markdown_x'],
      ),
    ).toEqual([
      { key: 'local', label: 'local', items: ['markdown_x'] },
      { key: 'chess', label: 'chess', items: ['chess_build', 'chess_play'] },
      { key: 'other', label: 'other tests', items: ['engineering_review'] },
    ])
    const html = renderToStaticMarkup(
      <ExecutionSetup
        {...sharedProps}
        mode="plan"
        availableScenarios={['security_review.scan_commit']}
        localScenarioIds={['markdown_console_draft']}
        scenarioTitles={{ markdown_console_draft: 'Markdown Console Draft' }}
        unavailableScenarios={{
          ids: ['markdown_console_draft'],
          reason: 'Local Markdown tests are not available in plans.',
        }}
        selectedScenarios={[]}
      />,
    )
    expect(html).toContain('Markdown Console Draft')
    expect(html).toContain('>local<')
    expect(html).toContain('not available in plans')
    expect(html).toContain('disabled=""')
    expect(html).toContain('data-scenario-group="local"')
  })

  // Audit RS-15: when something else holds the form it is parked, not dead —
  // the reader can still see what they would be configuring.
  it('parks the form visibly instead of leaving dead controls at full strength', () => {
    const parked = renderToStaticMarkup(
      <ExecutionSetup
        {...sharedProps}
        mode="quick"
        disabled
        selectedScenarios={[]}
      />,
    )
    expect(parked).toContain('data-parked="true"')
    expect(parked).toContain('opacity-55')

    const open = renderToStaticMarkup(
      <ExecutionSetup {...sharedProps} mode="quick" selectedScenarios={[]} />,
    )
    expect(open).not.toContain('data-parked')
    expect(open).not.toContain('opacity-55')
  })
})
