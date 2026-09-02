import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import {
  Button,
  buttonClassName,
  Callout,
  DataTable,
  DataTableRow,
  DeltaValue,
  Dialog,
  deltaTone,
  EmptyState,
  Field,
  FilterChip,
  FilterChipGroup,
  fieldDescribedBy,
  Input,
  MetricCard,
  numericCellClassName,
  type OperationalStatus,
  PageHeader,
  Panel,
  Select,
  StatusBadge,
  Textarea,
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

// Foundation primitives (audit DS-08..DS-11, DK-07, RD-03, RD-12, A11Y-04).
// Each story renders the primitive the way a page will use it.
describe('design system foundation primitives', () => {
  it('renders filter chips as pressed toggles with their counts', () => {
    const html = renderToStaticMarkup(
      <FilterChipGroup label="Result">
        <FilterChip active count={12}>
          all
        </FilterChip>
        <FilterChip count={3}>failed</FilterChip>
        <FilterChip disabled count={0}>
          cancelled
        </FilterChip>
      </FilterChipGroup>,
    )
    expect(html).toContain('<fieldset class="ds-chip-group">')
    expect(html).toContain('<legend class="ds-visually-hidden">Result</legend>')
    expect(html).toContain(
      'class="ds-chip ds-chip-active" type="button" aria-pressed="true"',
    )
    expect(html).toContain('aria-pressed="false"')
    expect(html).toContain('<span class="ds-chip-count">3</span>')
    expect(html).toContain('disabled=""')
  })

  it('gives tables a caption, sticky headers, numeric cells and row links', () => {
    const html = renderToStaticMarkup(
      <DataTable caption="Executions, 3 of 20" sticky minWidth="48rem">
        <thead>
          <tr>
            <th scope="col">Execution</th>
            <th scope="col" className={numericCellClassName}>
              Tokens
            </th>
          </tr>
        </thead>
        <tbody>
          <DataTableRow href="#/execution/abc">
            <td>
              <a href="#/execution/abc">control-plane run</a>
            </td>
            <td className={numericCellClassName}>7,918</td>
          </DataTableRow>
          <DataTableRow>
            <td>plain row</td>
            <td className={numericCellClassName}>—</td>
          </DataTableRow>
        </tbody>
      </DataTable>,
    )
    expect(html).toContain(
      '<caption class="ds-visually-hidden">Executions, 3 of 20</caption>',
    )
    expect(html).toContain('class="ds-table ds-table-sticky"')
    expect(html).toContain('min-width:48rem')
    expect(html).toContain(
      '<tr class="ds-table-row-link" data-href="#/execution/abc">',
    )
    expect(html).toContain('<tr><td>plain row</td>')
    expect(html.match(/ds-table-numeric/g)).toHaveLength(3)
    const visible = renderToStaticMarkup(
      <DataTable caption="Visible caption" captionVisible>
        <tbody />
      </DataTable>,
    )
    expect(visible).toContain(
      '<caption class="ds-table-caption">Visible caption</caption>',
    )
  })

  it('renders empty states with a heading, an action and an error tone', () => {
    const html = renderToStaticMarkup(
      <EmptyState
        title="No tests match these filters"
        description="Try another name or clear the filters."
        actions={<Button size="compact">clear filters</Button>}
      />,
    )
    expect(html).toContain('<section class="ds-empty">')
    expect(html).toContain(
      '<h2 class="ds-empty-title">No tests match these filters</h2>',
    )
    expect(html).toContain('clear filters')
    expect(html).not.toContain('role="alert"')
    const error = renderToStaticMarkup(
      <EmptyState title="Catalog unavailable" tone="error" headingLevel={3} />,
    )
    expect(error).toContain('role="alert"')
    expect(error).toContain('<h3 class="ds-empty-title">')
    expect(error).toContain('ds-empty-error')
  })

  it('signs deltas, marks the direction and colours by the outcome', () => {
    const percent = (value: number) => `${value.toFixed(1)}%`
    const up = renderToStaticMarkup(
      <DeltaValue value={3.21} format={percent} betterWhen="higher" />,
    )
    expect(up).toContain('ds-delta-positive')
    expect(up).toContain('data-direction="up"')
    expect(up).toContain('+3.2%')
    expect(up).toContain('<span aria-hidden="true">▲</span>')
    expect(up).toContain(', better')
    const worseWhenLowerIsBetter = renderToStaticMarkup(
      <DeltaValue value={120} betterWhen="lower" />,
    )
    expect(worseWhenLowerIsBetter).toContain('ds-delta-negative')
    expect(worseWhenLowerIsBetter).toContain('+120')
    const down = renderToStaticMarkup(
      <DeltaValue value={-2} betterWhen="lower" />,
    )
    expect(down).toContain('ds-delta-positive')
    expect(down).toContain('−2')
    expect(down).toContain('▼')
    const flat = renderToStaticMarkup(
      <DeltaValue value={0} betterWhen="higher" />,
    )
    expect(flat).toContain('ds-delta-neutral')
    expect(flat).toContain('±0')
    expect(flat).not.toContain('▲')
    const missing = renderToStaticMarkup(<DeltaValue value={null} />)
    expect(missing).toContain('ds-delta-unavailable')
    expect(missing).toContain('—')
    expect(missing).toContain('not reported')
    expect(deltaTone('up', 'neither')).toBe('neutral')
  })

  it('gives callouts a role that matches their urgency', () => {
    expect(
      renderToStaticMarkup(
        <Callout title="Judge needs review">Two runs disagreed.</Callout>,
      ),
    ).toContain('class="ds-callout ds-callout-info" role="note"')
    expect(renderToStaticMarkup(<Callout tone="warning">x</Callout>)).toContain(
      'role="status"',
    )
    expect(renderToStaticMarkup(<Callout tone="danger">x</Callout>)).toContain(
      'role="alert"',
    )
    expect(
      renderToStaticMarkup(
        <Callout tone="success" title="Saved">
          done
        </Callout>,
      ),
    ).toContain('<strong class="ds-callout-title">Saved</strong>')
  })

  it('wires fields to their controls, hints and errors', () => {
    const html = renderToStaticMarkup(
      <Field
        label="Plan label"
        htmlFor="plan-label"
        meta="required"
        hint="Shown in the plans list."
        error="Add a plan label."
      >
        <Input
          id="plan-label"
          aria-describedby={fieldDescribedBy('plan-label', {
            hint: true,
            error: true,
          })}
          aria-invalid
        />
      </Field>,
    )
    expect(html).toContain('class="ds-field ds-field-invalid"')
    expect(html).toContain('<label class="ds-field-label" for="plan-label">')
    expect(html).toContain('<span class="ds-field-meta">required</span>')
    expect(html).toContain('id="plan-label-hint"')
    expect(html).toContain(
      'class="ds-field-error" id="plan-label-error" role="alert"',
    )
    expect(html).toContain(
      'aria-describedby="plan-label-hint plan-label-error"',
    )
    expect(fieldDescribedBy('x', {})).toBeUndefined()
    const controls = renderToStaticMarkup(
      <>
        <Select id="s" aria-label="Lifecycle">
          <option>all</option>
        </Select>
        <Textarea id="t" />
      </>,
    )
    expect(controls).toContain(
      '<span class="ds-select"><select class="ds-input ds-select-control"',
    )
    expect(controls).toContain(
      '<span class="ds-select-chevron" aria-hidden="true">',
    )
    expect(controls).toContain('<textarea class="ds-input ds-textarea"')
  })

  it('renders a breadcrumb trail with the current page marked', () => {
    const html = renderToStaticMarkup(
      <PageHeader
        title="control-plane run"
        summary="12 scenarios"
        breadcrumb={[
          { label: 'executions', href: '#/executions' },
          { label: 'control-plane run' },
        ]}
      />,
    )
    expect(html).toContain(
      '<nav class="ds-breadcrumb" aria-label="Breadcrumb">',
    )
    expect(html).toContain('<a href="#/executions">executions</a>')
    expect(html).toContain('<span aria-current="page">control-plane run</span>')
    expect(
      renderToStaticMarkup(<PageHeader title="t" summary="s" />),
    ).not.toContain('ds-breadcrumb')
  })
})

describe('design system dialog', () => {
  it('renders header, scrolling body and footer with the title as the label', () => {
    const html = renderToStaticMarkup(
      <Dialog
        open
        onClose={() => undefined}
        size="lg"
        tall
        kicker="Evidence record"
        title="local calc smoke"
        description="run f27f…1840"
        closeLabel="Close assessment detail"
        actions={<span>badge</span>}
        footer={<button type="button">done</button>}
        bodyPadding
      >
        body
      </Dialog>,
    )
    expect(
      html.startsWith('<dialog class="ds-dialog ds-dialog-lg ds-dialog-tall"'),
    ).toBe(true)
    const labelledBy = html.match(/aria-labelledby="([^"]+)"/)?.[1]
    const describedBy = html.match(/aria-describedby="([^"]+)"/)?.[1]
    expect(labelledBy).toBeTruthy()
    expect(html).toContain(
      `<h2 id="${labelledBy}" tabindex="-1" class="ds-dialog-title">local calc smoke</h2>`,
    )
    expect(html).toContain(
      `<p id="${describedBy}" class="ds-dialog-description">run f27f…1840</p>`,
    )
    expect(html).toContain('<span class="ds-label">Evidence record</span>')
    expect(html).toContain(
      '<div class="ds-dialog-actions"><span>badge</span><button class="ds-dialog-close" type="button" aria-label="Close assessment detail">',
    )
    expect(html).toContain(
      '<div class="ds-dialog-body ds-dialog-body-padded">body</div>',
    )
    expect(html).toContain(
      '<footer class="ds-dialog-footer"><button type="button">done</button></footer>',
    )
  })

  it('omits the body and description when there is nothing to show', () => {
    const html = renderToStaticMarkup(
      <Dialog open onClose={() => undefined} size="sm" title="Discard?" />,
    )
    expect(html).toContain('ds-dialog-sm')
    expect(html).not.toContain('ds-dialog-body')
    expect(html).not.toContain('aria-describedby')
    expect(html).not.toContain('<footer')
  })
})
