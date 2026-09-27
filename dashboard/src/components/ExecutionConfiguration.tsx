import { ChevronRight, ExternalLink } from 'lucide-react'
import { type ReactNode, useId, useState } from 'react'
import { StackPanel } from '@/components/execution/StackPanel'
import type { Stack } from '@/lib/dashboard-data-source'
import { providerModel, suiteText } from '@/lib/execution-view'
import type { PlanExecution } from '@/lib/plan-execution'

/** Where the execution came from, with a link to its GitHub run. */
export function ExecutionOriginLink({
  source,
}: {
  source: PlanExecution['source']
}) {
  if (source.kind === 'docker') return <>Docker · attempt {source.attempt}</>
  if (source.kind !== 'github') return <>local</>
  return (
    <a
      className="inline-flex items-center gap-1 text-ink"
      href={source.url}
      target="_blank"
      rel="noreferrer"
    >
      GitHub #{source.run_id} · attempt {source.run_attempt}
      <ExternalLink size={12} aria-hidden="true" />
    </a>
  )
}

/** What the execution ran with, as one band of facts under the header, and
 *  the stack it ran on behind "Stack · N workers" (canvas: Execution detail). */
export function ExecutionFacts({
  execution,
  extra = [],
  loadStacks,
}: {
  execution: PlanExecution | null
  /** The Console's stacks, for a run that did not record its own. */
  loadStacks?: () => Promise<{ stacks: Stack[] }>
  /** Facts the page knows without a plan execution (started, versions). */
  extra?: Array<[string, ReactNode]>
}) {
  const [stackOpen, setStackOpen] = useState(false)
  const stackId = useId()
  const rows: Array<[string, ReactNode]> = []
  if (execution) {
    const parameters = execution.parameters
    const source = execution.source
    rows.push(['Where', <ExecutionOriginLink key="origin" source={source} />])
    if (parameters)
      rows.push(
        ['Suite', suiteText(parameters.suite) ?? 'not recorded'],
        ['Model', providerModel(parameters)],
        ['Profile', parameters.agent ?? 'default'],
      )
    if (parameters?.stack)
      rows.push([
        'Stack',
        `${parameters.stack.name}${parameters.stack.sha256 ? ` · ${parameters.stack.sha256.replace('sha256:', '').slice(0, 12)}` : ''}`,
      ])
    else if (source.kind === 'github' && source.stack)
      rows.push(['Stack', source.stack])
    if (source.kind === 'docker' && source.image)
      rows.push(['Image', source.image])
    if (source.kind === 'github')
      rows.push([
        'Release Control',
        source.release_control_execution_id ?? 'not reported',
      ])
    if (parameters && parameters.runs > 1)
      rows.push(['Runs', String(parameters.runs)])
    if (parameters && parameters.technical_retries > 0)
      rows.push(['Retries', String(parameters.technical_retries)])
  }
  rows.push(...extra.filter(([label]) => !rows.some(([key]) => key === label)))
  // The canvas order; anything else keeps its place after these.
  const ORDER = [
    'Where',
    'Model',
    'Profile',
    'Runner',
    'Harness',
    'Suite',
    'Stack',
    'Image',
    'Release Control',
    'Runs',
    'Retries',
    'Id',
  ]
  const rank = (label: string) => {
    const index = ORDER.indexOf(label)
    return index === -1 ? ORDER.length - 1 : index
  }
  rows.sort(([a], [b]) => rank(a) - rank(b))
  const stack = execution?.stack ?? []
  const workers = new Set(stack.map((worker) => worker.name)).size
  const differing = new Set(
    stack
      .filter((worker) => worker.groups?.length)
      .map((worker) => worker.name),
  ).size
  return (
    <section
      className="ep-facts"
      aria-label="Execution facts"
      data-execution-facts
    >
      <div className="ep-facts-row">
        {rows.map(([label, value]) => (
          <span className="ep-fact" key={label}>
            <span className="ep-fact-label">{label}</span>
            <span className="ep-fact-value">{value}</span>
          </span>
        ))}
        {workers > 0 ? (
          <button
            type="button"
            className="ep-act ep-fact-stack"
            aria-expanded={stackOpen}
            data-selected={stackOpen}
            aria-controls={stackId}
            onClick={() => setStackOpen(!stackOpen)}
            data-stack-toggle
          >
            Stack · {workers} {workers === 1 ? 'worker' : 'workers'}
            {differing ? ` · ${differing} differ between groups` : ''}
            <ChevronRight
              size={14}
              aria-hidden="true"
              className={stackOpen ? 'ep-rot' : undefined}
            />
          </button>
        ) : null}
      </div>
      {stackOpen && execution ? (
        <div className="ep-facts-stack">
          <StackPanel
            execution={execution}
            id={stackId}
            loadStacks={loadStacks}
          />
        </div>
      ) : null}
    </section>
  )
}
