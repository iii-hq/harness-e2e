import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  Button,
  buttonClassName,
  MetricCard,
  type OperationalStatus,
  PageHeader,
  Panel,
  StatusBadge,
} from './index'

describe('design system primitives', () => {
  it('preserves every operational status as a distinct semantic value', () => {
    const statuses: OperationalStatus[] = [
      'passed',
      'failed',
      'inconclusive',
      'unavailable',
      'hard_gate',
      'recommendation',
      'running',
      'cancelling',
      'cancelled',
      'incomplete',
    ]
    const html = renderToStaticMarkup(
      <div>
        {statuses.map((status) => (
          <StatusBadge status={status} key={status} />
        ))}
      </div>,
    )

    for (const status of statuses) {
      expect(html).toContain(`data-status="${status}"`)
      expect(html).toContain(`ds-status-${status}`)
    }
    expect(html).toContain('Hard gate')
    expect(html).toContain('Recommendation')
  })

  it('keeps unavailable metric evidence explicit', () => {
    const html = renderToStaticMarkup(
      <MetricCard
        label="Retained cost"
        value="Not reported"
        detail="No cost evidence was retained."
        tone="unavailable"
      />,
    )

    expect(html).toContain('ds-metric-unavailable')
    expect(html).toContain('Not reported')
    expect(html).not.toContain('>0<')
  })

  it('exposes busy buttons and page hierarchy accessibly', () => {
    const html = renderToStaticMarkup(
      <Panel as="article" tone="spotlight">
        <PageHeader
          headingLevel={2}
          title="Objective outcome"
          summary="Deterministic gates remain authoritative."
          actions={<Button busy>Run evaluation</Button>}
        />
      </Panel>,
    )

    expect(html).toContain('<article')
    expect(html).toContain('<h2>Objective outcome</h2>')
    expect(html).toContain('aria-busy="true"')
    expect(html).toContain('disabled=""')
    expect(html).toContain('ds-button-spinner')
  })

  it('shares button styling with accessible links without changing semantics', () => {
    expect(
      buttonClassName({ variant: 'primary', size: 'large', className: 'cta' }),
    ).toBe('ds-button ds-button-primary ds-button-large cta')
  })
})
