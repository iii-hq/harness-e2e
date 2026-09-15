import { Tabs, TabsList, TabsTrigger } from '@iii-dev/console-ui'
import { ArrowDown, ArrowUp } from 'lucide-react'
import { type ReactNode, useId, useMemo, useState } from 'react'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import {
  Button,
  DataTable,
  deltaDirection,
  deltaTone,
  EmptyState,
} from '@/design-system'
import { definitionTitle, shortDefinition } from '@/lib/definition-digest'
import { formatDuration } from '@/lib/execution-view'
import {
  comparePrimaryMetrics,
  type MetricId,
  type MetricValue,
  metricDelta,
  type PrimaryMetrics,
} from '@/lib/primary-metrics'
import './PrimaryMetricsView.css'

export const primaryMetricLabels: Record<MetricId, string> = {
  score: 'Score',
  totalTokens: 'Total tokens',
  inputTokens: 'Input tokens',
  outputTokens: 'Output tokens',
  cacheRead: 'Cache read',
  cacheWrite: 'Cache written',
  turns: 'Turns',
  functionCalls: 'Function calls',
  functionErrors: 'Function errors',
  durationMs: 'Accumulated time',
  costUsd: 'Spend',
}
export const PRIMARY_SUMMARY_METRICS = [
  'score',
  'costUsd',
  'durationMs',
  'totalTokens',
  'turns',
  'functionCalls',
] as const
const groups = {
  Overview: ['score', 'totalTokens', 'durationMs', 'costUsd'],
  Tokens: [
    'totalTokens',
    'inputTokens',
    'cacheRead',
    'cacheWrite',
    'outputTokens',
  ],
  Activity: ['score', 'turns', 'functionCalls', 'functionErrors', 'durationMs'],
} satisfies Record<string, MetricId[]>

export function formatPrimaryMetric(
  id: MetricId,
  value: number | null,
): string {
  if (value === null) return '—'
  if (id === 'durationMs') return formatDuration(value / 1000)
  if (id === 'costUsd')
    return value > 0 && value < 0.0001 ? '<$0.0001' : `$${value.toFixed(4)}`
  return value.toLocaleString('en-US', { maximumFractionDigits: 2 })
}

export function primaryMetricChange(
  id: MetricId,
  baseline?: MetricValue,
  candidate?: MetricValue,
) {
  const delta = metricDelta(baseline, candidate)
  const reference = baseline?.value ?? baseline?.observed
  const percentage =
    delta === 0
      ? 0
      : delta !== null && reference != null && reference > 0
        ? (delta / reference) * 100
        : null
  const label =
    delta === null
      ? 'Not comparable'
      : delta === 0
        ? '0%'
        : percentage === null
          ? `${delta > 0 ? '+' : '−'}${formatPrimaryMetric(id, Math.abs(delta))}${id === 'score' ? ' pts' : ''} · A is zero`
          : `${delta > 0 ? '+' : '−'}${Math.abs(percentage) < 0.01 ? '<0.01' : Math.abs(percentage).toLocaleString('en-US', { maximumFractionDigits: 2 })}%`
  return { delta, percentage, label }
}

function MetricNumber({ id, metric }: { id: MetricId; metric?: MetricValue }) {
  const value = metric?.value ?? metric?.observed ?? null
  return (
    <span className="pm-number">
      {value === null ? (
        <span role="img" aria-label="Not reported" title="Not reported">
          —
        </span>
      ) : (
        <span>{formatPrimaryMetric(id, value)}</span>
      )}
      {metric && metric.value === null && metric.observed !== null ? (
        <small className="pm-partial">
          {id === 'score'
            ? 'Partial mean'
            : `Observed subtotal · ${
                Number.isFinite(metric.expected)
                  ? `${metric.samples}/${metric.expected} samples`
                  : `${metric.samples} reported samples · planned count unknown`
              }`}
        </small>
      ) : null}
    </span>
  )
}

function Readout({
  id,
  baseline,
  candidate,
  comparing,
  compact = false,
}: {
  id: MetricId
  baseline?: MetricValue
  candidate?: MetricValue
  comparing: boolean
  compact?: boolean
}) {
  if (!comparing) return <MetricNumber id={id} metric={baseline} />
  const { delta, percentage, label } = primaryMetricChange(
    id,
    baseline,
    candidate,
  )
  return (
    <div
      className={`pm-pair${compact ? ' pm-compact' : ''}`}
      data-change={
        delta === null ? 'unavailable' : delta === 0 ? 'unchanged' : 'changed'
      }
      data-tone={deltaTone(
        deltaDirection(delta),
        id === 'score'
          ? 'higher'
          : id === 'costUsd' || id === 'durationMs' || id === 'functionErrors'
            ? 'lower'
            : 'neither',
      )}
    >
      <div className="pm-side">
        <span className="pm-side-label" title="A · Reference">
          {compact ? 'A' : 'A · Reference'}
        </span>
        <MetricNumber id={id} metric={baseline} />
      </div>
      <div className="pm-side">
        <span className="pm-side-label" title="B · Compared">
          {compact ? 'B' : 'B · Compared'}
        </span>
        <MetricNumber id={id} metric={candidate} />
      </div>
      <div className="pm-side pm-difference">
        <span className="pm-side-label" title="Change vs A">
          {compact ? 'Δ' : 'Change vs A'}
        </span>
        <span className="pm-delta">
          {delta !== null && delta !== 0 ? (
            delta > 0 ? (
              <ArrowUp size={16} aria-hidden="true" />
            ) : (
              <ArrowDown size={16} aria-hidden="true" />
            )
          ) : null}
          {delta === null ? (
            <span
              role="img"
              aria-label="Delta unavailable"
              title="This metric is not reported for A or B."
            >
              Not comparable
            </span>
          ) : delta === 0 ? (
            <span>{compact ? '0%' : '0% · No change'}</span>
          ) : percentage === null ? (
            <span title="Percentage change cannot be calculated from a zero reference.">
              {label}
            </span>
          ) : (
            <span>{`${label}${compact ? '' : ` · ${delta > 0 ? 'increase' : 'decrease'}`}`}</span>
          )}
        </span>
      </div>
    </div>
  )
}

export function PrimaryMetricsView({
  baseline,
  candidate,
  baselineLabel = 'Baseline',
  candidateLabel = 'Candidate',
  baselineExecutionId,
  candidateExecutionId,
  summaryOnly = false,
  showTests = false,
  excludeEmptyTests,
  onExcludeEmptyTestsChange,
  toolbarActions,
}: {
  baseline: PrimaryMetrics
  candidate?: PrimaryMetrics
  baselineLabel?: string
  candidateLabel?: string
  baselineExecutionId?: string
  candidateExecutionId?: string
  summaryOnly?: boolean
  showTests?: boolean
  excludeEmptyTests?: boolean
  onExcludeEmptyTestsChange?: (exclude: boolean) => void
  toolbarActions?: ReactNode
}) {
  const [localExclude, setLocalExclude] = useState(false)
  const exclude = excludeEmptyTests ?? localExclude
  const setExclude = onExcludeEmptyTestsChange ?? setLocalExclude
  const [view, setView] = useState('grouped')
  const [group, setGroup] = useState<keyof typeof groups>('Overview')
  const tableId = useId()
  const comparison = useMemo(
    () =>
      candidate ? comparePrimaryMetrics(baseline, candidate, exclude) : null,
    [baseline, candidate, exclude],
  )
  const a = comparison?.baseline ?? baseline
  const b = comparison?.candidate
  const comparing = Boolean(candidate)
  const tests = useMemo(() => {
    if (!comparison)
      return baseline.tests.map((test) => ({
        key: test.key,
        label: test.label,
        definition: test.definition,
        baseline: test,
        candidate: null,
      }))
    return comparison.tests
  }, [baseline, comparison])
  const excluded =
    comparison && candidate && comparison.excluded > 0
      ? comparePrimaryMetrics(baseline, candidate, false).tests.filter(
          (test) => !tests.some((included) => included.key === test.key),
        )
      : []
  const readout = (id: MetricId, compact = false) => (
    <Readout
      id={id}
      baseline={a.metrics[id]}
      candidate={b?.metrics[id]}
      comparing={comparing}
      compact={compact}
    />
  )

  return (
    <section
      className={`primary-metrics${summaryOnly ? ' pm-consolidated' : ''}${comparing ? ' pm-comparing' : ''}`}
      aria-label={
        comparing ? 'Execution comparison metrics' : 'Execution metrics'
      }
    >
      <div className="pm-toolbar">
        <div className="pm-context">
          <div className="pm-identities">
            {comparing ? (
              <>
                <span>
                  <b>A</b> {baselineLabel}
                </span>
                <span>
                  <b>B</b> {candidateLabel}
                </span>
                <span className="pm-muted">
                  Percentage change relative to A
                </span>
              </>
            ) : (
              <span className="pm-muted">Execution summary</span>
            )}
          </div>
          <span className="pm-scope" aria-live="polite">
            {comparing
              ? `${tests.length} of ${comparison?.totalTests} tests`
              : `${tests.length} ${tests.length === 1 ? 'test' : 'tests'}`}
          </span>
        </div>
        {toolbarActions}
        {!summaryOnly ? (
          <Tabs value={view} onValueChange={setView}>
            <TabsList aria-label="Metrics view" className="pm-view-tabs">
              <TabsTrigger
                value="grouped"
                id={`${tableId}-grouped-tab`}
                aria-controls={`${tableId}-view`}
                icon={false}
              >
                Grouped
              </TabsTrigger>
              <TabsTrigger
                value="by-test"
                id={`${tableId}-by-test-tab`}
                aria-controls={`${tableId}-view`}
                icon={false}
              >
                By test
              </TabsTrigger>
            </TabsList>
          </Tabs>
        ) : null}
      </div>
      {comparing ? (
        <div className="pm-filter-row">
          <label className="pm-filter">
            <input
              type="checkbox"
              checked={exclude}
              onChange={(event) => setExclude(event.target.checked)}
            />
            Hide tests with zero score or no result
          </label>
          {exclude && excluded.length > 0 ? (
            <span className="pm-muted">
              Same {tests.length} tests on both sides
            </span>
          ) : null}
        </div>
      ) : null}

      <div
        id={`${tableId}-view`}
        data-metrics-body
        {...(summaryOnly
          ? {}
          : {
              role: 'tabpanel',
              'aria-labelledby': `${tableId}-${view}-tab`,
              tabIndex: 0,
            })}
      >
        {tests.length === 0 ? (
          <EmptyState
            title={
              exclude
                ? 'No tests remain in this comparison'
                : summaryOnly
                  ? 'No matching tests'
                  : 'No test results yet'
            }
            description={
              exclude
                ? 'Every test has a zero score or a missing result on at least one side.'
                : summaryOnly
                  ? 'Show all tests or wait for results to become available.'
                  : 'Metrics will appear as test results become available.'
            }
            actions={
              exclude ? (
                <Button onClick={() => setExclude(false)}>
                  Show all tests
                </Button>
              ) : undefined
            }
          />
        ) : (
          <>
            {summaryOnly || view === 'grouped' ? (
              <>
                <div className="pm-headlines">
                  {(summaryOnly
                    ? PRIMARY_SUMMARY_METRICS
                    : (['score', 'costUsd', 'durationMs'] as const)
                  ).map((id) => (
                    <div className="pm-headline" key={id}>
                      <h3>
                        {primaryMetricLabels[id]}
                        {id === 'score' ? (
                          <span className="pm-unit"> / 100</span>
                        ) : id === 'costUsd' ? (
                          <span className="pm-unit"> USD</span>
                        ) : null}
                      </h3>
                      {readout(id)}
                      {id === 'score' ? (
                        <div
                          className="pm-test-coverage"
                          title="Counts tests with at least one recorded run or numeric score. A zero score counts as scored; repetitions are not counted as separate tests."
                        >
                          {(b ? [a, b] : [a]).map((metrics, index) => (
                            <p
                              className="pm-muted"
                              key={index === 0 ? 'a' : 'b'}
                              data-test-coverage={index === 0 ? 'a' : 'b'}
                            >
                              {comparing ? (
                                <strong>{index === 0 ? 'A' : 'B'} · </strong>
                              ) : null}
                              Tests executed:{' '}
                              {
                                metrics.tests.filter(
                                  (test) => test.executedRuns > 0,
                                ).length
                              }{' '}
                              · Scored:{' '}
                              {
                                metrics.tests.filter(
                                  (test) => test.metrics.score.samples > 0,
                                ).length
                              }
                            </p>
                          ))}
                        </div>
                      ) : null}
                      <p className="pm-muted">
                        {id === 'score'
                          ? 'Equal weight per test'
                          : id === 'costUsd'
                            ? 'Recorded execution spend'
                            : id === 'durationMs'
                              ? 'Run durations, including retries'
                              : id === 'totalTokens'
                                ? 'Input + output, including retries'
                                : id === 'turns'
                                  ? 'Across all test runs'
                                  : 'Recorded function calls'}
                      </p>
                    </div>
                  ))}
                </div>
                <div className="pm-bands">
                  <section className="pm-band" aria-label="Token metrics">
                    <div className="pm-band-heading">
                      <h3>Tokens</h3>
                      <span className="pm-muted">Input and output</span>
                    </div>
                    {(
                      [
                        'totalTokens',
                        'inputTokens',
                        'outputTokens',
                        'cacheRead',
                        'cacheWrite',
                      ] as const
                    )
                      .filter((id) => !summaryOnly || id !== 'totalTokens')
                      .map((id) => (
                        <div
                          className={`pm-metric ${id === 'totalTokens' || id === 'inputTokens' || id === 'outputTokens' ? '' : 'pm-breakdown'}`}
                          key={id}
                        >
                          <span>{primaryMetricLabels[id]}</span>
                          {readout(id, true)}
                        </div>
                      ))}
                    {!summaryOnly ? (
                      <p className="pm-token-note">
                        Total tokens are reported input plus output. Cache usage
                        is shown separately.
                      </p>
                    ) : null}
                  </section>
                  <section className="pm-band" aria-label="Activity metrics">
                    <div className="pm-band-heading">
                      <h3>Activity</h3>
                      <span className="pm-muted">Across test runs</span>
                    </div>
                    {(['turns', 'functionCalls', 'functionErrors'] as const)
                      .filter((id) => !summaryOnly || id === 'functionErrors')
                      .map((id) => (
                        <div className="pm-metric" key={id}>
                          <span>{primaryMetricLabels[id]}</span>
                          {readout(id, true)}
                        </div>
                      ))}
                  </section>
                </div>
                <div className="pm-availability">
                  <span>— Not reported</span>
                  <span>Partial values include only reported samples.</span>
                  {summaryOnly ? (
                    <span>
                      Cache usage is reported separately from input + output.
                    </span>
                  ) : null}
                  {comparing ? (
                    <span>
                      Changes use the displayed values. Both sides must report
                      the metric.
                    </span>
                  ) : null}
                </div>
              </>
            ) : null}
            {showTests || (!summaryOnly && view === 'by-test') ? (
              <section className="pm-tests" aria-label="Metrics by test">
                <div className="pm-tests-heading">
                  <h3>Test results</h3>
                  <span className="pm-muted">
                    {tests.length} {tests.length === 1 ? 'test' : 'tests'}
                  </span>
                </div>
                <Tabs
                  value={group}
                  onValueChange={(value) =>
                    setGroup(value as keyof typeof groups)
                  }
                >
                  <TabsList aria-label="Test metric groups">
                    {(Object.keys(groups) as Array<keyof typeof groups>).map(
                      (name) => (
                        <TabsTrigger
                          value={name}
                          icon={false}
                          key={name}
                          id={`${tableId}-${name}`}
                          aria-controls={tableId}
                        >
                          {name}
                        </TabsTrigger>
                      ),
                    )}
                  </TabsList>
                </Tabs>
                <div
                  id={tableId}
                  role="tabpanel"
                  aria-labelledby={`${tableId}-${group}`}
                >
                  <DataTable
                    caption={`${group} metrics by test${comparing ? ', baseline A and candidate B' : ''}`}
                    className="pm-table"
                  >
                    <thead>
                      <tr>
                        <th scope="col">Test</th>
                        {groups[group].map((id) => (
                          <th scope="col" key={id}>
                            {primaryMetricLabels[id]}
                            {id === 'costUsd' ? ' (USD)' : ''}
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody>
                      {tests.map((test) => (
                        <tr key={test.key}>
                          <th scope="row" className="pm-test-identity">
                            <span>{test.label}</span>
                            {test.definition !== null ? (
                              <small title={definitionTitle(test.definition)}>
                                Definition {shortDefinition(test.definition)}
                              </small>
                            ) : null}
                            <div className="pm-test-actions">
                              {baselineExecutionId && test.baseline ? (
                                <ScenarioChatAction
                                  executionId={baselineExecutionId}
                                  scenarioId={test.label}
                                  behaviorSha256={test.baseline.definition}
                                  label={
                                    comparing ? 'Transcript A' : 'Transcript'
                                  }
                                />
                              ) : null}
                              {candidateExecutionId && test.candidate ? (
                                <ScenarioChatAction
                                  executionId={candidateExecutionId}
                                  scenarioId={test.label}
                                  behaviorSha256={test.candidate.definition}
                                  label="Transcript B"
                                />
                              ) : null}
                            </div>
                            {comparing &&
                            (!test.baseline || !test.candidate) ? (
                              <small>
                                No result in {!test.baseline ? 'A' : 'B'}
                              </small>
                            ) : null}
                          </th>
                          {groups[group].map((id) => (
                            <td key={id} data-label={primaryMetricLabels[id]}>
                              <Readout
                                id={id}
                                baseline={test.baseline?.metrics[id]}
                                candidate={test.candidate?.metrics[id]}
                                comparing={comparing}
                                compact
                              />
                            </td>
                          ))}
                        </tr>
                      ))}
                    </tbody>
                  </DataTable>
                </div>
              </section>
            ) : null}
          </>
        )}
      </div>
      {excluded.length > 0 ? (
        <details className="pm-excluded">
          <summary>
            {excluded.length} {excluded.length === 1 ? 'test' : 'tests'}{' '}
            excluded from both sides
          </summary>
          <ul>
            {excluded.map((test) => (
              <li key={test.key}>
                <span>{test.label}</span>
                <span className="pm-muted">
                  {!test.baseline
                    ? 'No result in A'
                    : !test.candidate
                      ? 'No result in B'
                      : (test.baseline.metrics.score.value ??
                            test.baseline.metrics.score.observed) === 0
                        ? 'Zero score in A'
                        : (test.candidate.metrics.score.value ??
                              test.candidate.metrics.score.observed) === 0
                          ? 'Zero score in B'
                          : 'Score unavailable'}
                </span>
              </li>
            ))}
          </ul>
        </details>
      ) : null}
    </section>
  )
}
