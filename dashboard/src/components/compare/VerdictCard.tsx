import { AlertTriangle } from 'lucide-react'
import { plural } from '@/lib/format'
import {
  changeKey,
  type ExecutionVerdict,
  repetitionsText,
  ruleText,
  verdictHeadline,
  verdictTone,
} from '@/lib/verdict'

/** The verdict as the page holds it: loading while both are null, waiting
 *  while a side still runs. */
export type VerdictLoad = {
  value: ExecutionVerdict | null
  error: string | null
  waiting?: boolean
}

const signed = (value: number) =>
  `${value > 0 ? '+' : value < 0 ? '−' : ''}${Math.abs(value).toFixed(1)}`

/** Whether B is better than A, worse, no different or cannot tell yet, as
 *  the worker reads it over every counted run of both, whatever tests the
 *  page counts below; with its reasons, the repetitions and the criteria
 *  that explain the change. Warnings only: nothing is hidden. */
export function VerdictCard({ load }: { load: VerdictLoad }) {
  const { value: verdict, error, waiting } = load
  if (!verdict)
    return (
      <section
        className="cmp-card cmp-verdict"
        aria-labelledby="cmp-verdict"
        data-comparison-verdict={
          error ? 'error' : waiting ? 'waiting' : 'loading'
        }
      >
        <span id="cmp-verdict" className="cmp-eyebrow">
          Verdict · B compared with A
        </span>
        {error ? (
          <p className="cmp-warning">
            <AlertTriangle size={16} aria-hidden="true" />
            <span>The verdict could not be read: {error}</span>
          </p>
        ) : (
          <p className="cmp-faint" role="status">
            {waiting
              ? 'The verdict is read once both executions finish.'
              : 'Reading the verdict…'}
          </p>
        )}
      </section>
    )
  const { criteria } = verdict
  const insufficient = criteria.configurations.filter(
    (configuration) => !configuration.sufficient,
  ).length
  const classes = criteria.configurations
    .flatMap((configuration) => configuration.criteria)
    .filter((criterion) => criterion.class !== 'insufficient_data')
  const classCount = (name: string, label: string) =>
    `${classes.filter((criterion) => criterion.class === name).length} ${label}`
  const unevaluated = criteria.configurations.reduce(
    (total, configuration) => total + configuration.not_evaluated.length,
    0,
  )
  const health = [
    `Criterion health: ${plural(criteria.configurations.length, 'configuration')}`,
    insufficient > 0
      ? `${insufficient} with fewer than ${criteria.min_runs} counted runs: insufficient data`
      : null,
    classes.length > 0
      ? `${classCount('saturated', 'criteria saturated')}, ${classCount('never_full', 'never full')}, ${classCount('oscillating', 'oscillating')}`
      : null,
    unevaluated > 0
      ? `${plural(unevaluated, 'criterion', 'criteria')} no counted run evaluated`
      : null,
  ]
    .filter(Boolean)
    .join(' · ')
  return (
    <section
      className="cmp-card cmp-verdict"
      aria-labelledby="cmp-verdict"
      data-comparison-verdict={verdict.verdict ?? 'none'}
    >
      <span id="cmp-verdict" className="cmp-eyebrow">
        Verdict · B compared with A
      </span>
      <p
        className="cmp-headline cmp-tone"
        data-tone={verdictTone(verdict)}
        data-verdict-headline
      >
        {verdictHeadline(verdict)}
      </p>
      {verdict.unavailable ? (
        <p className="cmp-faint">{verdict.unavailable}</p>
      ) : null}
      {verdict.reasons.length > 0 ? (
        <ul className="cmp-verdict-list">
          {verdict.reasons.map((reason) => (
            <li key={reason}>{reason}</li>
          ))}
        </ul>
      ) : null}
      <p className="cmp-faint" data-verdict-repetitions>
        {repetitionsText(verdict)}
      </p>
      {verdict.warnings.map((warning) => (
        <p className="cmp-warning" key={warning}>
          <AlertTriangle size={16} aria-hidden="true" />
          <span>{warning}</span>
        </p>
      ))}
      {criteria.changes.length > 0 ? (
        <>
          <span className="cmp-eyebrow">
            Criteria that moved the score, failing together
          </span>
          <ul className="cmp-verdict-list" data-verdict-changes>
            {criteria.changes.map((change) => (
              <li key={changeKey(change)}>
                <span className="cmp-mono cmp-strong">{change.scenario}</span>{' '}
                {change.criteria.join(', ')}: short in {change.failing.a} of{' '}
                {change.runs.a} runs on A, {change.failing.b} of {change.runs.b}{' '}
                on B, {signed(change.delta)} points
              </li>
            ))}
          </ul>
        </>
      ) : null}
      <p className="cmp-faint" data-verdict-health>
        {`${health}.`}
      </p>
      <p className="cmp-faint">
        {`Over every counted run of both executions, whatever the tests counted below. ${ruleText(verdict.rule)}`}
      </p>
    </section>
  )
}
