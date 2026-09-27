import { Checkbox } from '@iii-dev/console-ui'
import {
  AlertTriangle,
  ArrowLeftRight,
  ArrowRight,
  ChevronRight,
  Equal,
  Info,
  RotateCcw,
  TrendingDown,
  TrendingUp,
} from 'lucide-react'
import {
  Fragment,
  type MouseEvent,
  type ReactNode,
  useMemo,
  useRef,
  useState,
} from 'react'
import { DisclosureLayer } from '@/components/DisclosureLayer'
import {
  type Image,
  ScreenshotViewer,
  useScreenshotImages,
} from '@/components/execution/screenshots'
import {
  FactChip,
  FactList,
  isInteractiveTarget,
  StatusLabel,
} from '@/design-system'
import { hashForExecution } from '@/hooks/use-hash-route'
import type {
  DashboardDataBridge,
  DashboardExecutionDetail,
} from '@/lib/dashboard-data-source'
import {
  type ComparedMetric,
  type ComparisonSide,
  compareRuns,
  comparisonHighlights,
  type ExecutionComparison,
  exclusionPhrase,
  gapPhrase,
  metricFigure,
  rerunPhrase,
  runnerWarning,
  type ScenarioComparison,
  type StackComparison,
  stackChanges,
} from '@/lib/execution-comparison'
import {
  buildExecutionPresentation,
  executionResult,
} from '@/lib/execution-view'
import { formatDuration, formatTokens, plural } from '@/lib/format'
import { runResultState } from '@/lib/result-status'
import { type ScreenshotEntry, screenshotsOf } from '@/lib/screenshots'
import '@/components/execution/execution-page.css'
import './compare.css'

export type Sides = { a: DashboardExecutionDetail; b: DashboardExecutionDetail }
type Which = 'a' | 'b'

const ROLE: Record<Which, string> = { a: 'Reference', b: 'Compared' }
const LIVE = ['running', 'importing', 'cancelling']

/* ------------------------------------------------------------- figures */

type Side = 'baseline' | 'candidate'

/** One side's figure, or a dash where it reported none. */
export function valueText(metric: ComparedMetric, side: Side): string {
  const value = metric[side]
  return value === null ? '—' : metricFigure(metric.format, value)
}

function sidesText(flags: Record<Side, boolean | number>) {
  return [flags.baseline ? 'A' : null, flags.candidate ? 'B' : null]
    .filter(Boolean)
    .join(' and ')
}

/** For figures over every run: how many of them are out of the totals. */
export function outsideText(metric: ComparedMetric): string | null {
  const outside = metric.outside
  if (!outside || (!outside.baseline && !outside.candidate)) return null
  const parts = (['baseline', 'candidate'] as const).flatMap((side) =>
    outside[side]
      ? [
          `${plural(outside[side], 'run')} in ${side === 'baseline' ? 'A' : 'B'}`,
        ]
      : [],
  )
  return `${parts.join(', ')} out of the totals`
}

/** B minus A, with the relative change where it means something. Only the
 *  difference: no side is called better. A side short of runs is partial,
 *  and no difference is taken from it. */
export function deltaText(metric: ComparedMetric): string {
  if (metric.delta === null)
    return metric.partial.baseline || metric.partial.candidate
      ? `${sidesText(metric.partial)} partial`
      : metric.baseline === null && metric.candidate === null
        ? ''
        : 'not comparable'
  if (Math.abs(metric.delta) < 1e-9) return 'no change'
  const sign = metric.delta > 0 ? '+' : '−'
  const size = Math.abs(metric.delta)
  if (metric.format === 'score') return `${sign}${Number(size.toFixed(1))} pts`
  if (metric.format === 'percent_points')
    return `${sign}${Number(size.toFixed(1))} pp`
  // A count of runs moves by runs, not by a share of them.
  const relative =
    metric.delta_percent === null ||
    metric.id === 'completed' ||
    metric.id === 'technical_failures'
      ? ''
      : ` · ${sign}${Math.abs(metric.delta_percent).toFixed(Math.abs(metric.delta_percent) < 10 ? 1 : 0)}%`
  return `${sign}${metricFigure(metric.format, size)}${relative}`
}

/** A run's state where its score would be: `infrastructure error`. */
function stateText(state: string | null) {
  return state ? state.replaceAll('_', ' ') : '—'
}

function metricOf(scenario: ScenarioComparison, id: string) {
  return scenario.metrics.find((metric) => metric.id === id)
}

function scoreDelta(scenario: ScenarioComparison): number | null {
  return metricOf(scenario, 'score')?.delta ?? null
}

/** Largest score change first; tests without one last, then by id. */
export function byScoreChange(scenarios: ScenarioComparison[]) {
  return [...scenarios].sort((one, two) => {
    const a = scoreDelta(one)
    const b = scoreDelta(two)
    if (a === null || b === null)
      return a === b ? one.id.localeCompare(two.id) : a === null ? 1 : -1
    return Math.abs(b) - Math.abs(a) || one.id.localeCompare(two.id)
  })
}

const REASON: Record<string, string> = {
  missing: 'missing',
  redefined: 'redefined',
  technical_invalid: 'technically invalid',
  undetermined: 'undetermined',
  no_score: 'no score',
}

/** Why a test is out of the totals, short enough for a pill. */
function outLabel(scenario: ScenarioComparison): string | null {
  if (scenario.counted) return null
  if (scenario.leftOut) return 'left out'
  const exclusion = scenario.exclusion
  if (!exclusion) return 'out'
  return exclusion.reason === 'missing' || exclusion.reason === 'no_score'
    ? `${REASON[exclusion.reason]} in ${exclusion.sides.map((side) => side.toUpperCase()).join(' and ')}`
    : REASON[exclusion.reason]
}

function listText(items: string[]) {
  if (items.length <= 1) return items.join('')
  return `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`
}

/* --------------------------------------------------------------- sides */

function SideCard({
  which,
  side,
  detail,
}: {
  which: Which
  side: ComparisonSide
  detail: DashboardExecutionDetail
}) {
  // The execution's result as the list and its page say it.
  const result = executionResult(buildExecutionPresentation(detail))
  return (
    <article
      className="cmp-side"
      aria-label={`${which.toUpperCase()} · ${ROLE[which]}`}
      data-comparison-side={which}
    >
      <div className="cmp-side-top">
        <span className="cmp-letter" aria-hidden="true">
          {which.toUpperCase()}
        </span>
        <span className="cmp-faint">{ROLE[which]}</span>
        <StatusLabel
          className="cmp-side-state"
          state={result.state}
          label={result.label}
        />
      </div>
      <p className="cmp-side-title">{side.title}</p>
      <p className="cmp-side-meta">
        {[
          side.subject,
          side.profile === null
            ? 'profile not recorded'
            : side.profile === 'no profile'
              ? 'no profile'
              : `profile ${side.profile}`,
          side.suite ?? 'suite not recorded',
        ].join(' · ')}
      </p>
      <div className="cmp-side-foot">
        <span className="cmp-side-origin" title={side.origin}>
          {side.origin}
        </span>
        <a className="cmp-open" href={hashForExecution(side.id)}>
          Open execution
          <ArrowRight size={16} aria-hidden="true" />
        </a>
      </div>
    </article>
  )
}

/** What the two runs share, said once: "Same suite, model and profile." */
function sameSentence(comparison: ExecutionComparison): string {
  const { a, b, parameters, runner } = comparison
  const changed = new Set(parameters.map((change) => change.field))
  const same = [
    a.suite !== null && b.suite !== null && !changed.has('suite')
      ? 'suite'
      : null,
    !changed.has('scenarios') ? 'tests' : null,
    !changed.has('model') && !changed.has('provider') ? 'model' : null,
    a.profile !== null && b.profile !== null && !changed.has('profile')
      ? 'profile'
      : null,
    runner.a !== null && runner.b !== null && !runner.differs ? 'runner' : null,
  ].filter((item): item is string => item !== null)
  return same.length > 0 ? `Same ${listText(same)}.` : ''
}

/* -------------------------------------------------------------- picker */

function TestPicker({
  comparison,
  onCount,
}: {
  comparison: ExecutionComparison
  onCount: (ids: string[] | null) => void
}) {
  const { scenarios, exclusions } = comparison
  const [open, setOpen] = useState(
    () =>
      exclusions.length > 0 ||
      scenarios.some((scenario) => scenario.leftOut || !scenario.counted),
  )
  const counted = scenarios.filter((scenario) => scenario.counted)
  const ids = (list: ScenarioComparison[]) =>
    list.map((scenario) => scenario.id)
  const total = scenarios.length
  const out = exclusions.filter((exclusion) => exclusion.applied).length
  return (
    <section
      className="cmp-card cmp-picker"
      aria-label="Tests in this comparison"
    >
      <button
        type="button"
        className="cmp-picker-toggle"
        aria-expanded={open}
        aria-controls="cmp-picker-body"
        onClick={() => setOpen(!open)}
      >
        <ChevronRight
          size={16}
          aria-hidden="true"
          className={open ? 'cmp-chevron cmp-rot' : 'cmp-chevron'}
        />
        <span className="cmp-picker-copy">
          <span className="cmp-picker-title">Tests in this comparison</span>
          <span className="cmp-faint">
            {counted.length === total
              ? `All ${plural(total, 'test')} counted`
              : `${counted.length} of ${total} counted · totals recomputed from them`}
          </span>
        </span>
        <span className="cmp-badge">
          {counted.length} of {total}
        </span>
      </button>
      {open ? (
        <div className="cmp-picker-body" id="cmp-picker-body">
          <div className="cmp-picker-bar">
            {/* biome-ignore lint/a11y/useSemanticElements: a labelled group of commands, not a form fieldset */}
            <div
              role="group"
              aria-label="Select tests"
              className="cmp-segments"
            >
              <button type="button" onClick={() => onCount(ids(scenarios))}>
                All <span className="cmp-count">{total}</span>
              </button>
              <button
                type="button"
                onClick={() =>
                  onCount(
                    ids(
                      scenarios.filter((scenario) => {
                        const delta = scoreDelta(scenario)
                        return delta !== null && Math.abs(delta) > 1e-9
                      }),
                    ),
                  )
                }
              >
                Score changed
              </button>
              <button
                type="button"
                onClick={() =>
                  onCount(
                    ids(scenarios.filter((scenario) => !scenario.counted)),
                  )
                }
              >
                Invert
              </button>
              {exclusions.length > 0 ? (
                <button
                  type="button"
                  title="Take out every test with a gap the subject did not cause, on either side"
                  onClick={() => onCount(null)}
                >
                  Automatic
                </button>
              ) : null}
            </div>
            <p className="cmp-faint cmp-picker-note">
              Highlights, totals and every panel below are recomputed from the
              tests you keep. Zeros and incomplete tasks are results and stay
              in.
            </p>
          </div>
          <ul className="cmp-picks">
            {byScoreChange(scenarios).map((scenario) => {
              const score = metricOf(scenario, 'score')
              const gap = gapPhrase(scenario)
              const note = scenario.leftOut
                ? 'Left out by you'
                : scenario.exclusion
                  ? `${scenario.exclusion.applied ? 'Out by itself' : 'Brought back'} · ${gap}`
                  : null
              return (
                <li key={scenario.id} data-pick={scenario.id}>
                  <Checkbox
                    className="cmp-pick"
                    checked={scenario.counted}
                    onChange={() =>
                      onCount(
                        scenario.counted
                          ? ids(counted).filter((id) => id !== scenario.id)
                          : [...ids(counted), scenario.id],
                      )
                    }
                    label={
                      <>
                        <span className="cmp-pick-id">
                          <span className="cmp-mono">{scenario.id}</span>
                          {note ? (
                            <span className="cmp-pick-note">{note}</span>
                          ) : null}
                        </span>
                        <span className="cmp-pick-values">
                          {score ? valueText(score, 'baseline') : '—'} →{' '}
                          {score ? valueText(score, 'candidate') : '—'}
                        </span>
                        <span className="cmp-pick-delta">
                          {score ? deltaText(score) : ''}
                        </span>
                      </>
                    }
                  />
                </li>
              )
            })}
          </ul>
          <p className="cmp-note">
            <Info size={16} aria-hidden="true" />
            <span>
              A test missing on one side, technically invalid, undetermined,
              unscored or redefined leaves the totals by itself.{' '}
              {exclusions.length === 0
                ? 'None did here.'
                : `${plural(exclusions.length, 'test')} did here${out < exclusions.length ? `, ${exclusions.length - out} brought back` : ''}.`}
            </span>
          </p>
        </div>
      ) : null}
    </section>
  )
}

/* ---------------------------------------------------------- highlights */

const DIRECTION = {
  up: TrendingUp,
  down: TrendingDown,
  same: Equal,
} as const

function Highlights({
  comparison,
  onShow,
}: {
  comparison: ExecutionComparison
  onShow: (scenarioId: string) => void
}) {
  const { headline, detail, items } = comparisonHighlights(comparison)
  return (
    <section
      className="cmp-card cmp-highlights"
      aria-labelledby="cmp-highlights"
      data-comparison-highlights
    >
      <span id="cmp-highlights" className="cmp-eyebrow">
        Highlights · B compared with A
      </span>
      <p className="cmp-headline">{headline}</p>
      <p className="cmp-faint cmp-headline-detail">{detail}</p>
      {items.length > 0 ? (
        <ul className="cmp-highlight-list">
          {items.map((item) => {
            const Icon = DIRECTION[item.direction]
            const body = (
              <>
                <Icon
                  className="cmp-highlight-icon"
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
            return (
              <li key={`${item.test}:${item.text}`}>
                {item.test ? (
                  <button
                    type="button"
                    className="cmp-highlight"
                    onClick={() => item.test && onShow(item.test)}
                  >
                    {body}
                    <span className="cmp-show">
                      Show
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
    </section>
  )
}

/* -------------------------------------------------------------- totals */

const KPIS = ['score', 'completed', 'tokens', 'duration', 'function_calls']

function Totals({ comparison }: { comparison: ExecutionComparison }) {
  const [open, setOpen] = useState(false)
  const counted = comparison.scenarios.filter((scenario) => scenario.counted)
  const runs = (which: Which) =>
    counted.reduce((total, scenario) => total + scenario.sides[which].runs, 0)
  return (
    <section className="cmp-totals" aria-label="Totals">
      <div className="cmp-kpis">
        {KPIS.flatMap((id) => {
          const metric = comparison.totals.find((entry) => entry.id === id)
          if (!metric) return []
          const [a, b] =
            id === 'completed'
              ? [
                  `${metric.baseline ?? 0}/${runs('a')}`,
                  `${metric.candidate ?? 0}/${runs('b')}`,
                ]
              : [valueText(metric, 'baseline'), valueText(metric, 'candidate')]
          return [
            <div className="cmp-kpi" key={id} data-kpi={id}>
              <span className="cmp-kpi-label">
                {id === 'tokens' ? 'Tokens' : metric.label}
              </span>
              <span className="cmp-kpi-values">
                <span className="cmp-kpi-a">{a}</span>
                <span className="cmp-kpi-arrow" aria-hidden="true">
                  →
                </span>
                <span className="cmp-kpi-b">{b}</span>
              </span>
              <span className="cmp-kpi-delta">{deltaText(metric) || '—'}</span>
            </div>,
          ]
        })}
      </div>
      <button
        type="button"
        className="cmp-act cmp-all-toggle"
        aria-expanded={open}
        aria-controls="cmp-all-metrics"
        onClick={() => setOpen(!open)}
      >
        <ChevronRight
          size={16}
          aria-hidden="true"
          className={open ? 'cmp-chevron cmp-rot' : 'cmp-chevron'}
        />
        All metrics
      </button>
      {open ? (
        <div className="cmp-card cmp-all" id="cmp-all-metrics">
          <MetricTable
            caption="All metrics"
            metrics={comparison.totals.filter(
              (metric) => metric.baseline !== null || metric.candidate !== null,
            )}
          />
        </div>
      ) : null}
    </section>
  )
}

function MetricTable({
  caption,
  metrics,
}: {
  caption: string
  metrics: ComparedMetric[]
}) {
  return (
    <table className="cmp-metrics" data-comparison-metrics>
      <caption className="ep-sr">{caption}</caption>
      <thead>
        <tr>
          <th scope="col">
            <span className="ep-sr">Metric</span>
          </th>
          <th scope="col">A</th>
          <th scope="col">B</th>
          <th scope="col">Difference</th>
        </tr>
      </thead>
      <tbody>
        {metrics.map((metric) => (
          <tr key={metric.id} data-metric-id={metric.id}>
            <th scope="row">
              {metric.label}
              {outsideText(metric) ? (
                <span className="cmp-faint-num"> · {outsideText(metric)}</span>
              ) : null}
            </th>
            <td className="cmp-faint-num">{valueText(metric, 'baseline')}</td>
            <td>{valueText(metric, 'candidate')}</td>
            <td className="cmp-delta">{deltaText(metric) || '—'}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/* ------------------------------------------------------------- results */

const CELLS: Array<[string, string]> = [
  ['completed', 'Passed'],
  ['duration', 'Duration'],
  ['tokens', 'Tokens'],
  ['cost', 'Cost'],
  ['turns', 'Turns'],
]

function Pair({ a, b, delta }: { a: string; b: string; delta: string }) {
  return (
    <span className="cmp-pair">
      <span className="cmp-pair-values" title={`${a} → ${b}`}>
        <span className="cmp-faint-num">{a} → </span>
        {b}
      </span>
      <span className="cmp-pair-delta">{delta}</span>
    </span>
  )
}

/** Where each side's score sits on a 0–100 track. */
function ScoreTrack({ a, b }: { a: number | null; b: number | null }) {
  const at = (score: number) => `${Math.max(0, Math.min(100, score))}%`
  return (
    <span className="cmp-track" aria-hidden="true">
      <span className="cmp-track-rail" />
      {a !== null && b !== null ? (
        <span
          className="cmp-track-line"
          style={{
            left: at(Math.min(a, b)),
            width: `${Math.abs(Math.min(100, b) - Math.min(100, a))}%`,
          }}
        />
      ) : null}
      {a !== null ? (
        <span className="cmp-track-a" style={{ left: at(a) }} />
      ) : null}
      {b !== null ? (
        <span className="cmp-track-b" style={{ left: at(b) }} />
      ) : null}
    </span>
  )
}

function cellPair(scenario: ScenarioComparison, id: string) {
  const metric = metricOf(scenario, id)
  if (!metric || (metric.baseline === null && metric.candidate === null))
    return <span className="cmp-faint-num">—</span>
  if (id === 'completed') {
    const { a, b } = scenario.sides
    return (
      <Pair
        a={a.runs ? `${metric.baseline ?? 0}/${a.runs}` : '—'}
        b={b.runs ? `${metric.candidate ?? 0}/${b.runs}` : '—'}
        delta={deltaText(metric)}
      />
    )
  }
  return (
    <Pair
      a={valueText(metric, 'baseline')}
      b={valueText(metric, 'candidate')}
      delta={deltaText(metric)}
    />
  )
}

function Results({
  comparison,
  sides,
  bridge,
  open,
  onToggle,
  onRunTest,
}: {
  comparison: ExecutionComparison
  sides: Sides
  bridge: DashboardDataBridge | null
  open: Set<string>
  onToggle: (scenarioId: string) => void
  onRunTest?: (scenarioId: string) => void
}) {
  return (
    <section
      className="cmp-card cmp-results"
      aria-labelledby="cmp-results-title"
      data-comparison-scenarios
    >
      <div className="cmp-results-head">
        <div className="cmp-results-copy">
          <h2 id="cmp-results-title" className="cmp-h2">
            Results by test
          </h2>
          <p className="cmp-faint">
            Every test on either side, largest score change first. Open a test
            for its criteria, runs and screenshots.
          </p>
        </div>
        <div className="cmp-legend" aria-hidden="true">
          <span>
            <span className="cmp-track-a cmp-legend-dot" />A · Reference
          </span>
          <span>
            <span className="cmp-track-b cmp-legend-dot" />B · Compared
          </span>
        </div>
      </div>
      <table className="cmp-table">
        <caption className="ep-sr">Results by test, A then B</caption>
        <thead>
          <tr>
            <th scope="col">Test</th>
            <th scope="col">Score</th>
            {CELLS.map(([id, label]) => (
              <th key={id} scope="col" className="cmp-wide">
                {label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {byScoreChange(comparison.scenarios).map((scenario) => {
            const expanded = open.has(scenario.id)
            const score = metricOf(scenario, 'score')
            const delta = scoreDelta(scenario)
            const out = outLabel(scenario)
            const detailId = `cmp-detail-${scenario.id}`
            return (
              <Fragment key={scenario.id}>
                <tr
                  className="cmp-row"
                  data-scenario={scenario.id}
                  data-counted={scenario.counted}
                  onClick={(event: MouseEvent<HTMLTableRowElement>) => {
                    if (!isInteractiveTarget(event.target))
                      onToggle(scenario.id)
                  }}
                >
                  <th scope="row" className="cmp-test">
                    <button
                      type="button"
                      className="cmp-row-toggle"
                      aria-expanded={expanded}
                      aria-controls={detailId}
                      onClick={() => onToggle(scenario.id)}
                    >
                      <ChevronRight
                        size={16}
                        aria-hidden="true"
                        className={
                          expanded ? 'cmp-chevron cmp-rot' : 'cmp-chevron'
                        }
                      />
                      <span
                        className="cmp-dot"
                        data-moved={delta !== null && Math.abs(delta) > 1e-9}
                        aria-hidden="true"
                      />
                      <span className="cmp-test-id">{scenario.id}</span>
                    </button>
                    {out ? (
                      <span
                        className="cmp-pill"
                        title={exclusionPhrase(scenario) ?? undefined}
                      >
                        {out}
                      </span>
                    ) : null}
                  </th>
                  <td className="cmp-score">
                    <span className="cmp-score-cell">
                      <ScoreTrack
                        a={score?.baseline ?? null}
                        b={score?.candidate ?? null}
                      />
                      <Pair
                        a={
                          score?.baseline != null
                            ? valueText(score, 'baseline')
                            : stateText(scenario.sides.a.state)
                        }
                        b={
                          score?.candidate != null
                            ? valueText(score, 'candidate')
                            : stateText(scenario.sides.b.state)
                        }
                        delta={score ? deltaText(score) : ''}
                      />
                    </span>
                  </td>
                  {CELLS.map(([id]) => (
                    <td key={id} className="cmp-wide" data-cell={id}>
                      {cellPair(scenario, id)}
                    </td>
                  ))}
                </tr>
                <tr className="cmp-detail-row" id={detailId} hidden={!expanded}>
                  <td colSpan={2 + CELLS.length}>
                    {expanded ? (
                      <RowDetail
                        scenario={scenario}
                        sides={sides}
                        bridge={bridge}
                        onRunTest={onRunTest}
                      />
                    ) : null}
                  </td>
                </tr>
              </Fragment>
            )
          })}
        </tbody>
      </table>
    </section>
  )
}

const DETAIL_METRICS = [
  'score',
  'tokens',
  'cache_read',
  'cache_write',
  'cost',
  'duration',
  'turns',
  'function_calls',
  'function_errors',
  'technical_failures',
]

function rowSummary(scenario: ScenarioComparison): string {
  const out = exclusionPhrase(scenario)
  const reran = rerunPhrase(scenario)
  const delta = scoreDelta(scenario)
  const moved =
    delta === null
      ? 'No score to compare'
      : Math.abs(delta) < 1e-9
        ? 'Same score on both sides'
        : `B ${delta < 0 ? 'lost' : 'gained'} ${plural(Number(Math.abs(delta).toFixed(1)), 'point')}`
  return [
    out ? `Out of the totals: ${out}` : null,
    scenario.criteria.length > 0
      ? plural(
          scenario.criteria.length,
          'criterion changed',
          'criteria changed',
        )
      : null,
    moved,
    reran ? `${reran}, the last attempt compared` : null,
  ]
    .filter(Boolean)
    .join(' · ')
}

function points(value: number) {
  return String(Number(value.toFixed(1)))
}

export function RowDetail({
  scenario,
  sides,
  bridge,
  onRunTest,
}: {
  scenario: ScenarioComparison
  sides: Sides
  bridge: DashboardDataBridge | null
  onRunTest?: (scenarioId: string) => void
}) {
  const runs = useMemo(
    () =>
      (['a', 'b'] as const).flatMap((which) =>
        compareRuns(sides[which])
          .filter((run) => run.scenarioId === scenario.id)
          .map((run) => ({ which, run })),
      ),
    [sides, scenario.id],
  )
  const score = metricOf(scenario, 'score')
  const bothFull =
    score?.baseline === 100 && score?.candidate === 100
      ? 'No criterion changed. Both sides scored 100.'
      : 'No criterion changed between A and B.'
  return (
    <div className="cmp-detail" data-scenario-detail={scenario.id}>
      <div className="cmp-detail-bar">
        <span className="cmp-faint">{rowSummary(scenario)}</span>
        {onRunTest ? (
          <button
            type="button"
            className="cmp-act"
            onClick={() => onRunTest(scenario.id)}
          >
            <RotateCcw size={16} aria-hidden="true" />
            Run this test again
          </button>
        ) : null}
      </div>
      <div className="cmp-detail-grid">
        <div className="cmp-detail-col">
          <h3 className="cmp-h3">Criteria that changed</h3>
          {scenario.criteria.length === 0 ? (
            <p className="cmp-faint cmp-empty-line">{bothFull}</p>
          ) : null}
          {scenario.criteria.map((criterion) => (
            <div
              key={criterion.key}
              className="cmp-criterion"
              data-criterion={criterion.key}
            >
              <div className="cmp-criterion-head">
                <span
                  className="cmp-criterion-id"
                  title={
                    criterion.label === criterion.id
                      ? undefined
                      : criterion.label
                  }
                >
                  {criterion.id}
                </span>
                <span className="cmp-mono">
                  <span className="cmp-faint-num">
                    {points(criterion.a)}/{criterion.possible} →{' '}
                  </span>
                  {points(criterion.b)}/{criterion.possible}
                </span>
                <span className="cmp-tag">
                  {criterion.delta > 0 ? '+' : '−'}
                  {points(Math.abs(criterion.delta))}
                </span>
              </div>
              {criterion.label !== criterion.id ? (
                <p className="cmp-faint cmp-criterion-label">
                  {criterion.label}
                </p>
              ) : null}
              {(['a', 'b'] as const).flatMap((which) =>
                criterion.reasons[which].map((reason) => (
                  <div className="cmp-reason" key={`${which}:${reason}`}>
                    <span className="cmp-letter-sm">{which.toUpperCase()}</span>
                    <code>{reason}</code>
                  </div>
                )),
              )}
            </div>
          ))}
          {scenario.lostOnBoth.length > 0 ? (
            <>
              <h4 className="cmp-h4">Lost points on both sides</h4>
              <ul className="cmp-both">
                {scenario.lostOnBoth.map((criterion) => (
                  <li key={criterion.key} title={criterion.label}>
                    <span className="cmp-mono">{criterion.id}</span>
                    <span className="cmp-faint-num">
                      A {points(criterion.a)}/{criterion.possible} · B{' '}
                      {points(criterion.b)}/{criterion.possible}
                    </span>
                  </li>
                ))}
              </ul>
            </>
          ) : null}
        </div>
        <div className="cmp-detail-col">
          <h3 className="cmp-h3">Metrics</h3>
          <div className="cmp-inset">
            <MetricTable
              caption={`${scenario.id} metrics`}
              metrics={DETAIL_METRICS.flatMap((id) => {
                const metric = metricOf(scenario, id)
                return metric &&
                  (metric.baseline !== null || metric.candidate !== null)
                  ? [metric]
                  : []
              })}
            />
          </div>
          <h3 className="cmp-h3">Runs</h3>
          {runs.length === 0 ? (
            <p className="cmp-faint cmp-empty-line">No run on either side.</p>
          ) : (
            <ul className="cmp-runs">
              {runs.map(({ which, run }, index) => (
                <li
                  key={`${which}:${run.runId ?? index}`}
                  data-run-side={which}
                >
                  <span className="cmp-letter-sm">{which.toUpperCase()}</span>
                  <StatusLabel
                    state={runResultState({
                      status: run.status ?? 'unavailable',
                      completion: run.completion as
                        | 'completed'
                        | 'task_incomplete'
                        | 'undetermined'
                        | null,
                      score: run.score,
                    })}
                  />
                  <span className="cmp-run-meta">
                    {[
                      run.score === null ? null : `${points(run.score)}/100`,
                      run.wallTimeMs === null
                        ? null
                        : formatDuration(run.wallTimeMs),
                      run.subjectTokens === null
                        ? null
                        : `${formatTokens(run.subjectTokens)} tokens`,
                    ]
                      .filter(Boolean)
                      .join(' · ')}
                  </span>
                  {run.runId ? (
                    <>
                      <a
                        className="cmp-act"
                        href={hashForExecution(
                          sides[which].id,
                          null,
                          run.runId,
                          'transcript',
                        )}
                      >
                        Transcript
                      </a>
                      <a
                        className="cmp-act"
                        href={hashForExecution(
                          sides[which].id,
                          null,
                          run.runId,
                        )}
                      >
                        Evidence
                      </a>
                    </>
                  ) : null}
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
      <ScreenshotPairs bridge={bridge} sides={sides} scenarioId={scenario.id} />
    </div>
  )
}

/* --------------------------------------------------------- screenshots */

export type ScreenshotPair = {
  caption: string
  a: ScreenshotEntry | null
  b: ScreenshotEntry | null
}

/** A's and B's screenshots paired by caption, in the order they first
 *  appear; a caption on one side only pairs with nothing. */
export function pairByCaption(
  a: ScreenshotEntry[],
  b: ScreenshotEntry[],
): ScreenshotPair[] {
  const captions = [
    ...new Set([...a, ...b].map((screenshot) => screenshot.caption)),
  ]
  return captions.flatMap((caption) => {
    const left = a.filter((screenshot) => screenshot.caption === caption)
    const right = b.filter((screenshot) => screenshot.caption === caption)
    return Array.from(
      { length: Math.max(left.length, right.length) },
      (_, index) => ({
        caption,
        a: left[index] ?? null,
        b: right[index] ?? null,
      }),
    )
  })
}

/** One side of a pair: the image, which opens full size, and its record. */
export function ScreenshotFigure({
  which,
  screenshot,
  image,
  evidenceHref,
  onOpen,
  onSize,
}: {
  which: Which
  screenshot: ScreenshotEntry | null
  image: Image
  evidenceHref: string | null
  onOpen?: () => void
  onSize?: (size: string) => void
}) {
  const side = which.toUpperCase()
  return (
    <figure
      className="cmp-shot"
      data-screenshot={screenshot?.key}
      data-comparison-evidence={which}
    >
      {screenshot ? (
        <button
          type="button"
          className="cmp-shot-frame"
          aria-label={`Open ${side} · ${screenshot.caption} full size`}
          onClick={onOpen}
        >
          {image && 'source' in image ? (
            <img
              src={image.source}
              alt={`${side} · ${screenshot.caption}`}
              loading="lazy"
              onLoad={(event) =>
                onSize?.(
                  `${event.currentTarget.naturalWidth} × ${event.currentTarget.naturalHeight}`,
                )
              }
            />
          ) : (
            <span className="cmp-faint" role="status">
              {image ? image.error : 'Loading screenshot…'}
            </span>
          )}
        </button>
      ) : (
        <div className="cmp-shot-frame cmp-shot-none">
          <span className="cmp-faint">
            No screenshot with this caption in {side}
          </span>
        </div>
      )}
      <figcaption>
        <span className="cmp-letter-sm">{side}</span>
        {ROLE[which]}
        {evidenceHref ? <a href={evidenceHref}>Evidence record</a> : null}
      </figcaption>
    </figure>
  )
}

function ScreenshotPairs({
  bridge,
  sides,
  scenarioId,
}: {
  bridge: DashboardDataBridge | null
  sides: Sides
  scenarioId: string
}) {
  const shots = useMemo(
    () => ({
      a: screenshotsOf(sides.a, scenarioId),
      b: screenshotsOf(sides.b, scenarioId),
    }),
    [sides, scenarioId],
  )
  const imagesA = useScreenshotImages(bridge, shots.a)
  const imagesB = useScreenshotImages(bridge, shots.b)
  const images = { ...imagesA, ...imagesB }
  const [sizes, setSizes] = useState<Record<string, string>>({})
  const [viewer, setViewer] = useState<{ which: Which; index: number } | null>(
    null,
  )
  // Focus goes back to the screenshot the viewer opened from.
  const trigger = useRef<HTMLElement | null>(null)
  const pairs = pairByCaption(shots.a, shots.b)
  if (pairs.length === 0) return null
  const evidence = (which: Which, screenshot: ScreenshotEntry | null) =>
    screenshot
      ? hashForExecution(sides[which].id, null, screenshot.runId)
      : null
  const current = viewer ? shots[viewer.which][viewer.index] : null
  return (
    <section className="cmp-shots" aria-label="Screenshots">
      <div className="cmp-shots-head">
        <h3 className="cmp-h3">Screenshots</h3>
        <span className="cmp-faint">
          Paired by caption. Select one to open it full size.
        </span>
      </div>
      {pairs.map((pair, index) => {
        const key = `${pair.caption}:${index}`
        return (
          <div className="cmp-shot-pair" key={key} data-screenshot-pair>
            <div className="cmp-shot-title">
              <span>{pair.caption}</span>
              {sizes[key] ? (
                <span className="cmp-faint-num">{sizes[key]}</span>
              ) : null}
            </div>
            <div className="cmp-shot-grid">
              {(['a', 'b'] as const).map((which) => {
                const screenshot = pair[which]
                return (
                  <ScreenshotFigure
                    key={which}
                    which={which}
                    screenshot={screenshot}
                    image={screenshot ? images[screenshot.key] : undefined}
                    evidenceHref={evidence(which, screenshot)}
                    onOpen={() => {
                      if (!screenshot) return
                      trigger.current =
                        document.activeElement as HTMLElement | null
                      setViewer({
                        which,
                        index: shots[which].indexOf(screenshot),
                      })
                    }}
                    onSize={(size) =>
                      setSizes((known) =>
                        known[key] ? known : { ...known, [key]: size },
                      )
                    }
                  />
                )
              })}
            </div>
          </div>
        )
      })}
      {viewer && current ? (
        <ScreenshotViewer
          screenshots={shots[viewer.which]}
          images={images}
          index={viewer.index}
          onIndex={(index) => setViewer({ which: viewer.which, index })}
          onClose={() => {
            setViewer(null)
            trigger.current?.focus()
          }}
          evidenceHref={evidence(viewer.which, current) ?? undefined}
        />
      ) : null}
    </section>
  )
}

/* --------------------------------------------------------------- stack */

function StackDetail({ stack }: { stack: StackComparison }) {
  const unrecorded = (['a', 'b'] as const).filter(
    (which) => !stack.recorded[which],
  )
  if (unrecorded.length > 0)
    return (
      <p className="cmp-faint cmp-empty-line">
        No stack recorded for{' '}
        {unrecorded.map((which) => which.toUpperCase()).join(' and ')}: the
        workers cannot be compared.
      </p>
    )
  const onlyHere = (which: Which) => [
    ...(which === 'a' ? stack.onlyA : stack.onlyB),
    ...stack.yourCode
      .filter((group) => group.side === which)
      .flatMap((group) => group.onlyHere),
  ]
  return (
    <div className="cmp-stack" data-comparison-stack>
      {stack.changed.length > 0 ? (
        <table className="cmp-metrics cmp-stack-table">
          <caption className="ep-sr">
            Workers that ran a different build
          </caption>
          <thead>
            <tr>
              <th scope="col">Worker</th>
              <th scope="col">A</th>
              <th scope="col">B</th>
            </tr>
          </thead>
          <tbody>
            {stack.changed.map((change) => (
              <tr key={change.field} data-stack-worker={change.field}>
                <th scope="row">{change.field}</th>
                <td className="cmp-faint-num">{change.a}</td>
                <td>{change.b}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p className="cmp-faint cmp-empty-line">
          Every worker on both sides ran the same build.
        </p>
      )}
      <dl className="cmp-stack-facts">
        {(['a', 'b'] as const).map((which) =>
          onlyHere(which).length > 0 ? (
            <div key={which} data-stack-only={which}>
              <dt>Only in {which.toUpperCase()}</dt>
              <dd>{onlyHere(which).sort().join(', ')}</dd>
            </div>
          ) : null,
        )}
        {stack.same.length > 0 ? (
          <div>
            <dt>The same on both sides</dt>
            <dd>{stack.same.join(', ')}</dd>
          </div>
        ) : null}
      </dl>
    </div>
  )
}

function Methodology({ comparison }: { comparison: ExecutionComparison }) {
  const reran = comparison.scenarios.flatMap((scenario) => {
    const phrase = rerunPhrase(scenario)
    return phrase ? [`${scenario.id}: ${phrase}`] : []
  })
  const { runner } = comparison
  return (
    <div className="cmp-method">
      <p>
        A is the reference only because it was chosen first; every difference is
        B minus A, an observation that ranks neither side. Tests pair by
        scenario, case seed and repetition, as Release Control pairs them.
      </p>
      <p>
        A score is the mean of a side’s scored runs, given only when every
        planned run was observed. Tokens, time, turns and calls sum the runs
        that reported them. A side short of runs is shown as partial and no
        difference is taken from it.
      </p>
      <dl className="cmp-stack-facts">
        <div>
          <dt>Runner</dt>
          <dd>
            A {runner.a ?? 'not recorded'} · B {runner.b ?? 'not recorded'}
          </dd>
        </div>
        {runner.definitionsChanged.length > 0 ? (
          <div>
            <dt>Definitions changed</dt>
            <dd>{runner.definitionsChanged.join(', ')}</dd>
          </div>
        ) : null}
        {reran.length > 0 ? (
          <div>
            <dt>Run again, last attempt compared</dt>
            <dd>{reran.join('; ')}</dd>
          </div>
        ) : null}
      </dl>
    </div>
  )
}

/* ---------------------------------------------------------------- view */

/** The comparison itself, from two loaded executions. Every difference is
 *  an observation: no side is labelled better or worse. */
export function ComparisonView({
  comparison,
  sides,
  bridge = null,
  swap,
  refreshError = null,
  onCount,
  onRunTest,
}: {
  comparison: ExecutionComparison
  sides: Sides
  bridge?: DashboardDataBridge | null
  /** Why the last refresh failed; what was loaded stays on screen. */
  refreshError?: string | null
  /** The link to the same comparison with A and B swapped. */
  swap: string
  /** Count exactly these tests; null goes back to the automatic rule. */
  onCount: (ids: string[] | null) => void
  /** Run one test again with B's parameters. */
  onRunTest?: (scenarioId: string) => void
}) {
  const [open, setOpen] = useState<Set<string>>(new Set())
  const [stackOpen, setStackOpen] = useState(false)
  const warning = runnerWarning(comparison.runner)
  const stack = stackChanges(comparison.stack)
  const same = sameSentence(comparison)
  const toggle = (id: string) =>
    setOpen((current) => {
      const next = new Set(current)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  const show = (id: string) => {
    setOpen((current) => new Set(current).add(id))
    window.setTimeout(
      () =>
        document
          .querySelector(`[data-scenario="${CSS.escape(id)}"]`)
          ?.scrollIntoView({ block: 'start', behavior: 'smooth' }),
      0,
    )
  }
  const live = (['a', 'b'] as const).filter((which) =>
    LIVE.includes(String(sides[which].status ?? '')),
  )
  const changes: ReactNode[] = [
    ...comparison.parameters.map((change) => (
      <FactChip
        key={change.field}
        label={change.field}
        value={`${change.a} → ${change.b}`}
        data-change={change.field}
      />
    )),
    ...(comparison.runner.differs
      ? [
          <FactChip
            key="runner"
            label="runner"
            value={`${comparison.runner.a} → ${comparison.runner.b}`}
            data-change="runner"
          />,
        ]
      : []),
    ...(stack
      ? [
          <FactChip
            key="stack"
            label="stack"
            value={stack}
            data-change="stack"
          />,
        ]
      : []),
  ]
  return (
    <>
      {refreshError ? (
        <p className="cmp-warning" role="status" data-comparison-refresh-error>
          <AlertTriangle size={16} aria-hidden="true" />
          <span>
            Refresh failed. Showing the comparison as last loaded; updates will
            retry. {refreshError}
          </span>
        </p>
      ) : null}
      <section className="cmp-sides" aria-label="Executions compared">
        <SideCard which="a" side={comparison.a} detail={sides.a} />
        <a
          className="cmp-swap"
          href={swap}
          aria-label="Swap A and B"
          title="Swap A and B"
        >
          <ArrowLeftRight size={16} aria-hidden="true" />
        </a>
        <SideCard which="b" side={comparison.b} detail={sides.b} />
      </section>
      {live.length > 0 ? (
        <p className="cmp-live" role="status" data-comparison-live>
          <StatusLabel state="running" label="Running" />
          <span>
            {live.map((which) => which.toUpperCase()).join(' and ')}{' '}
            {live.length === 1 ? 'is' : 'are'} still running: figures short of
            their planned runs are partial, and this page follows them.
          </span>
        </p>
      ) : null}

      <section
        className="cmp-changes"
        aria-label="What changed"
        data-comparison-changes
      >
        <div className="cmp-changes-row">
          <span className="cmp-eyebrow">What changed</span>
          {changes.length > 0 ? <FactList>{changes}</FactList> : null}
          <span className="cmp-faint">
            {changes.length === 0 ? `Nothing recorded differs. ${same}` : same}
          </span>
          <button
            type="button"
            className="cmp-act"
            aria-controls="comparison-stack"
            onClick={() => {
              setStackOpen(true)
              window.setTimeout(
                () =>
                  document
                    .getElementById('comparison-stack')
                    ?.scrollIntoView({ block: 'start', behavior: 'smooth' }),
                0,
              )
            }}
          >
            Stack details
          </button>
        </div>
        {warning ? (
          <p className="cmp-warning" data-runner-warning>
            <AlertTriangle size={16} aria-hidden="true" />
            <span>{warning}</span>
          </p>
        ) : null}
      </section>

      <TestPicker comparison={comparison} onCount={onCount} />
      <Highlights comparison={comparison} onShow={show} />
      <Totals comparison={comparison} />
      <Results
        comparison={comparison}
        sides={sides}
        bridge={bridge}
        open={open}
        onToggle={toggle}
        onRunTest={onRunTest}
      />

      <section className="cmp-more" aria-label="More about this comparison">
        <DisclosureLayer
          id="comparison-stack"
          label="Stack"
          scent={[
            stack ?? 'same stack',
            comparison.stack.same.length > 0
              ? `${comparison.stack.same.length} the same`
              : null,
          ]
            .filter(Boolean)
            .join(' · ')}
          open={stackOpen}
          onToggle={setStackOpen}
        >
          <StackDetail stack={comparison.stack} />
        </DisclosureLayer>
        <DisclosureLayer
          id="comparison-method"
          label="Test contracts and methodology"
          scent="Means per side, a test run again by its last attempt, changed definitions flagged"
          open={false}
        >
          <Methodology comparison={comparison} />
        </DisclosureLayer>
      </section>
    </>
  )
}
