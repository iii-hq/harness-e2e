// The worker's answers to trends-get and version-compare over the Trends
// canvas's series (src/test-fixtures/trends.json: real executions of the dev
// Console on Sep 28), for the browser checks and captures. No worker runs.
import { readFileSync } from 'node:fs'

export const fixture = JSON.parse(
  readFileSync(new URL('../src/test-fixtures/trends.json', import.meta.url)),
)

const stackOf = (point) => point.stack.name ?? 'not_recorded'
const key = ({ suite, provider, model, profile }) => ({
  suite,
  provider,
  model,
  profile,
})

const RFC3339 =
  /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$/

/** A bound of the period: an RFC 3339 instant, else the worker refuses. */
function bound(name, value) {
  if (value === undefined) return null
  if (typeof value !== 'string' || !RFC3339.test(value))
    throw new Error(`${name} is not an RFC 3339 instant: ${value}`)
  return Date.parse(value)
}

/** trends-get as the worker answers it: the latest series that fits what
 *  the request names (suite, then model, then profile), else the one with
 *  the latest execution. The period (since/until, inclusive) limits the
 *  points and the stacks' counts, never the series; an execution whose
 *  start cannot be read is left out once a bound is set. Every stack the
 *  series ever ran on is listed, latest first, then not_recorded and any;
 *  the stack filter applies when listed (even at 0 in the period), else
 *  any, the one applied always said in `stack`. */
export function trendsAnswer(request = {}) {
  const since = bound('since', request.since)
  const until = bound('until', request.until)
  const fits = ({ series }) =>
    (!request.suite || series.suite === request.suite) &&
    (!request.provider || series.provider === request.provider) &&
    (!request.model || series.model === request.model) &&
    (request.profile === undefined ||
      (series.profile || null) === (request.profile || null))
  const chosen = fixture.series.find(fits) ?? fixture.series[0]
  const all = chosen.points
  const points =
    since === null && until === null
      ? all
      : all.filter((point) => {
          const at = Date.parse(point.started_at)
          return (
            !Number.isNaN(at) &&
            (since === null || at >= since) &&
            (until === null || at <= until)
          )
        })
  const names = [
    ...new Set(all.map((point) => point.stack.name).reverse()),
  ].filter(Boolean)
  const count = (name) =>
    points.filter((point) => name === 'any' || stackOf(point) === name).length
  const stacks = [
    ...names,
    ...(all.some((point) => !point.stack.name) ? ['not_recorded'] : []),
    'any',
  ].map((name) => ({ name, executions: count(name) }))
  const stack = stacks.some((item) => item.name === request.stack)
    ? request.stack
    : 'any'
  return {
    series: fixture.series.map((item) => item.series),
    selected: key(chosen.series),
    stack,
    stacks,
    points:
      stack === 'any'
        ? points
        : points.filter((point) => stackOf(point) === stack),
  }
}

// The commits GitHub counted on Sep 28 (Trends.dc.html's COMMITS).
const COMMITS = {
  'iii|0.24.2-rc.2|0.24.3-rc.1': 9,
  'ade|1.9.41|1.9.42': 2,
  'harness|1.8.31|1.8.34': 19,
  'harness|1.8.34|1.8.35': 7,
  'harness|1.8.35|1.8.36': 9,
  'iii-directory|1.2.29|1.2.30': 2,
}

const REPOSITORY = { iii: 'iii-hq/iii', 'harness-e2e': 'iii-hq/harness-e2e' }

/** version-compare as the contract says: tags of the release, or both
 *  commits in the same repository. */
export function compareAnswer({ name, base, head }) {
  const repository = REPOSITORY[name] ?? 'iii-hq/workers'
  const commits = base.startsWith('@') && head.startsWith('@')
  const [from, to] = commits
    ? [base.slice(1), head.slice(1)]
    : [`${name}/v${base}`, `${name}/v${head}`]
  return {
    url: `https://github.com/${repository}/compare/${from}...${to}`,
    total_commits:
      COMMITS[`${name}|${commits ? from : base}|${commits ? to : head}`] ??
      null,
  }
}
