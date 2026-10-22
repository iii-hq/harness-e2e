// Projects one tour of the onboarding worker's src/tours.mjs to the shape
// src/scenarios/onboarding_tour.rs compares the served tour with:
//
//   node scripts/onboarding_tour_fixture.mjs <workers>/onboarding/src/tours.mjs learn-iii \
//     > tests/fixtures/onboarding-tour/learn-iii.json
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const [source, id] = process.argv.slice(2)
const { getTour } = await import(pathToFileURL(resolve(source)).href)
const tour = getTour(id)
if (!tour) throw new Error(`${source} has no tour ${id}`)
const step = ({ id, ask, condition }) => ({
  id,
  ...(ask ? { ask: ask.text } : {}),
  ...(ask?.agent ? { agent: ask.agent } : {}),
  ...(condition ? { condition: { type: condition.type, config: condition.config ?? {} } } : {}),
})
process.stdout.write(`${JSON.stringify({ id: tour.id, steps: tour.steps.map(step) }, null, 2)}\n`)
