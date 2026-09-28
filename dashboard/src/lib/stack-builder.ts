/* The New stack builder as the redesign canvas draws it (StackBuilder.dc.html):
   what each declared worker does and whether it stops Create, the YAML the
   form writes, the picker's groups and the footer's status. The rules are the
   executor's (scripts/exact_stack_campaign.py project_scaffold and
   run_exact_stack_group.sh): with a template each group runs the template's
   project and the stack only pins versions of what it declares; without one,
   a worker that arrives as another's dependency must not be declared again.
   The runner itself never refuses either: only the builder does. */
import type {
  Stack,
  StackTemplate,
  WorkerResolution,
} from '@/lib/dashboard-data-source'

/** A container the stack declares. */
export type Declared = {
  name: string
  worker: string
  /** `latest`, a version, or '' while one is typed; unused with a commit. */
  version: string
  /** A commit, '' while one is typed; null pins a version. */
  commit: string | null
}

/** What the iii registry said about a worker, by name. */
export type Lookup =
  | { state: 'checking' }
  | { state: 'found'; version: string; dependencies: string[] }
  | { state: 'missing'; message: string }
  | { state: 'failed'; message: string }

export type Lookups = Partial<Record<string, Lookup>>

export type Tone = 'info' | 'block' | 'warn' | 'pending'

export type Verdict = { tone: Tone; lead: string; text: string }

/** One provider per model family, as the canvas lists them. */
export const PROVIDERS = [
  'provider-anthropic',
  'provider-openai',
  'provider-deepseek',
  'provider-zai',
  'provider-kimi',
  'provider-xai',
  'provider-openrouter',
  'provider-llamacpp',
  'provider-openai-codex',
  'provider-claude-code',
  'provider-github-copilot',
]

/** What the executor adds to every group, whatever the stack declares. */
export const ADDED_WHEN_IT_RUNS = [
  { name: 'harness-e2e', why: 'the runner, in every group' },
  { name: 'the model’s provider', why: 'for the model a run picks' },
  { name: 'canvas', why: 'for visual tests' },
  { name: 'iii-directory', why: 'when a run uses an agent profile' },
]

/** A name the registry could hold: `^[a-z0-9][a-z0-9-]{0,63}$`. */
export const WORKER_NAME = /^[a-z0-9][a-z0-9-]{0,63}$/

const PACKAGE = 'package://'

export function packageName(worker: string) {
  return worker.startsWith(PACKAGE) ? worker.slice(PACKAGE.length) : null
}

function isProvider(name: string) {
  return name.startsWith('provider-')
}

/** The id a stack's `template:` names, without its `@revision`. */
export function templateId(template: string | null) {
  return template ? template.split('@')[0] : null
}

export function lookupOf(resolution: WorkerResolution): Lookup {
  return 'error' in resolution
    ? { state: 'missing', message: resolution.error.message }
    : {
        state: 'found',
        version: resolution.version,
        dependencies: resolution.dependencies,
      }
}

/** The declared worker that brings `name` with it, by the registry. */
export function hostOf(name: string, declared: Declared[], lookups: Lookups) {
  return (
    declared.find((other) => {
      const own = packageName(other.worker)
      if (!own || own === name) return false
      const found = lookups[own]
      return found?.state === 'found' && found.dependencies.includes(name)
    })?.name ?? null
  )
}

/** A pin still being typed. */
export function unfinished(entry: Declared) {
  return entry.commit === '' || (entry.commit === null && entry.version === '')
}

/** What a declared worker does, and whether it stops Create. With a template
 *  that did not load, nothing is said: it cannot be checked. */
export function judge(
  entry: Declared,
  template: string | null,
  tpl: StackTemplate | null,
  declared: Declared[],
  lookups: Lookups,
): Verdict | null {
  if (template) {
    if (!tpl) return null
    if (tpl.workers.some((worker) => worker.name === entry.name)) {
      const text =
        entry.commit === ''
          ? 'Type the commit to pin it.'
          : entry.commit
            ? `The template keeps it, built from commit ${entry.commit.slice(0, 7)} with the dependencies that commit declares.`
            : entry.version === ''
              ? 'Type the version to pin it.'
              : entry.version !== 'latest'
                ? `The template keeps it, at ${entry.version}.`
                : 'The template keeps it, at the newest release.'
      return { tone: 'info', lead: `Pins the template’s ${entry.name}.`, text }
    }
    if (entry.name === 'harness-e2e')
      return {
        tone: 'info',
        lead: 'Pins the runner.',
        text: 'The executor adds harness-e2e to every group; this sets its version.',
      }
    if (isProvider(entry.name))
      return {
        tone: 'info',
        lead: `Pins it for runs that pick a ${entry.name.replace('provider-', '')} model.`,
        text: 'The template doesn’t bring it; the executor adds the chosen model’s provider, at this version.',
      }
    return {
      tone: 'block',
      lead: 'Would be ignored.',
      text: `With a template, each group runs the ${tpl.id} project and this stack only pins versions of what it declares. ${entry.name} isn’t in it, so it would never start. Remove it, or start without a template.`,
    }
  }
  const own = packageName(entry.worker)
  const host = hostOf(own ?? entry.name, declared, lookups)
  if (host)
    return {
      tone: 'block',
      lead: `Already arrives with ${host}.`,
      text: `Declaring it here is a second spec for the same worker, and the engine refuses it. Remove it; ${host} brings it.`,
    }
  if (!own) return null
  const found = lookups[own]
  if (!found || found.state === 'checking')
    return { tone: 'pending', lead: 'Checking the registry…', text: '' }
  if (found.state === 'missing')
    return {
      tone: 'warn',
      lead: 'Not in the iii registry.',
      text: found.message,
    }
  if (found.state === 'failed')
    return {
      tone: 'warn',
      lead: 'Couldn’t check the registry.',
      text: `${found.message} Nothing is blocked for it.`,
    }
  if (found.dependencies.length)
    return {
      tone: 'info',
      lead: `Brings ${found.dependencies.join(', ')} with it.`,
      text: 'They start from its package; don’t declare them again.',
    }
  return null
}

/** A scalar as YAML reads it back as text: a version like 1.10 is quoted. */
function scalar(value: string) {
  return value !== '' && !Number.isNaN(Number(value))
    ? JSON.stringify(value)
    : value
}

/** The stack's YAML, as the form writes it. */
export function yamlOf(
  iii: string,
  template: string | null,
  declared: Declared[],
) {
  const out = [`iii: ${scalar(iii)}`]
  if (template) out.push(`template: ${template}`)
  out.push('', declared.length ? 'containers:' : 'containers: {}')
  for (const entry of declared) {
    out.push(`  ${entry.name}:`, `    worker: ${entry.worker}`)
    out.push(
      entry.commit !== null
        ? `    commit: ${JSON.stringify(entry.commit)}`
        : `    version: ${scalar(entry.version || 'latest')}`,
    )
  }
  out.push('', 'startup_timeout: 5m', 'stop_timeout: 30s')
  return out
}

/** The YAML's lines, each container's block marked when it stops Create. */
export function markedLines(lines: string[], blocked: Set<string>) {
  let inBad = false
  return lines.map((text) => {
    const head = /^ {2}([^\s:]+):$/.exec(text)
    if (head) inBad = blocked.has(head[1])
    else if (!text.startsWith('    ')) inBad = false
    return { text, blocked: inBad }
  })
}

/** What a declared worker's version button says. */
export function pinLabel(entry: Declared) {
  if (entry.commit !== null)
    return entry.commit ? `commit ${entry.commit.slice(0, 7)}` : 'commit …'
  return entry.version || 'version …'
}

/** Templates with workers first, each part in the order main lists them. */
export function orderTemplates(templates: StackTemplate[]) {
  return [
    ...templates.filter((entry) => entry.workers.length),
    ...templates.filter((entry) => !entry.workers.length),
  ]
}

/** What a template brings that its description does not say. */
export function templateExtra(tpl: StackTemplate) {
  if (tpl.note) return tpl.note
  return tpl.workers
    .filter((worker) => !packageName(worker.worker))
    .map(
      (worker) => `${worker.name} runs from ${worker.worker} in the project.`,
    )
    .join(' ')
}

/** A stack's containers as the builder declares them. */
export function declaredOf(stack: Stack): Declared[] {
  return stack.containers.map((container) => ({
    name: container.name,
    worker: container.worker ?? `${PACKAGE}${container.name}`,
    version: container.version ?? 'latest',
    commit: container.commit,
  }))
}

export type PickItem = {
  name: string
  sub: string
  tag: string
  /** The tag says why it would be ignored, or the registry refused it. */
  alert: boolean
  off: boolean
  /** What adding it declares; null when it can't be added or shows more. */
  add: Declared | null
}

export type PickGroup = { label: string; sub: string; items: PickItem[] }

/** The query as a worker's name: `package://` and case dropped. */
export function pickerQuery(query: string) {
  return query
    .trim()
    .replace(/^package:\/\//, '')
    .toLowerCase()
}

/** A whole name the picker's lists do not hold: checked in the registry. */
export function typedName(query: string, groups: PickGroup[]): string | null {
  const q = pickerQuery(query)
  const registry = groups.find(
    (group) => group.label === 'From the iii registry',
  )
  return registry && !registry.items[0]?.alert ? q : null
}

/** The picker: what the template, your stacks, the providers and the other
 *  templates offer, filtered by the query; a whole name no list holds is
 *  offered as the registry answers it. */
export function pickerGroups({
  query,
  template,
  tpl,
  declared,
  lookups,
  stacks,
  templates,
  showAll,
}: {
  query: string
  template: string | null
  tpl: StackTemplate | null
  declared: Declared[]
  lookups: Lookups
  stacks: Stack[]
  templates: StackTemplate[]
  showAll: boolean
}): PickGroup[] {
  const q = pickerQuery(query)
  const match = (name: string) => !q || name.includes(q)
  const tplWorkers = tpl?.workers ?? []
  const tplNames = tplWorkers.map((worker) => worker.name)
  const has = new Set(declared.map((entry) => entry.name))
  const item = (name: string, worker = `${PACKAGE}${name}`): PickItem => {
    const isAdded = has.has(name)
    const inTpl = tplNames.includes(name)
    const ignored =
      Boolean(template) && !inTpl && name !== 'harness-e2e' && !isProvider(name)
    const host = template ? null : hostOf(name, declared, lookups)
    const off = isAdded || ignored || Boolean(host)
    const tag = isAdded
      ? 'added'
      : ignored
        ? 'not in the template, ignored'
        : host
          ? `arrives with ${host}`
          : inTpl
            ? 'pins it'
            : name === 'harness-e2e'
              ? 'the runner, always added'
              : isProvider(name) && template
                ? 'added with its model'
                : ''
    return {
      name,
      sub: worker,
      tag,
      alert: ignored,
      off,
      add: off ? null : { name, worker, version: 'latest', commit: null },
    }
  }
  const packages = (workers: Array<string | null>) =>
    workers.flatMap((worker) => {
      const name = worker ? packageName(worker) : null
      return name ? [name] : []
    })
  const inStacks = [
    ...new Set(
      packages(
        stacks.flatMap((stack) =>
          stack.containers.map((container) => container.worker),
        ),
      ),
    ),
  ].filter((name) => !tplNames.includes(name))
  const providers = PROVIDERS.filter(
    (name) => !tplNames.includes(name) && !inStacks.includes(name),
  )
  const other = [
    ...new Set(
      packages(
        templates.flatMap((entry) =>
          entry.workers.map((worker) => worker.worker),
        ),
      ),
    ),
  ]
    .filter(
      (name) =>
        !tplNames.includes(name) &&
        !inStacks.includes(name) &&
        !providers.includes(name),
    )
    .sort()
  const groups: PickGroup[] = []
  if (tpl) {
    const inT = tplWorkers.filter((worker) => match(worker.name))
    const shown = q || showAll ? inT : inT.slice(0, 4)
    const items = shown.map((worker) => item(worker.name, worker.worker))
    if (shown.length < inT.length)
      items.push({
        name: `Show ${inT.length - shown.length} more`,
        sub: 'the rest of the template’s workers',
        tag: '',
        alert: false,
        off: false,
        add: null,
      })
    groups.push({
      label: `In the ${tpl.id} template`,
      sub: 'adding one pins its version',
      items,
    })
  }
  groups.push(
    {
      label: 'In your stacks',
      sub: 'repository and this Console',
      items: inStacks.filter(match).map((name) => item(name)),
    },
    {
      label: 'Providers',
      sub: 'one per model family',
      items: providers.filter(match).map((name) => item(name)),
    },
    {
      label: 'Other iii workers',
      sub: 'seen in the templates',
      items: other.filter(match).map((name) => item(name)),
    },
  )
  const known = [...tplNames, ...inStacks, ...providers, ...other]
  if (q && !known.includes(q) && WORKER_NAME.test(q)) {
    const typed = item(q)
    const found = lookups[q]
    const waiting = !typed.alert && (!found || found.state === 'checking')
    const refused = !typed.alert && found?.state === 'missing'
    const said = typed.alert
      ? typed.tag
      : !found || found.state === 'checking'
        ? 'checking the registry…'
        : found.state === 'found'
          ? `found, ${found.version}`
          : found.state === 'missing'
            ? found.message
            : `couldn’t check the registry: ${found.message}`
    const off = typed.off || waiting || refused
    groups.push({
      label: 'From the iii registry',
      sub: 'checked as you type',
      items: [
        {
          ...typed,
          sub: `${PACKAGE}${q} · ${said}`,
          tag: off ? '' : 'add',
          alert: typed.alert || refused,
          off,
          add: off ? null : typed.add,
        },
      ],
    })
  }
  return groups.filter((group) => group.items.length)
}

export type Source = 'none' | 'template' | 'copy'

/** The footer: what Create would make, or why it can't yet. */
export function builderStatus({
  name,
  source,
  template,
  tpl,
  declared,
  verdicts,
}: {
  name: string
  source: Source
  template: string | null
  tpl: StackTemplate | null
  declared: Declared[]
  verdicts: Array<Verdict | null>
}): { text: string; blocked: boolean; alert: boolean } {
  const blockers = declared.filter(
    (_, index) => verdicts[index]?.tone === 'block',
  )
  if (blockers.length) {
    const names = blockers.map((entry) => entry.name)
    return {
      text: template
        ? `Can’t create it yet: ${names.join(' and ')} would be ignored by the ${templateId(template)} template.`
        : `Can’t create it yet: ${names.join(', ')} already arrives with another worker.`,
      blocked: true,
      alert: true,
    }
  }
  const waiting = (text: string) => ({ text, blocked: true, alert: false })
  const typing = declared.find(unfinished)
  if (typing)
    return waiting(
      `Type the ${typing.commit === '' ? 'commit' : 'version'} for ${typing.name}.`,
    )
  if (source === 'template' && !template) return waiting('Pick a template.')
  if (!template && !declared.length) return waiting('Add at least one worker.')
  if (!name.trim()) return waiting('Name the stack.')
  const text = template
    ? declared.length
      ? `Runs the ${templateId(template)} project with ${declared.length} pinned.`
      : `Runs the ${templateId(template)} project with every worker at the template’s version.`
    : `Runs the ${declared.length} workers declared, plus what they depend on.`
  return {
    text:
      tpl || !template
        ? text
        : `${text} Its workers couldn’t be read, so nothing was checked.`,
    blocked: false,
    alert: false,
  }
}
