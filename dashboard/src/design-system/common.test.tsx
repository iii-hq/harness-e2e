import { isValidElement, type ReactElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
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

  it('paints the label by its tone only when asked', () => {
    expect(renderToStaticMarkup(<StatusLabel state="not_run" />)).not.toContain(
      'data-tinted',
    )
    const tinted = renderToStaticMarkup(<StatusLabel state="not_run" tinted />)
    expect(tinted).toContain('data-tinted="true"')
    expect(tinted).toContain('data-tone="alert"')
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
    expect(html).toMatch(/^<li class="ds-fact"/)
    expect(html).toContain('title="Where: GitHub"')
    expect(html).toContain('aria-label="Where: GitHub"')
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
    for (const attribute of ['title', 'aria-label'])
      expect(html).toContain(
        `${attribute}="Executor image: ghcr.io/iii-hq/harness-e2e:tools-d9a8b54a2c85"`,
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
      /^<ul role="list" class="ds-fact-list" aria-label="Execution facts">/,
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

  it('keeps a disabled item focusable, with its reason at full contrast', () => {
    const deleteItem = html.split('role="menuitem"').at(-1) ?? ''
    expect(deleteItem).not.toContain('data-disabled')
    expect(deleteItem).toContain('tabindex="-1"')
    expect(deleteItem).toContain(
      '<span class="ds-row-menu-label">Delete…</span>',
    )
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

// The element tree RowMenu returns, walked without rendering, to reach the
// handlers static markup cannot fire.
function elements(node: ReactNode): ReactElement<Record<string, unknown>>[] {
  if (Array.isArray(node)) return node.flatMap(elements)
  if (!isValidElement<Record<string, unknown>>(node)) return []
  return [node, ...elements(node.props.children as ReactNode)]
}

describe('RowMenu events', () => {
  const opened = vi.fn()
  const tree = elements(
    RowMenu({
      label: 'Actions for Regression',
      items: [
        { label: 'Open', onSelect: opened },
        { label: 'Delete…', disabledReason: 'Finish or cancel it first' },
      ],
    }),
  )
  const byClass = (name: string) =>
    tree.filter((element) => element.props.className === name)

  it('keeps clicks and keys from reaching the row', () => {
    const [trigger] = byClass('ds-row-menu-trigger')
    const [menu] = byClass('ds-row-menu')
    for (const element of [trigger, menu])
      for (const handler of ['onClick', 'onKeyDown']) {
        const event = { stopPropagation: vi.fn() }
        ;(element.props[handler] as (event: unknown) => void)(event)
        expect(event.stopPropagation).toHaveBeenCalled()
      }
  })

  it('selects an enabled item and holds a disabled one open', () => {
    const [open, remove] = byClass('ds-row-menu-item')
    ;(open.props.onSelect as () => void)()
    expect(opened).toHaveBeenCalled()
    const event = new Event('menu.itemSelect', { cancelable: true })
    ;(remove.props.onSelect as (event: Event) => void)(event)
    expect(event.defaultPrevented).toBe(true)
  })
})
