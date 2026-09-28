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

/** trends-get as the worker answers it: the latest series that fits what
 *  the request names (suite, then model, then profile), else the one with
 *  the latest execution; every stack the series ran on, latest first,
 *  then not_recorded and any; the stack filter when the series lists it,
 *  else (none asked, or one it never ran on) any, the one applied always
 *  said in `stack`. */
export function trendsAnswer(request = {}) {
  const fits = ({ series }) =>
    (!request.suite || series.suite === request.suite) &&
    (!request.provider || series.provider === request.provider) &&
    (!request.model || series.model === request.model) &&
    (request.profile === undefined ||
      (series.profile || null) === (request.profile || null))
  const chosen = fixture.series.find(fits) ?? fixture.series[0]
  const points = chosen.points
  const names = [
    ...new Set(points.map((point) => point.stack.name).reverse()),
  ].filter(Boolean)
  const stacks = names.map((name) => ({
    name,
    executions: points.filter((point) => point.stack.name === name).length,
  }))
  const unrecorded = points.filter((point) => !point.stack.name).length
  if (unrecorded) stacks.push({ name: 'not_recorded', executions: unrecorded })
  stacks.push({ name: 'any', executions: points.length })
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
