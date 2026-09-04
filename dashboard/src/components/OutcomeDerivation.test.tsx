import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  OutcomeDerivation,
  outcomeStatus,
} from '@/components/OutcomeDerivation'

describe('outcome derivation', () => {
  it('names each value by its role instead of listing three peers', () => {
    const html = renderToStaticMarkup(
      <OutcomeDerivation
        rows={[
          { role: 'system', value: 'passed' },
          { role: 'advisory', value: 'pass_with_concerns' },
          { role: 'effective', value: 'passed_with_concerns' },
        ]}
      />,
    )
    expect(html).toContain('data-outcome-derivation')
    expect(html).toContain('system · deterministic gates')
    expect(html).toContain('advisory · separate qualitative conclusion')
    expect(html).toContain('effective · the status the result contract')
    // The advisory never reads as authoritative, and a concern is not a failure.
    expect(html).toContain('never overrides the system')
    expect(html).toContain('ds-status-inconclusive')
    expect(html).not.toContain('ds-status-failed')
  })

  it('omits the published status when there is nothing extra to publish', () => {
    const html = renderToStaticMarkup(
      <OutcomeDerivation
        rows={[
          { role: 'system', value: 'passed' },
          { role: 'advisory', value: 'pass' },
        ]}
      />,
    )
    expect(html).toContain('system · deterministic gates')
    expect(html).not.toContain('effective · the status')
  })

  it('reads a concern apart from a failure, and invents no verdict', () => {
    expect(outcomeStatus('passed')).toBe('passed')
    expect(outcomeStatus('pass')).toBe('passed')
    expect(outcomeStatus('pass_with_concerns')).toBe('inconclusive')
    expect(outcomeStatus('passed_with_concerns')).toBe('inconclusive')
    expect(outcomeStatus('hard_gate_failed')).toBe('failed')
    expect(outcomeStatus('infrastructure_error')).toBe('failed')
    // Audit AW-04: an unknown value is unavailable, never a pass or a fail.
    expect(outcomeStatus('not_requested')).toBe('unavailable')
    expect(outcomeStatus('')).toBe('unavailable')
  })
})
