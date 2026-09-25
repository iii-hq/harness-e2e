import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { RESULT_STATES, type ResultState } from '@/lib/result-status'
import { FactChip, FactList, RowMenu, StatusLabel } from './index'

describe('StatusLabel', () => {
  it('renders a hidden 6px dot and the state label for every state', () => {
    for (const state of Object.keys(RESULT_STATES) as ResultState[]) {
      const html = renderToStaticMarkup(<StatusLabel state={state} />)
      expect(html).toContain(`data-state="${state}"`)
      expect(html).toContain('aria-hidden="true"')
      expect(html).toContain(`>${RESULT_STATES[state].label}</span>`)
    }
  })

  it('pulses only while live', () => {
    expect(renderToStaticMarkup(<StatusLabel state="running" />)).toContain(
      'pulse-dot',
    )
    for (const state of ['passed', 'queued', 'waiting', 'cancelled'] as const)
      expect(renderToStaticMarkup(<StatusLabel state={state} />)).not.toContain(
        'pulse-dot',
      )
  })

  // The host dot paints its tone with a bg-<tone> class; it has no ghost.
  it('maps tones onto the host dot, ghost drawn by the label', () => {
    const passed = renderToStaticMarkup(<StatusLabel state="passed" />)
    expect(passed).toContain('data-tone="ok"')
    expect(passed).toContain('bg-ok')
    const queued = renderToStaticMarkup(<StatusLabel state="queued" />)
    expect(queued).toContain('data-tone="ghost"')
    expect(queued).toContain('bg-ink')
  })

  it('takes a label of its own and keeps the state tone', () => {
    const html = renderToStaticMarkup(
      <StatusLabel state="running" label="Cancelling" />,
    )
    expect(html).toContain('>Cancelling</span>')
    expect(html).toContain('data-tone="accent"')
  })
})

describe('FactChip', () => {
  it('labels a mono value and titles it whole', () => {
    const html = renderToStaticMarkup(<FactChip label="Where" value="GitHub" />)
    expect(html).toContain('title="Where: GitHub"')
    expect(html).toContain('<span class="ds-fact-label">Where</span>')
    expect(html).toContain('<span class="ds-fact-value">GitHub</span>')
  })

  it('keeps the whole value in the title when the chip shows it short', () => {
    const html = renderToStaticMarkup(
      <FactChip
        label="Executor image"
        value="tools-d9a8b54a2c85"
        full="ghcr.io/iii-hq/harness-e2e:tools-d9a8b54a2c85"
      />,
    )
    expect(html).toContain(
      'title="Executor image: ghcr.io/iii-hq/harness-e2e:tools-d9a8b54a2c85"',
    )
    expect(html).toContain('>tools-d9a8b54a2c85<')
  })

  it('lines chips up in a wrapping list', () => {
    const html = renderToStaticMarkup(
      <FactList aria-label="Execution facts">
        <FactChip label="Model" value="deepseek/deepseek-flash" />
        <FactChip label="Runner" value="0.14.0" />
      </FactList>,
    )
    expect(html).toMatch(
      /^<div class="ds-fact-list" aria-label="Execution facts">/,
    )
    expect(html.match(/class="ds-fact"/g)).toHaveLength(2)
  })
})

describe('RowMenu', () => {
  const html = renderToStaticMarkup(
    <RowMenu
      label="Actions for Regression"
      items={[
        { label: 'Open', onSelect() {} },
        {
          label: 'Import again',
          hint: 'Replaces its evidence with the run’s',
          onSelect() {},
        },
        { label: 'Copy execution id', onSelect() {} },
        {
          label: 'Delete…',
          danger: true,
          separator: true,
          disabledReason: 'Finish or cancel it first',
        },
      ]}
    />,
  )

  const tag = (pattern: RegExp) => html.match(pattern)?.[0] ?? ''

  it('opens from a named ⋯ button', () => {
    const trigger = tag(/<button[^>]*ds-row-menu-trigger[^>]*>/)
    expect(trigger).toContain('type="button"')
    expect(trigger).toContain('aria-label="Actions for Regression"')
    expect(trigger).toContain('aria-haspopup="menu"')
    expect(trigger).toContain('aria-expanded="false"')
    expect(tag(/<div[^>]*role="menu"[^>]*>/)).toContain(
      'aria-label="Actions for Regression"',
    )
  })

  it('shows hints under their items and separates the dangerous one', () => {
    expect(html).toContain(
      '<span class="ds-row-menu-hint">Replaces its evidence with the run’s</span>',
    )
    const separator = html.indexOf('role="separator"')
    const danger = html.indexOf('data-danger="true"')
    expect(separator).toBeGreaterThan(html.indexOf('Copy execution id'))
    expect(danger).toBeGreaterThan(separator)
  })

  it('disables an item with its reason in sight', () => {
    const deleteItem = html.split('role="menuitem"').at(-1) ?? ''

    expect(deleteItem).toContain('aria-disabled="true"')
    expect(deleteItem).toContain(
      '<span class="ds-row-menu-hint">Finish or cancel it first</span>',
    )
    expect(html.match(/aria-disabled/g)).toHaveLength(1)
  })
})
