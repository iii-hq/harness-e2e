import { ChevronRight } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { hashForStacks } from '@/hooks/use-hash-route'
import type { Stack, StackWorker } from '@/lib/dashboard-data-source'
import type { PlanExecution } from '@/lib/plan-execution'
import './execution-page.css'

export type StackYamlContainer = {
  name: string
  worker: string | null
  version: string | null
  commit: string | null
}

function unquote(value: string) {
  const text = value.trim()
  return text.replace(/^(['"])(.*)\1$/, '$2') || null
}

/** The keys a stack.yaml writes: `iii`, `template` and its containers. A small
 *  reader for the stack format (top-level keys, `containers:` as a map of
 *  name to worker/version/commit); anything else is ignored. */
export function parseStackYaml(yaml: string): {
  iii: string | null
  template: string | null
  containers: StackYamlContainer[]
} {
  let iii: string | null = null
  let template: string | null = null
  const containers: StackYamlContainer[] = []
  let inContainers = false
  let containerIndent: number | null = null
  let current: StackYamlContainer | null = null
  for (const raw of yaml.split('\n')) {
    const line = raw.replace(/(^|\s)#.*$/, '')
    if (!line.trim()) continue
    const indent = line.length - line.trimStart().length
    const match = line.trim().match(/^([A-Za-z0-9_.-]+):\s*(.*)$/)
    if (!match) continue
    const [, key, value] = match
    if (indent === 0) {
      inContainers = key === 'containers'
      containerIndent = null
      current = null
      if (key === 'iii') iii = unquote(value)
      if (key === 'template') template = unquote(value)
      continue
    }
    if (!inContainers) continue
    if (containerIndent === null) containerIndent = indent
    if (indent === containerIndent) {
      current = { name: key, worker: null, version: null, commit: null }
      containers.push(current)
      continue
    }
    if (!current) continue
    if (key === 'worker') current.worker = unquote(value)
    if (key === 'version') current.version = unquote(value)
    if (key === 'commit') current.commit = unquote(value)
  }
  return { iii, template, containers }
}

/** What the harness's own workers are for, as the stack's comments say. */
const ROLES: Record<string, string> = {
  harness: 'the application under test',
  'harness-e2e': 'the runner',
}

function origin(worker: StackWorker) {
  if (worker.source === 'path')
    return `path${worker.commit ? ` @${worker.commit.slice(0, 7)}` : ''}${worker.dirty ? ' + changes' : ''}`
  return `package://${worker.name}`
}

const SHOWN = 5

/** The stack an execution ran on, opened from "Stack · N workers" (canvas:
 *  Execution detail · Stack): what it declares, what Compose added from the
 *  packages, and that every group starts from one lock. */
export function StackPanel({
  execution,
  id,
  loadStacks,
}: {
  execution: PlanExecution
  id?: string
  /** The stacks this Console knows, to name what an older run did not record. */
  loadStacks?: () => Promise<{ stacks: Stack[] }>
}) {
  const [more, setMore] = useState(false)
  const recorded = execution.parameters?.stack ?? null
  const named =
    execution.source.kind === 'github' ? (execution.source.stack ?? null) : null
  // An imported or Docker run that recorded no stack.yaml ran on a stack of
  // the repository: the one its contract names, else the default.
  const wanted =
    recorded?.yaml || execution.source.kind === 'local'
      ? null
      : (named ?? 'default')
  const [fallback, setFallback] = useState<Stack | null>(null)
  // The loader changes identity on every page render; read it once per
  // wanted stack.
  const loader = useRef(loadStacks)
  loader.current = loadStacks
  useEffect(() => {
    const load = loader.current
    if (!wanted || !load) return
    let live = true
    load()
      .then(({ stacks }) => {
        if (live)
          setFallback(
            stacks.find(
              (stack) => stack.id === wanted && stack.source === 'repository',
            ) ?? null,
          )
      })
      .catch(() => {})
    return () => {
      live = false
    }
  }, [wanted])
  const assumed = !recorded?.yaml && !named && fallback !== null
  const yaml = recorded?.yaml ?? fallback?.yaml ?? null
  const parsed = yaml ? parseStackYaml(yaml) : null
  const seen = new Set<string>()
  const workers = execution.stack.filter((worker) => {
    if (seen.has(worker.name)) return false
    seen.add(worker.name)
    return true
  })
  const declared = parsed?.containers ?? []
  const declaredNames = new Set(declared.map((container) => container.name))
  // A declared container the run did not resolve (an unrecorded run compared
  // with today's stack) says so on its row.
  const missing = new Set(
    declared
      .filter((container) => !seen.has(container.name))
      .map((container) => container.name),
  )
  const dependencies = workers.filter(
    (worker) => !declaredNames.has(worker.name),
  )
  const groups = new Set(execution.slots.map((slot) => slot.group_id)).size
  const name = recorded?.name ?? named ?? fallback?.id ?? null
  const line = [
    parsed?.iii ? `iii ${parsed.iii}` : null,
    parsed?.template ? `template ${parsed.template}` : null,
    `${workers.length} ${workers.length === 1 ? 'worker' : 'workers'}`,
    groups > 1 ? 'one worker-compose.lock for every group' : null,
  ]
    .filter(Boolean)
    .join(' · ')
  const kpis: Array<[string, string, string]> = [
    ['Workers', String(workers.length), 'as Compose resolved them'],
    ...(parsed
      ? ([
          [
            'Declared',
            String(declared.length),
            missing.size
              ? `${declared.length - missing.size} of them ran`
              : 'containers in the stack',
          ],
          [
            'From dependencies',
            String(dependencies.length),
            'resolved from the packages',
          ],
        ] as Array<[string, string, string]>)
      : []),
    [
      'Groups',
      String(groups),
      groups > 1 ? 'each starts from the same lock' : 'one group ran it',
    ],
  ]
  const shownDependencies = more ? dependencies : dependencies.slice(0, SHOWN)
  const dependencyRows = parsed ? shownDependencies : workers
  return (
    <section
      id={id}
      className="ep-stack"
      aria-label="Stack"
      data-execution-stack
    >
      <div className="ep-stack-head">
        <span className="ep-strong">Stack</span>
        {name ? <span className="ep-mono ep-strong">{name}</span> : null}
        <span className="ep-stack-line">{line}</span>
        {assumed ? (
          <span className="ep-stack-line" data-stack-assumed>
            · not recorded by this run: the repository default
          </span>
        ) : null}
        <a className="ep-act ep-act-outline" href={hashForStacks()}>
          Open in Stacks
          <ChevronRight size={14} aria-hidden="true" />
        </a>
      </div>
      <dl className="ep-kpis ep-stack-kpis">
        {kpis.map(([label, value, sub]) => (
          <div className="ep-kpi" key={label}>
            <dt className="ep-kpi-label">{label}</dt>
            <dd className="ep-kpi-value">{value}</dd>
            <dd className="ep-kpi-sub ep-stack-sub">{sub}</dd>
          </div>
        ))}
      </dl>
      <div className={parsed ? 'ep-stack-cols' : undefined}>
        {parsed ? (
          <div className="ep-stack-col">
            <h3 className="ep-stack-title">
              Declared by the stack{' '}
              <span className="ep-count">{declared.length}</span>
            </h3>
            <p className="ep-stack-help">
              The containers written in the stack. Each names its worker package
              and version.
            </p>
            <table className="ep-stack-table">
              <thead>
                <tr>
                  <th scope="col">Worker</th>
                  <th scope="col">Version</th>
                </tr>
              </thead>
              <tbody>
                {declared.map((container) => (
                  <tr key={container.name} data-stack-declared={container.name}>
                    <td>
                      <span className="ep-stack-name">
                        {container.name}
                        {missing.has(container.name) ? (
                          <span className="ep-stack-chip">not in this run</span>
                        ) : null}
                      </span>
                      <span className="ep-stack-origin">
                        {[container.worker, ROLES[container.name]]
                          .filter(Boolean)
                          .join(' · ')}
                      </span>
                    </td>
                    <td className="ep-stack-version">
                      {container.version ??
                        container.commit?.slice(0, 12) ??
                        '—'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : null}
        <div className="ep-stack-col">
          <h3 className="ep-stack-title">
            {parsed ? 'From dependencies' : 'Workers'}{' '}
            <span className="ep-count">
              {parsed ? dependencies.length : workers.length}
            </span>
          </h3>
          <p className="ep-stack-help">
            {parsed
              ? 'Compose adds these from the declared packages. Their versions come from worker-compose.lock.'
              : 'This harness ran on its own stack: the workers it resolved, with their versions.'}
          </p>
          <table className="ep-stack-table">
            <thead>
              <tr>
                <th scope="col">Worker</th>
                <th scope="col">Version</th>
              </tr>
            </thead>
            <tbody>
              {dependencyRows.map((worker) => (
                <tr key={worker.name} data-stack-dependency={worker.name}>
                  <td>
                    <span className="ep-stack-name">
                      {worker.name}
                      {worker.groups?.length ? (
                        <span className="ep-stack-chip">
                          differs in {worker.groups.length}{' '}
                          {worker.groups.length === 1 ? 'group' : 'groups'}
                        </span>
                      ) : null}
                    </span>
                    <span className="ep-stack-origin">{origin(worker)}</span>
                  </td>
                  <td
                    className={`ep-stack-version ${worker.observed ? '' : 'ep-stack-lock'}`}
                  >
                    {worker.resolved ??
                      worker.observed ??
                      worker.requested ??
                      'from the lock'}
                  </td>
                </tr>
              ))}
              {parsed && dependencies.length > SHOWN ? (
                <tr>
                  <td colSpan={2}>
                    <button
                      type="button"
                      className="ep-stack-more"
                      aria-expanded={more}
                      onClick={() => setMore(!more)}
                    >
                      {more
                        ? 'Show fewer'
                        : `and ${dependencies.length - SHOWN} more in worker-compose.lock`}
                    </button>
                  </td>
                </tr>
              ) : null}
            </tbody>
          </table>
        </div>
      </div>
      <p className="ep-stack-help">
        Each group also gets its own namespace, data directory and provider
        credentials. Those are stamped per group and never go in a stack.
      </p>
    </section>
  )
}
