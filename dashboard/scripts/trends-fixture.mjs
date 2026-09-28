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

/** trends-get as the contract says: the request's series, else the one
 *  with the latest execution; the stack filter, else the latest
 *  execution's stack. */
export function trendsAnswer(request = {}) {
  const chosen =
    fixture.series.find(
      ({ series }) =>
        series.suite === request.suite &&
        series.provider === request.provider &&
        series.model === request.model &&
        (series.profile || null) === (request.profile || null),
    ) ?? fixture.series[0]
  const points = chosen.points
  const names = [...new Set(points.map((point) => point.stack.name))].filter(
    Boolean,
  )
  const stacks = names.map((name) => ({
    name,
    executions: points.filter((point) => point.stack.name === name).length,
  }))
  const unrecorded = points.filter((point) => !point.stack.name).length
  if (unrecorded) stacks.push({ name: 'not_recorded', executions: unrecorded })
  stacks.push({ name: 'any', executions: points.length })
  const stack = stacks.some((item) => item.name === request.stack)
    ? request.stack
    : stackOf(points.at(-1))
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
  'harness-e2e|0.11.21|0.11.24': 7,
  'harness-e2e|0.11.24|0.12.4': 24,
  'harness-e2e|0.12.4|0.14.0': 17,
  'harness-e2e|0.14.0|0.15.0': 6,
  'harness-e2e|0.15.0|0.15.1': 5,
  'harness-e2e|0.15.1|0.16.2': 15,
  'harness-e2e|0.16.2|0.17.0': 2,
  'harness-e2e|0.11.28|0.12.6': 18,
  'harness-e2e|7a16130|2f8826a': 1,
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
