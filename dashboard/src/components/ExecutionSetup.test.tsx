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
  catalogStatus: {
    tone: 'ready' as const,
    text: 'catalog ready · 1 model · 1 test',
  },
  onLabelChange: () => undefined,
  onUrlChange: () => undefined,
  onSubjectChange: () => undefined,
  onSelectedScenariosChange: () => undefined,
  onQueryChange: () => undefined,
  onRunsChange: () => undefined,
  onTechnicalRetriesChange: () => undefined,
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
      expect(html).toContain('Choose the model')
      expect(html).not.toContain('Judge')
      expect(html).toContain('Pick the tests')
      expect(html).toContain('Advanced · sampling and retries')
      // Every execution runs the canonical cases, so runs pair up.
      expect(html).not.toMatch(/seed/i)
      expect(html).toContain('Search by name or id')
      expect(html).toContain('2 runs in total')
      expect(html).toContain('catalog ready · 1 model · 1 test')
      expect(html).not.toContain('logical')
      // Audit RS-04: no 01/02/03 numerals.
      expect(html).not.toContain('>01<')
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
    // Audit PN-21: a plan's endpoint lives under advanced, read from the
    // summary. A quick run always targets this worker's stack.
    expect(plan).toContain('Harness endpoint')
    expect(plan).toContain('ws://127.0.0.1:49134')
    expect(quick).not.toContain('Harness endpoint')
    expect(quick).not.toContain('ws://127.0.0.1:49134')
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
    // A quick run needs no endpoint, so a failed catalog does not block it.
    expect(
      validateExecutionSetup({
        mode: 'quick',
        label: '',
        subject: 'openai\ngpt-5',
        selectedScenarios: ['a'],
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

  it('opens on the selected tests when asked, as Run again does', () => {
    const props = {
      ...sharedProps,
      mode: 'quick' as const,
      availableScenarios: ['minimal_path', 'context_pressure', 'trend_blog'],
      selectedScenarios: ['minimal_path'],
    }
    const all = renderToStaticMarkup(<ExecutionSetup {...props} />)
    const selected = renderToStaticMarkup(
      <ExecutionSetup {...props} initialOnlySelected />,
    )
    expect(all).toContain('3 of 3 shown')
    expect(selected).toContain('1 of 3 shown')
    expect(selected).toContain('>minimal_path<')
    expect(selected).not.toContain('>trend_blog<')
    // The native box stays a visible, clickable control.
    expect(selected).toContain('appearance-auto')
  })

  // Audit RS-07 / PN-20: the review is one sentence plus a detail line.
  it('summarises the setup in one sentence for the footer', () => {
    const summary = executionSetupSummary({
      mode: 'quick',
      selectedScenarios: 2,
      runsPerScenario: 1,
      technicalRetries: 1,
      subject: 'anthropic / claude-fable-5',
      url: 'ws://127.0.0.1:49134',
    })
    expect(summary.headline).toBe(
      '2 tests · 2 runs · anthropic / claude-fable-5',
    )
    expect(summary.detail).toBe('1 run per test · 1 retry')
    const html = renderToStaticMarkup(
      <ExecutionSetupFooter
        summary={{
          mode: 'plan',
          selectedScenarios: 0,
          runsPerScenario: 2,
          technicalRetries: 0,
          subject: '',
          url: 'ws://x',
        }}
        pending={['Add a plan label.', 'Select at least one test.']}
      >
        <button type="submit">create draft plan</button>
      </ExecutionSetupFooter>,
    )
    expect(html).toContain('0 tests · 0 runs · no model')
    expect(html).toContain('2 runs per test · 0 retries · ws://x')
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
          subject: 'openai / gpt-5',
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

  // Audit PN-09: families group the rows; singletons gather under "other".
  it('groups tests by family and gathers singletons under other tests', () => {
    expect(
      groupScenarios([
        'chess_build',
        'chess_play',
        'engineering_review',
        'minimal_path',
      ]),
    ).toEqual([
      { key: 'chess', label: 'chess', items: ['chess_build', 'chess_play'] },
      {
        key: 'other',
        label: 'other tests',
        items: ['engineering_review', 'minimal_path'],
      },
    ])
    const html = renderToStaticMarkup(
      <ExecutionSetup
        {...sharedProps}
        mode="plan"
        availableScenarios={['chess_build', 'chess_play', 'minimal_path']}
        selectedScenarios={[]}
      />,
    )
    expect(html).toContain('data-scenario-group="chess"')
    expect(html).toContain('data-scenario-group="other"')
    expect(html).toContain('minimal_path')
    expect(html).not.toContain('>local<')
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
