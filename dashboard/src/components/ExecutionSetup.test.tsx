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
  availableScenarios: ['security_review.scan_commit'],
  selectedScenarios: ['security_review.scan_commit'],
  query: '',
  runs: '2',
  technicalRetries: '1',
  catalogStatus: {
    tone: 'ready' as const,
    text: 'catalog ready · 1 test',
  },
  onLabelChange: () => undefined,
  onSelectedScenariosChange: () => undefined,
  onQueryChange: () => undefined,
  onRunsChange: () => undefined,
  onTechnicalRetriesChange: () => undefined,
}

describe('suite editor form', () => {
  it('names the suite, sets its runs and retries, and picks its tests', () => {
    const html = renderToStaticMarkup(<ExecutionSetup {...sharedProps} />)
    expect(html).toContain('Name the suite')
    expect(html).toContain('Suite name')
    expect(html).toContain('Runs and retries')
    expect(html).toContain('Pick the tests')
    expect(html).toContain('Runs per test')
    expect(html).toContain('Technical retries')
    expect(html).toContain('Search by name or id')
    expect(html).toContain('catalog ready · 1 test')
    expect(html).toContain('data-scenario-group="other"')
    expect(html).toContain('clear group')
    expect(html).not.toContain('type="search"')
    expect(html).toContain('1 of 1 shown · 1 selected · 2 runs in total')
    // Running tests has its own dialog; the suite editor holds no model.
    expect(html).not.toContain('Choose the model')
    expect(html).not.toContain('Execution model')
    expect(html).not.toContain('Name this run')
  })

  // Audit PN-05: validation names each pending item and marks the field.
  it('shows the submit-time errors inline', () => {
    expect(
      validateExecutionSetup({ label: ' ', selectedScenarios: [] }),
    ).toEqual({
      label: 'Name the suite.',
      scenarios: 'Select at least one test.',
    })
    expect(
      validateExecutionSetup({ label: 'Nightly', selectedScenarios: ['a'] }),
    ).toEqual({})
    const html = renderToStaticMarkup(
      <ExecutionSetup
        {...sharedProps}
        selectedScenarios={[]}
        errors={{
          label: 'Name the suite.',
          scenarios: 'Select at least one test.',
        }}
      />,
    )
    expect(html).toContain('aria-invalid="true"')
    expect(html).toContain('id="test-setup-label-error"')
    expect(html).toContain('Select at least one test.')
  })

  it('opens on the selected tests when asked, as editing a suite does', () => {
    const props = {
      ...sharedProps,
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
    expect(selected).toContain('appearance-auto')
  })

  // Audit RS-07 / PN-20: the review is one sentence plus a detail line.
  it('summarises the suite in one sentence for the footer', () => {
    const summary = executionSetupSummary({
      selectedScenarios: 2,
      runsPerScenario: 1,
      technicalRetries: 1,
    })
    expect(summary.headline).toBe('2 tests · 2 runs')
    expect(summary.detail).toBe('1 run per test · 1 retry')
    const html = renderToStaticMarkup(
      <ExecutionSetupFooter
        summary={{
          selectedScenarios: 0,
          runsPerScenario: 2,
          technicalRetries: 0,
        }}
        pending={['Name the suite.', 'Select at least one test.']}
      >
        <button type="submit">save suite</button>
      </ExecutionSetupFooter>,
    )
    expect(html).toContain('>0 tests · 0 runs<')
    expect(html).toContain('2 runs per test · 0 retries')
    expect(html).toContain(
      'Before saving: Name the suite. Select at least one test.',
    )
    expect(html).toContain('role="status"')
    expect(html).toContain('data-execution-setup-footer')
  })

  it('reports the footer error as an alert', () => {
    const html = renderToStaticMarkup(
      <ExecutionSetupFooter
        summary={{
          selectedScenarios: 1,
          runsPerScenario: 1,
          technicalRetries: 0,
        }}
        error="Could not save the suite"
      >
        <button type="submit">save suite</button>
      </ExecutionSetupFooter>,
    )
    expect(html).toContain('role="alert"')
    expect(html).toContain('Could not save the suite')
  })

  // Audit PN-17: an empty catalog names the fix instead of asking for another search.
  it('distinguishes an empty catalog from an empty search', () => {
    const html = renderToStaticMarkup(
      <ExecutionSetup
        {...sharedProps}
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
        availableScenarios={['chess_build', 'chess_play', 'minimal_path']}
        selectedScenarios={[]}
      />,
    )
    expect(html).toContain('data-scenario-group="chess"')
    expect(html).toContain('data-scenario-group="other"')
  })

  // Audit RS-15: a parked form stays readable instead of dead at full strength.
  it('parks the form visibly while saving', () => {
    const parked = renderToStaticMarkup(
      <ExecutionSetup {...sharedProps} disabled selectedScenarios={[]} />,
    )
    expect(parked).toContain('data-parked="true"')
    expect(parked).toContain('opacity-55')
    const open = renderToStaticMarkup(
      <ExecutionSetup {...sharedProps} selectedScenarios={[]} />,
    )
    expect(open).not.toContain('data-parked')
  })
})
