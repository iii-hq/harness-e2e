import { ExternalLink } from 'lucide-react'
import type { ReactNode } from 'react'
import { DisclosureLayer } from '@/components/DisclosureLayer'
import { DataTable } from '@/design-system'
import { providerModel } from '@/lib/execution-view'
import type { PlanExecution } from '@/lib/plan-execution'

/** Where the execution came from, with a link to its GitHub run. */
export function ExecutionOriginLink({
  source,
}: {
  source: PlanExecution['source']
}) {
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

/** What the execution ran with (what running it again needs), where it came
 *  from, the stack it ran on and the warnings recorded with it. */
export function ExecutionConfiguration({
  execution,
}: {
  execution: PlanExecution
}) {
  const parameters = execution.parameters
  const source = execution.source
  const rows: Array<[string, ReactNode]> = [
    ['origin', <ExecutionOriginLink key="origin" source={source} />],
  ]
  if (source.kind === 'github' && source.release_control_execution_id)
    rows.push(['release control', source.release_control_execution_id])
  if (parameters)
    rows.push(
      ['model', providerModel(parameters)],
      ['profile', parameters.agent ?? 'default'],
      ['scenarios', parameters.scenarios.join(', ')],
      ['runs', String(parameters.runs)],
      ['technical retries', String(parameters.technical_retries)],
      [
        'seed',
        parameters.seed === null ? 'canonical' : String(parameters.seed),
      ],
    )
  const differing = new Set(
    execution.stack
      .filter((worker) => worker.groups?.length)
      .map((worker) => worker.name),
  )
  return (
    <section
      id="configuration"
      className="mt-6 grid min-w-0 scroll-mt-24 gap-3"
      aria-labelledby="execution-configuration-heading"
      data-execution-configuration
    >
      <h2
        id="execution-configuration-heading"
        className="m-0 text-base font-semibold text-ink"
      >
        Configuration
      </h2>
      <dl className="m-0 grid min-w-0 grid-cols-[max-content_minmax(0,1fr)] gap-x-6 gap-y-2 font-mono text-xs">
        {rows.map(([key, value]) => (
          <div key={key} className="contents">
            <dt className="ds-label">{key}</dt>
            <dd className="m-0 break-words text-ink">{value}</dd>
          </div>
        ))}
      </dl>
      {execution.warnings?.length ? (
        <ul
          className="m-0 grid gap-1 pl-4 text-xs text-warning"
          aria-label="Execution warnings"
          data-execution-warnings
        >
          {execution.warnings.map((warning) => (
            <li key={warning}>{warning}</li>
          ))}
        </ul>
      ) : null}
      {execution.stack.length > 0 ? (
        <DisclosureLayer
          id="stack"
          label="stack"
          scent={`${new Set(execution.stack.map((worker) => worker.name)).size} workers${differing.size ? ` · ${differing.size} differ between groups` : ''}`}
          open={false}
        >
          <DataTable caption="Stack workers" collapse data-execution-stack>
            <thead>
              <tr>
                <th scope="col">worker</th>
                <th scope="col">source</th>
                <th scope="col">requested</th>
                <th scope="col">observed</th>
                <th scope="col">groups</th>
              </tr>
            </thead>
            <tbody>
              {execution.stack.map((worker) => (
                <tr
                  key={`${worker.name}:${worker.requested}:${worker.observed}:${worker.groups?.join(',')}`}
                >
                  <td data-label="worker" className="font-mono text-xs">
                    {worker.name}
                  </td>
                  <td data-label="source" className="font-mono text-xs">
                    {worker.source}
                    {worker.commit ? ` · ${worker.commit.slice(0, 12)}` : ''}
                    {worker.dirty ? ' · dirty' : ''}
                  </td>
                  <td data-label="requested" className="font-mono text-xs">
                    {worker.requested ?? '—'}
                  </td>
                  <td data-label="observed" className="font-mono text-xs">
                    {worker.observed ?? '—'}
                  </td>
                  <td
                    data-label="groups"
                    className="font-mono text-xs text-ink-muted"
                  >
                    {worker.groups?.length ? worker.groups.join(', ') : 'all'}
                  </td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        </DisclosureLayer>
      ) : null}
    </section>
  )
}
