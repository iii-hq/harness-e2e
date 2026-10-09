import {
  Chip,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  SegmentedControl,
  StatusPanel,
} from '@iii-dev/console-ui'
import {
  AlertTriangle,
  ArrowUpDown,
  ChevronRight,
  Ellipsis,
  Equal,
  Plus,
  TrendingDown,
  TrendingUp,
} from 'lucide-react'
import { useMemo, useState } from 'react'
import { type DeltaTone, deltaTone, FactChip, FactList } from '@/design-system'
import { hashForExecution } from '@/hooks/use-hash-route'
import {
  type GroupHighlight,
  type GroupMember,
  groupChanges,
  groupHighlights,
  groupRunnerNote,
  lettersText,
  memberMetric,
  memberValues,
  spreadOf,
  testListCount,
  testListGap,
} from '@/lib/comparison-group'
import type { DashboardExecutionDetail } from '@/lib/dashboard-data-source'
import {
  betterWhen,
  type ComparedMetric,
  type ComparisonGroup,
  type ExecutionComparison,
  metricFigure,
  type ScenarioComparison,
} from '@/lib/execution-comparison'
import { formatDateTime, plural } from '@/lib/format'

/** More than two executions: every one of them, and where each action of a
 *  column leads. */
export type GroupView = {
  /** Every execution, in the order chosen. */
  executions: DashboardExecutionDetail[]
  /** The reference against each other execution, in that order. */
  pairs: ExecutionComparison[]
  group: ComparisonGroup
  /** The tests whose scoring moved with the runner, by member. */
  scoring: Map<string, number[][]>
  /** The hash that reads this execution against the reference in detail. */
  compareHref: (id: string) => string
  /** The hash that makes this execution the reference. */
  referenceHref: (id: string) => string
  /** The hash of the same comparison without these executions. */
  withoutHref: (ids: string[]) => string
  /** Opens the dialog that adds executions to the group. */
  onAdd?: () => void
}

function go(hash: string) {
  window.location.hash = hash
}

/** What a column is called where the page names it: its title, unless every
 *  member shares it; then when it ran. */
function columnNames(members: GroupMember[]) {
  const sameTitle = new Set(members.map((member) => member.title)).size === 1
  return (member: GroupMember) => {
    const when = member.ranAt ? formatDateTime(member.ranAt) : ''
    return sameTitle
      ? { heading: when || member.title, sub: member.source }
      : { heading: member.title, sub: when || member.source }
  }
}

/* ------------------------------------------------------------- summary */

/** What differs across the group, said once, and the warnings that change
 *  how its figures read. */
export function GroupSummary({
  members,
  view,
}: {
  members: GroupMember[]
  view: GroupView
}) {
  const reference = members.find((member) => member.reference) ?? members[0]
  const { changes, same } = groupChanges(
    members,
    testListCount(view.executions),
  )
  const runner = groupRunnerNote(members, view.scoring)
  const gap = testListGap(view.executions, view.group, reference.index)
  const odd = gap ? gap.odd.map((index) => members[index]) : []
  return (
    <section
      className="cmp-changes"
      aria-label="What differs across the group"
      data-group-changes
    >
      <div className="cmp-changes-row">
        <span className="cmp-eyebrow">What differs</span>
        {changes.length > 0 ? (
          <FactList>
            {changes.map((change) => (
              <FactChip
                key={change.label}
                label={change.label}
                value={change.value}
                data-change={change.label}
              />
            ))}
          </FactList>
        ) : null}
        <span className="cmp-faint">
          {changes.length === 0 ? `Nothing recorded differs. ${same}` : same}
        </span>
      </div>
      {gap ? (
        <StatusPanel
          variant="warn"
          className="cmp-status"
          icon={<AlertTriangle size={16} aria-hidden="true" />}
          headline={`${lettersText(gap.odd)} ran ${gap.odd.length === 1 ? 'a different test list' : 'different test lists'}`}
          detail={`${gap.missing} of ${plural(gap.total, 'test')} are missing from at least one execution and leave every total, so every execution is read over the same ${gap.counted}.`}
          action={
            gap.removable ? (
              <a
                className="cmp-act cmp-act-fill"
                href={view.withoutHref(odd.map((member) => member.id))}
                data-remove-odd
              >
                Remove {lettersText(gap.odd)}
              </a>
            ) : undefined
          }
        />
      ) : null}
      {runner ? (
        <StatusPanel
          variant="warn"
          className="cmp-status"
          icon={<AlertTriangle size={16} aria-hidden="true" />}
          headline={runner.headline}
          detail={runner.detail}
        />
      ) : null}
    </section>
  )
}

/* ---------------------------------------------------------- highlights */

const DIRECTION = {
  up: TrendingUp,
  down: TrendingDown,
  mixed: ArrowUpDown,
  same: Equal,
} as const

function highlightTone(item: GroupHighlight): DeltaTone | 'mixed' {
  if (item.direction === 'mixed') return 'mixed'
  return deltaTone(
    item.direction === 'same' ? 'flat' : item.direction,
    item.metric ? betterWhen(item.metric) : 'neither',
  )
}

export function GroupHighlights({
  members,
  onShow,
}: {
  members: GroupMember[]
  onShow: (scenarioId: string, executionId: string | null) => void
}) {
  const { headline, detail, items } = groupHighlights(members)
  const reference = members.find((member) => member.reference) ?? members[0]
  const letter = (id: string | null) =>
    members.find((member) => member.id === id)?.letter ?? null
  return (
    <section
      className="cmp-card cmp-panel"
      aria-labelledby="cmp-group-highlights"
      data-comparison-highlights
    >
      <h2 id="cmp-group-highlights" className="cmp-panel-head">
        Highlights · {plural(members.length, 'execution')} against{' '}
        {reference.letter}
      </h2>
      <div className="cmp-panel-body cmp-group-highlights">
        <p className="cmp-group-headline">{headline}</p>
        <p className="cmp-faint">{detail}</p>
        {items.length > 0 ? (
          <ul className="cmp-highlight-list">
            {items.map((item) => {
              const Icon = DIRECTION[item.direction]
              const target = letter(item.target)
              const body = (
                <>
                  <Icon
                    className="cmp-highlight-icon cmp-tone"
                    data-tone={highlightTone(item)}
                    size={16}
                    aria-hidden="true"
                  />
                  <span className="cmp-highlight-text">
                    {item.test ? (
                      <span className="cmp-mono cmp-strong">{item.test} </span>
                    ) : null}
                    {item.text}
                  </span>
                </>
              )
              const test = item.test
              return (
                <li key={`${item.test}:${item.text}`}>
                  {test ? (
                    <button
                      type="button"
                      className="cmp-highlight"
                      onClick={() => onShow(test, item.target)}
                    >
                      {body}
                      <span className="cmp-show">
                        {target ? `Show ${target}` : 'Show'}
                        <ChevronRight size={16} aria-hidden="true" />
                      </span>
                    </button>
                  ) : (
                    <div className="cmp-highlight">{body}</div>
                  )}
                </li>
              )
            })}
          </ul>
        ) : null}
      </div>
    </section>
  )
}

/* -------------------------------------------------------------- matrix */

const KPIS = ['score', 'completed', 'tokens', 'duration', 'function_calls']
const FIGURES = [
  { value: 'score', label: 'Score' },
  { value: 'tokens', label: 'Tokens' },
  { value: 'duration', label: 'Duration' },
  { value: 'function_calls', label: 'Function calls' },
  { value: 'turns', label: 'Turns' },
] as const
type Figure = (typeof FIGURES)[number]['value']

const REASON: Record<string, string> = {
  missing: 'missing',
  redefined: 'redefined',
  technical_invalid: 'technically invalid',
  undetermined: 'undetermined',
  no_score: 'no score',
}

/** The difference a cell has room for: points for a score, a percentage
 *  for a measure that has one, else the amount. */
export function matrixDelta(metric: ComparedMetric): string {
  if (metric.partial.candidate) return 'partial'
  if (metric.partial.baseline) return 'reference partial'
  const delta = metric.delta
  if (delta === null) return ''
  const shown =
    metric.format === 'score' || metric.format === 'percent_points'
      ? Number(delta.toFixed(1))
      : delta
  if (Math.abs(shown) < 1e-9) return 'same'
  const sign = shown > 0 ? '+' : '−'
  if (metric.format === 'score') return `${sign}${Math.abs(shown)}`
  if (metric.delta_percent !== null && metric.id !== 'completed') {
    const size = Math.abs(metric.delta_percent)
    return `${sign}${size < 10 ? size.toFixed(1) : Math.round(size)}%`
  }
  return `${sign}${metricFigure(metric.format, Math.abs(shown))}`
}

function toneOf(metric: ComparedMetric): DeltaTone {
  const delta = metric.delta
  if (delta === null || metric.partial.baseline || metric.partial.candidate)
    return 'neutral'
  const shown = metric.format === 'score' ? Number(delta.toFixed(1)) : delta
  return deltaTone(
    Math.abs(shown) < 1e-9 ? 'flat' : shown > 0 ? 'up' : 'down',
    betterWhen(metric.id),
  )
}

/** A member's figure and, but for the reference, its difference from it. */
function Figure({
  member,
  first,
  metricId,
  scenarioId = null,
  empty = '—',
}: {
  member: GroupMember
  first: ExecutionComparison
  metricId: string
  scenarioId?: string | null
  empty?: string
}) {
  const { metric, side } = memberMetric(member, first, metricId, scenarioId)
  if (!metric || metric[side] === null)
    return (
      <span className="cmp-matrix-figure">
        <span className="cmp-faint-num">{empty}</span>
        <span className="cmp-matrix-delta" />
      </span>
    )
  const note = member.reference
    ? metric.partial.baseline
      ? 'partial'
      : 'reference'
    : matrixDelta(metric)
  return (
    <span className="cmp-matrix-figure">
      <span className={member.reference ? 'cmp-strong' : undefined}>
        {metricFigure(metric.format, metric[side] ?? 0)}
      </span>
      <span
        className="cmp-matrix-delta cmp-tone"
        data-tone={member.reference ? undefined : toneOf(metric)}
      >
        {note}
      </span>
    </span>
  )
}

/** Where every member's value sits: the reference hollow, the one in
 *  detail filled in ink, the rest in ghost. Scores on 0–100. */
function Spread({
  values,
  members,
  compared,
  format,
  score,
}: {
  values: Array<number | null>
  members: GroupMember[]
  compared: string
  format: ComparedMetric['format']
  score: boolean
}) {
  const range = spreadOf(values)
  if (!range) return <span className="cmp-faint-num">—</span>
  const low = score ? 0 : range.min
  const high = score ? 100 : range.max
  const at = (value: number) =>
    high === low ? 50 : ((value - low) / (high - low)) * 100
  return (
    <span className="cmp-spread">
      <span className="cmp-spread-track" aria-hidden="true">
        {members.map((member) => {
          const value = values[member.index]
          if (value === null) return null
          return (
            <span
              key={member.id}
              className="cmp-spread-dot"
              data-role={
                member.reference
                  ? 'reference'
                  : member.id === compared
                    ? 'compared'
                    : undefined
              }
              style={{ left: `${Math.min(100, Math.max(0, at(value)))}%` }}
            />
          )
        })}
      </span>
      <span className="cmp-spread-text">
        {range.min === range.max
          ? `${metricFigure(format, range.min)} in all`
          : `${metricFigure(format, range.min)}–${metricFigure(format, range.max)}`}
      </span>
    </span>
  )
}

/** A test id that wraps after its underscores, not inside a word. */
function breakable(id: string) {
  return id.split('_').map((part, index) => (
    // biome-ignore lint/suspicious/noArrayIndexKey: the parts of one id, in order
    <span key={index}>
      {index > 0 ? '_' : null}
      {index > 0 ? <wbr /> : null}
      {part}
    </span>
  ))
}

/** Why a test is out of every total, as a chip and the sentence behind it;
 *  a gap names the executions that left it by letter. */
function outChip(
  scenario: ScenarioComparison,
  members: GroupMember[],
  group: ComparisonGroup,
): { label: string; title: string } | null {
  if (scenario.counted) return null
  if (scenario.leftOut)
    return { label: 'left out', title: 'You left it out of the totals.' }
  const gap = group.exclusions.get(scenario.id)
  if (!gap) return { label: 'out', title: 'Out of the totals.' }
  const reason = REASON[gap.reason] ?? gap.reason
  if (gap.reason === 'redefined')
    return {
      label: reason,
      title:
        'Out of every total: the case inputs differ between the executions.',
    }
  const indexes = members
    .filter((member) => gap.executions.includes(member.id))
    .map((member) => member.index)
  const where = lettersText(indexes)
  return {
    label: `${reason} in ${indexes.length > 3 ? plural(indexes.length, 'execution') : where}`,
    title: `Out of every total: ${reason} in ${where}.`,
  }
}

function ColumnMenu({
  member,
  members,
  compared,
  view,
  name,
}: {
  member: GroupMember
  members: GroupMember[]
  compared: string
  view: GroupView
  name: string
}) {
  const reference = members.find((one) => one.reference) ?? members[0]
  const next = members.find((one) => !one.reference)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="cmp-icon-act"
          aria-label={`More for ${member.letter}, ${name}`}
          title={`More for ${member.letter}`}
          data-column-menu={member.id}
        >
          <Ellipsis size={16} aria-hidden="true" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="cmp-menu">
        <DropdownMenuItem onSelect={() => go(hashForExecution(member.id))}>
          Open execution
        </DropdownMenuItem>
        {!member.reference && member.id !== compared ? (
          <DropdownMenuItem onSelect={() => go(view.compareHref(member.id))}>
            Read against {reference.letter} in detail
          </DropdownMenuItem>
        ) : null}
        {!member.reference ? (
          <DropdownMenuItem onSelect={() => go(view.referenceHref(member.id))}>
            <span className="cmp-menu-item">
              <span>Make reference</span>
              <span className="cmp-menu-hint">
                Every figure is read against {member.letter}
              </span>
            </span>
          </DropdownMenuItem>
        ) : null}
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={() => go(view.withoutHref([member.id]))}
          data-remove={member.id}
        >
          <span className="cmp-menu-item">
            <span>Remove from comparison</span>
            <span className="cmp-menu-hint">
              {member.reference && next
                ? `${next.letter} becomes the reference`
                : 'Nothing is deleted'}
            </span>
          </span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** Every execution in a column, each figure against the reference, over the
 *  same tests; a test's figure opens it in the detail below. */
export function GroupMatrix({
  members,
  view,
  compared,
  onOpen,
}: {
  members: GroupMember[]
  view: GroupView
  compared: string
  onOpen: (executionId: string, scenarioId: string) => void
}) {
  const [figure, setFigure] = useState<Figure>('score')
  const [all, setAll] = useState(false)
  const first = view.pairs[0]
  const reference = members.find((member) => member.reference) ?? members[0]
  const names = columnNames(members)
  const role = (member: GroupMember) =>
    member.reference
      ? 'reference'
      : member.id === compared
        ? 'compared'
        : undefined
  const totals = first.totals.filter((metric, index) =>
    all
      ? metric.baseline !== null ||
        view.pairs.some((pair) => pair.totals[index].candidate !== null)
      : KPIS.includes(metric.id),
  )
  // Counted tests first, the most varied score first; then those out.
  const scenarios = useMemo(() => {
    const spread = (scenario: ScenarioComparison) => {
      const range = spreadOf(memberValues(members, first, 'score', scenario.id))
      return range ? range.max - range.min : -1
    }
    return [...first.scenarios].sort(
      (one, two) =>
        Number(two.counted) - Number(one.counted) ||
        spread(two) - spread(one) ||
        one.id.localeCompare(two.id),
    )
  }, [first, members])
  const figureFormat = (id: string, scenarioId: string | null = null) =>
    memberMetric(reference, first, id, scenarioId).metric?.format ?? 'count'
  const versions: Array<{ id: string; label: string; values: string[] }> = [
    {
      id: 'harness',
      label: 'harness',
      values: members.map((member) => member.harness ?? '—'),
    },
    {
      id: 'runner',
      label: 'runner',
      values: members.map((member) => member.runner ?? '—'),
    },
  ]
  const cell = (member: GroupMember) => ({
    'data-role': role(member),
    'data-selected': member.id === compared || undefined,
  })
  return (
    <section
      className="cmp-card cmp-panel cmp-group"
      aria-labelledby="cmp-group-title"
      data-comparison-group
    >
      <div className="cmp-panel-head">
        <h2 id="cmp-group-title">
          {plural(members.length, 'execution')} side by side
        </h2>
        {view.onAdd ? (
          <button
            type="button"
            className="cmp-act"
            onClick={view.onAdd}
            data-add-executions
          >
            <Plus size={16} aria-hidden="true" />
            Add executions…
          </button>
        ) : null}
      </div>
      <p className="cmp-faint cmp-panel-note">
        Each figure against {reference.letter}, over the same{' '}
        {plural(
          first.scenarios.filter((scenario) => scenario.counted).length,
          'test',
        )}
        . Pick a test's figure to read it in detail below.
      </p>
      <div className="cmp-matrix-scroll">
        <table className="cmp-matrix" data-comparison-matrix>
          <caption className="ep-sr">
            Every execution against the reference, {reference.letter}
          </caption>
          <thead>
            <tr>
              <th scope="col" className="cmp-matrix-corner">
                Against {reference.letter}
              </th>
              {members.map((member) => {
                const name = names(member)
                return (
                  <th
                    key={member.id}
                    scope="col"
                    data-matrix-execution={member.id}
                    {...cell(member)}
                  >
                    <span className="cmp-matrix-head">
                      <span className="cmp-letter">{member.letter}</span>
                      {role(member) ? (
                        <span className="cmp-matrix-role">
                          {member.reference ? 'Reference' : 'In detail'}
                        </span>
                      ) : null}
                      <ColumnMenu
                        member={member}
                        members={members}
                        compared={compared}
                        view={view}
                        name={name.heading}
                      />
                    </span>
                    <span className="cmp-matrix-title" title={name.heading}>
                      {name.heading}
                    </span>
                    <span className="cmp-matrix-id" title={member.id}>
                      {name.sub}
                    </span>
                  </th>
                )
              })}
              <th scope="col" className="cmp-matrix-spread">
                <span className="cmp-strong">Spread</span>
                <span className="cmp-faint-num">
                  hollow {reference.letter}
                  {members.find((member) => member.id === compared)
                    ? `, filled ${members.find((member) => member.id === compared)?.letter}`
                    : ''}
                </span>
              </th>
            </tr>
          </thead>
          <tbody aria-label="Versions">
            <tr className="cmp-matrix-group">
              <th scope="colgroup" colSpan={members.length + 2}>
                <span className="cmp-matrix-sticky">
                  <span className="cmp-eyebrow">Versions</span>
                  <span className="cmp-faint">
                    Every value against {reference.letter}
                  </span>
                </span>
              </th>
            </tr>
            {versions.map((row) => (
              <tr key={row.id} data-matrix-version={row.id}>
                <th scope="row">{row.label}</th>
                {members.map((member) => {
                  const value = row.values[member.index]
                  const same = value === row.values[reference.index]
                  return (
                    <td key={member.id} {...cell(member)}>
                      <span className="cmp-matrix-figure">
                        <span
                          className={
                            member.reference || !same
                              ? 'cmp-strong'
                              : 'cmp-faint-num'
                          }
                        >
                          {value}
                        </span>
                        <span
                          className="cmp-matrix-delta cmp-tone"
                          data-tone={
                            member.reference || same ? undefined : 'warn'
                          }
                        >
                          {member.reference
                            ? 'reference'
                            : same
                              ? 'same'
                              : 'changed'}
                        </span>
                      </span>
                    </td>
                  )
                })}
                <td className="cmp-matrix-spread cmp-faint-num">
                  {plural(new Set(row.values).size, 'version')}
                </td>
              </tr>
            ))}
            <tr data-matrix-version="others">
              <th scope="row">other workers</th>
              {members.map((member) => (
                <td key={member.id} {...cell(member)}>
                  <span className="cmp-matrix-figure">
                    <span>
                      {member.otherChanges === null
                        ? '—'
                        : String(member.otherChanges)}
                    </span>
                    <span className="cmp-matrix-delta">
                      {member.reference
                        ? 'reference'
                        : member.otherChanges
                          ? 'changed'
                          : 'same'}
                    </span>
                  </span>
                </td>
              ))}
              <td className="cmp-matrix-spread cmp-faint-num">
                at another build
              </td>
            </tr>
          </tbody>
          <tbody aria-label="Totals">
            <tr className="cmp-matrix-group">
              <th scope="colgroup" colSpan={members.length + 2}>
                <span className="cmp-matrix-sticky">
                  <span className="cmp-eyebrow">Totals</span>
                  <button
                    type="button"
                    className="cmp-act"
                    aria-pressed={all}
                    data-selected={all || undefined}
                    onClick={() => setAll(!all)}
                  >
                    All metrics
                  </button>
                </span>
              </th>
            </tr>
            {totals.map((metric) => {
              const values = memberValues(members, first, metric.id)
              return (
                <tr key={metric.id} data-matrix-metric={metric.id}>
                  <th scope="row">{metric.label}</th>
                  {members.map((member) => (
                    <td key={member.id} {...cell(member)}>
                      <Figure
                        member={member}
                        first={first}
                        metricId={metric.id}
                      />
                    </td>
                  ))}
                  <td className="cmp-matrix-spread">
                    <Spread
                      values={values}
                      members={members}
                      compared={compared}
                      format={metric.format}
                      score={metric.id === 'score'}
                    />
                  </td>
                </tr>
              )
            })}
          </tbody>
          <tbody aria-label="Results by test">
            <tr className="cmp-matrix-group">
              <th scope="colgroup" colSpan={members.length + 2}>
                <span className="cmp-matrix-sticky">
                  <span className="cmp-eyebrow">Results by test</span>
                  <SegmentedControl
                    variant="radio"
                    aria-label="Figure in each cell"
                    className="cmp-matrix-figures"
                    value={figure}
                    onChange={setFigure}
                    options={FIGURES.map((option) => ({ ...option }))}
                  />
                  <span className="cmp-faint">Most varied first</span>
                </span>
              </th>
            </tr>
            {scenarios.map((scenario) => {
              const out = outChip(scenario, members, view.group)
              const moved = view.scoring.get(scenario.id)
              const values = memberValues(members, first, figure, scenario.id)
              return (
                <tr
                  key={scenario.id}
                  data-matrix-scenario={scenario.id}
                  data-counted={scenario.counted}
                >
                  <th scope="row">
                    <span className="cmp-matrix-test">
                      {breakable(scenario.id)}
                    </span>
                    {out || moved ? (
                      <span className="cmp-matrix-chips">
                        {out ? (
                          <Chip title={out.title}>{out.label}</Chip>
                        ) : null}
                        {moved ? (
                          <Chip
                            tone="warning"
                            title={`The scoring changed with the runner: ${moved.map(lettersText).join(' · ')}.`}
                          >
                            scoring changed
                          </Chip>
                        ) : null}
                      </span>
                    ) : null}
                  </th>
                  {members.map((member) => {
                    const { metric, side } = memberMetric(
                      member,
                      first,
                      'score',
                      scenario.id,
                    )
                    const own = (member.pair ?? first).scenarios.find(
                      (row) => row.id === scenario.id,
                    )
                    const state = own?.sides[member.reference ? 'a' : 'b'].state
                    // Only a score that moved tints its cell.
                    const tint =
                      figure === 'score' && !member.reference && metric
                        ? toneOf(metric)
                        : undefined
                    return (
                      <td
                        key={member.id}
                        {...cell(member)}
                        data-tint={
                          tint === 'positive' || tint === 'negative'
                            ? tint
                            : undefined
                        }
                      >
                        <button
                          type="button"
                          className="cmp-matrix-cell"
                          data-matrix-cell={member.id}
                          onClick={() => onOpen(member.id, scenario.id)}
                        >
                          <span className="ep-sr">
                            {scenario.id} in {member.letter}:{' '}
                          </span>
                          <Figure
                            member={member}
                            first={first}
                            metricId={figure}
                            scenarioId={scenario.id}
                            empty={
                              metric?.[side] === null && state
                                ? state.replaceAll('_', ' ')
                                : '—'
                            }
                          />
                        </button>
                      </td>
                    )
                  })}
                  <td className="cmp-matrix-spread">
                    <Spread
                      values={values}
                      members={members}
                      compared={compared}
                      format={figureFormat(figure, scenario.id)}
                      score={figure === 'score'}
                    />
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
    </section>
  )
}

/* --------------------------------------------------------------- pair */

/** The pair read below: the reference against the member picked, and the
 *  letters to pick another. */
export function PairHeading({
  members,
  compared,
  view,
}: {
  members: GroupMember[]
  compared: string
  view: GroupView
}) {
  const reference = members.find((member) => member.reference) ?? members[0]
  const detail = members.find((member) => member.id === compared)
  const names = columnNames(members)
  if (!detail) return null
  return (
    <div className="cmp-pair-head" data-comparison-pair>
      <div className="cmp-pair-copy">
        <span className="cmp-eyebrow">In detail</span>
        <h2 className="cmp-pair-title">
          {reference.letter} against {detail.letter} · {names(detail).heading}
        </h2>
        <p className="cmp-faint">
          The two-execution comparison for this pair. Pick a letter, a column or
          a figure above to read another one.
        </p>
      </div>
      <SegmentedControl
        variant="radio"
        aria-label={`Execution read against ${reference.letter}`}
        className="cmp-pair-picks"
        value={compared}
        onChange={(id) => go(view.compareHref(id))}
        options={members
          .filter((member) => !member.reference)
          .map((member) => ({
            value: member.id,
            label: member.letter,
            title: names(member).heading,
          }))}
      />
    </div>
  )
}
