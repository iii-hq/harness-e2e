import { Tabs, TabsList, TabsTrigger } from '@iii-dev/console-ui'
import { ArrowDown, ArrowUp } from 'lucide-react'
import { useId, useMemo, useState } from 'react'
import { ScenarioChatAction } from '@/components/ScenarioChatAction'
import { Button, DataTable, EmptyState } from '@/design-system'
import { formatDuration } from '@/lib/execution-view'
import {
  comparePrimaryMetrics,
  type MetricId,
  type MetricValue,
  type PrimaryMetrics,
} from '@/lib/primary-metrics'
import './PrimaryMetricsView.css'

const labels: Record<MetricId, string> = {
  score: 'Score',
  totalTokens: 'Total tokens',
  inputTokens: 'Input tokens',
  outputTokens: 'Output tokens',
  inputNormal: 'Normal input',
  cacheRead: 'Cache read',
  cacheWrite: 'Cache written',
  turns: 'Turns',
  functionCalls: 'Function calls',
  functionErrors: 'Function errors',
  durationMs: 'Accumulated time',
  costUsd: 'Spend',
}
const groups = {
  Overview: ['score', 'totalTokens', 'durationMs', 'costUsd'],
  Tokens: [
    'totalTokens',
    'inputTokens',
    'inputNormal',
    'cacheRead',
    'cacheWrite',
    'outputTokens',
  ],
  Activity: ['score', 'turns', 'functionCalls', 'functionErrors', 'durationMs'],
} satisfies Record<string, MetricId[]>

function format(id: MetricId, value: number | null): string {
  if (value === null) return '—'
  if (id === 'durationMs') return formatDuration(value / 1000)
  if (id === 'costUsd')
    return value > 0 && value < 0.0001 ? '<$0.0001' : `$${value.toFixed(4)}`
  return value.toLocaleString('en-US', { maximumFractionDigits: 2 })
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
        <span>{format(id, value)}</span>
      )}
      {metric && metric.value === null && metric.observed !== null ? (
        <small className="pm-partial">
          {id === 'score' ? 'Partial mean' : 'Observed subtotal'} ·{' '}
          {Number.isFinite(metric.expected)
            ? `${metric.samples}/${metric.expected} samples`
            : `${metric.samples} reported samples · planned count unknown`}
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
  delta = null,
}: {
  id: MetricId
  baseline?: MetricValue
  candidate?: MetricValue
  comparing: boolean
  delta?: number | null
}) {
  if (!comparing) return <MetricNumber id={id} metric={baseline} />
  return (
    <div className="pm-pair">
      <div className="pm-side">
        <span className="pm-side-label">A</span>
        <MetricNumber id={id} metric={baseline} />
      </div>
      <div className="pm-side">
        <span className="pm-side-label">B</span>
        <MetricNumber id={id} metric={candidate} />
      </div>
      <div className="pm-side pm-difference">
        <span className="pm-side-label">Δ</span>
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
              title="Deltas require matching test cases and repetitions with complete metrics."
            >
              —
            </span>
          ) : (
            <span>{`${delta > 0 ? '+' : delta < 0 ? '−' : ''}${format(id, Math.abs(delta))}${id === 'score' ? ' pts' : ''}`}</span>
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
}: {
  baseline: PrimaryMetrics
  candidate?: PrimaryMetrics
  baselineLabel?: string
  candidateLabel?: string
  baselineExecutionId?: string
  candidateExecutionId?: string
}) {
  const [exclude, setExclude] = useState(false)
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
        version: test.version,
        baseline: test,
        candidate: null,
        deltas: null,
      }))
    return comparison.tests.map((test) => ({
      ...test,
      deltas:
        test.baseline && test.candidate
          ? comparePrimaryMetrics(
              { tests: [test.baseline], metrics: test.baseline.metrics },
              { tests: [test.candidate], metrics: test.candidate.metrics },
              false,
            ).deltas
          : null,
    }))
  }, [baseline, comparison])
  const excluded =
    comparison && candidate && comparison.excluded > 0
      ? comparePrimaryMetrics(baseline, candidate, false).tests.filter(
          (test) => !tests.some((included) => included.key === test.key),
        )
      : []
  const readout = (id: MetricId) => (
    <Readout
      id={id}
      baseline={a.metrics[id]}
      candidate={b?.metrics[id]}
      comparing={comparing}
      delta={comparison?.deltas[id]}
    />
  )

  return (
    <section
      className="primary-metrics"
      aria-label={
        comparing ? 'Execution comparison metrics' : 'Execution metrics'
      }
    >
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
              <span className="pm-muted">Deltas are B − A</span>
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
      <div
        id={`${tableId}-view`}
        role="tabpanel"
        aria-labelledby={`${tableId}-${view}-tab`}
        // biome-ignore lint/a11y/noNoninteractiveTabindex: The active tab panel must be reachable from its tab.
        tabIndex={0}
      >
        {tests.length === 0 ? (
          <EmptyState
            title={
              exclude
                ? 'No tests remain in this comparison'
                : 'No test results yet'
            }
            description={
              exclude
                ? 'Every test has a zero score or a missing result on at least one side.'
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
        ) : view === 'grouped' ? (
          <>
            <div className="pm-headlines">
              {(['score', 'costUsd', 'durationMs'] as const).map((id) => (
                <div className="pm-headline" key={id}>
                  <h3>
                    {labels[id]}
                    {id === 'score' ? (
                      <span className="pm-unit"> / 100</span>
                    ) : id === 'costUsd' ? (
                      <span className="pm-unit"> USD</span>
                    ) : null}
                  </h3>
                  {readout(id)}
                  <p className="pm-muted">
                    {id === 'score'
                      ? 'Equal weight per test'
                      : id === 'costUsd'
                        ? 'Recorded execution spend'
                        : 'Run durations, including retries'}
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
                    'inputNormal',
                    'cacheRead',
                    'cacheWrite',
                  ] as const
                ).map((id) => (
                  <div
                    className={`pm-metric ${id === 'totalTokens' || id === 'inputTokens' || id === 'outputTokens' ? '' : 'pm-breakdown'}`}
                    key={id}
                  >
                    <span>{labels[id]}</span>
                    {readout(id)}
                  </div>
                ))}
                <p className="pm-token-note">
                  Input tokens include normal input, cache read and cache
                  written; all three must be reported.
                </p>
              </section>
              <section className="pm-band" aria-label="Activity metrics">
                <div className="pm-band-heading">
                  <h3>Activity</h3>
                  <span className="pm-muted">Across test runs</span>
                </div>
                {(['turns', 'functionCalls', 'functionErrors'] as const).map(
                  (id) => (
                    <div className="pm-metric" key={id}>
                      <span>{labels[id]}</span>
                      {readout(id)}
                    </div>
                  ),
                )}
              </section>
            </div>
            <div className="pm-availability">
              <span>— Not reported</span>
              <span>Partial values include only reported samples.</span>
              {comparing ? (
                <span>
                  Deltas require matching test cases and repetitions with
                  complete metrics.
                </span>
              ) : null}
            </div>
          </>
        ) : (
          <section className="pm-tests" aria-label="Metrics by test">
            <div className="pm-tests-heading">
              <h3>Test results</h3>
              <span className="pm-muted">
                {tests.length} {tests.length === 1 ? 'test' : 'tests'}
              </span>
            </div>
            <Tabs
              value={group}
              onValueChange={(value) => setGroup(value as keyof typeof groups)}
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
                        {labels[id]}
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
                        {test.version !== null ? (
                          <small>Version {test.version}</small>
                        ) : null}
                        <div className="pm-test-actions">
                          {baselineExecutionId && test.baseline ? (
                            <ScenarioChatAction
                              executionId={baselineExecutionId}
                              scenarioId={test.label}
                              scenarioVersion={test.version}
                              label={comparing ? 'Transcript A' : 'Transcript'}
                            />
                          ) : null}
                          {candidateExecutionId && test.candidate ? (
                            <ScenarioChatAction
                              executionId={candidateExecutionId}
                              scenarioId={test.label}
                              scenarioVersion={test.version}
                              label="Transcript B"
                            />
                          ) : null}
                        </div>
                        {comparing && (!test.baseline || !test.candidate) ? (
                          <small>
                            No result in {!test.baseline ? 'A' : 'B'}
                          </small>
                        ) : null}
                      </th>
                      {groups[group].map((id) => (
                        <td key={id} data-label={labels[id]}>
                          <Readout
                            id={id}
                            baseline={test.baseline?.metrics[id]}
                            candidate={test.candidate?.metrics[id]}
                            comparing={comparing}
                            delta={test.deltas?.[id]}
                          />
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </DataTable>
            </div>
          </section>
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
                      : test.baseline.metrics.score.value === 0
                        ? 'Zero score in A'
                        : test.candidate.metrics.score.value === 0
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
