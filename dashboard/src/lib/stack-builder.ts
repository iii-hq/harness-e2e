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
import { plural } from '@/lib/format'

/** A container the stack declares. */
export type Declared = {
  name: string
  /** As written; null for a container without one, which stays so. */
  worker: string | null
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

export type Verdict = {
  tone: Tone
  lead: string
  text: string
  /** The worker to ask the registry about again. */
  retry?: string
}

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

/** What the executor adds to a group, whatever the stack declares
 *  (exact_stack_campaign.py project_scaffold). Without a template it never
 *  adds iii-directory: a run with an agent profile needs the stack to
 *  declare it or bring it with a declared worker, or it refuses to start. */
export function addedWhenItRuns(
  template: string | null,
  declared: Declared[],
  lookups: Lookups,
) {
  const host = hostOf('iii-directory', declared, lookups)
  const directory = template
    ? 'when a run uses an agent profile'
    : declared.some((entry) => packageName(entry.worker) === 'iii-directory')
      ? 'declared here, for runs with an agent profile'
      : host
        ? `comes with ${host}, for runs with an agent profile`
        : 'not added: runs with an agent profile need it declared here or brought by a declared worker'
  return [
    { name: 'harness-e2e', why: 'the runner, in every group' },
    { name: 'the model’s provider', why: 'for the model a run picks' },
    { name: 'canvas', why: 'for visual tests' },
    { name: 'iii-directory', why: directory },
  ]
}

/** Packages the executor adds itself where a run needs them; with a
 *  template, declaring one pins the version it adds. */
function addedByExecutor(name: string) {
  return (
    name === 'harness-e2e' ||
    name === 'canvas' ||
    name === 'iii-directory' ||
    name.startsWith('provider-')
  )
}

/** The packages a template declares; what a stack pins is compared by
 *  package, as the executor's template_packages does, never by container
 *  name. */
function templatePackages(tpl: StackTemplate | null) {
  return (tpl?.workers ?? []).flatMap((worker) => {
    const name = packageName(worker.worker)
    return name ? [name] : []
  })
}

/** A name the registry could hold: `^[a-z0-9][a-z0-9-]{0,63}$`. */
export const WORKER_NAME = /^[a-z0-9][a-z0-9-]{0,63}$/

const PACKAGE = 'package://'

export function packageName(worker: string | null) {
  return worker?.startsWith(PACKAGE) ? worker.slice(PACKAGE.length) : null
}

function isProvider(name: string) {
  return name.startsWith('provider-')
}

/** The id a stack's `template:` names, without its `@revision`. */
export function templateId(template: string | null) {
  return template ? template.split('@')[0] : null
}

/** Only `worker_not_found` says a worker does not exist; any other error
 *  is the registry not answering, asked again on Try again. */
export function lookupOf(resolution: WorkerResolution): Lookup {
  if ('error' in resolution)
    return resolution.error.code === 'worker_not_found'
      ? { state: 'missing', message: resolution.error.message }
      : { state: 'failed', message: resolution.error.message }
  return {
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
  const own = packageName(entry.worker)
  if (template) {
    if (!tpl) return null
    if (own && templatePackages(tpl).includes(own)) {
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
      return { tone: 'info', lead: `Pins the template’s ${own}.`, text }
    }
    if (own === 'harness-e2e')
      return {
        tone: 'info',
        lead: 'Pins the runner.',
        text: 'The executor adds harness-e2e to every group; this sets its version.',
      }
    if (own === 'canvas')
      return {
        tone: 'info',
        lead: 'Pins canvas for visual tests.',
        text: 'The template doesn’t bring it; the executor adds canvas to the groups that build a visual worker, at this version.',
      }
    if (own === 'iii-directory')
      return {
        tone: 'info',
        lead: 'Pins iii-directory for runs with an agent profile.',
        text: 'The template doesn’t bring it; the executor adds it when a run uses an agent profile, at this version.',
      }
    if (own && isProvider(own))
      return {
        tone: 'info',
        lead: `Pins it for runs that pick a ${own.replace('provider-', '')} model.`,
        text: 'The template doesn’t bring it; the executor adds the chosen model’s provider, at this version.',
      }
    return {
      tone: 'block',
      lead: 'Would be ignored.',
      text: `With a template, each group runs the ${tpl.id} project and this stack only pins versions of what it declares. ${entry.name} isn’t in it, so it would never start. Remove it, or start without a template.`,
    }
  }
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
      lead: 'Couldn’t reach the registry.',
      text: `${found.message} Nothing is blocked for it.`,
      retry: own,
    }
  if (found.dependencies.length)
    return {
      tone: 'info',
      lead: `Brings ${found.dependencies.join(', ')} with it.`,
      text: 'They start from its package; don’t declare them again.',
    }
  return null
}

/** Words YAML (1.1, as the executor reads it, or 1.2) takes for null or a
 *  boolean. */
const YAML_WORDS = /^(null|~|true|false|yes|no|on|off|y|n)$/i

/** A value written so YAML reads it back as the same text: plain only when
 *  it starts with a letter, holds `A-Za-z0-9._+/:@-` alone and is no word
 *  YAML reads as something else; anything else (a number, a date, `@1.2`,
 *  `a: b`, `1.2.3 #rc`) is quoted. */
export function yamlScalar(value: string) {
  const plain =
    /^[A-Za-z][A-Za-z0-9._+/:@-]*$/.test(value) &&
    !value.endsWith(':') &&
    !YAML_WORDS.test(value)
  return plain ? value : JSON.stringify(value)
}

/** The stack's YAML, as the form writes it. */
export function yamlOf(
  iii: string,
  template: string | null,
  declared: Declared[],
) {
  const out = [`iii: ${yamlScalar(iii)}`]
  if (template) out.push(`template: ${yamlScalar(template)}`)
  out.push('', declared.length ? 'containers:' : 'containers: {}')
  for (const entry of declared)
    out.push(`  ${yamlScalar(entry.name)}:`, ...containerLines(entry))
  out.push('', 'startup_timeout: 5m', 'stop_timeout: 30s')
  return out
}

/** A container's fields as the form writes them: its worker and its pin. */
function containerLines(entry: Declared) {
  return [
    ...(entry.worker === null
      ? []
      : [`    worker: ${yamlScalar(entry.worker)}`]),
    pinLine(entry),
  ]
}

function pinLine(entry: Declared) {
  return entry.commit !== null
    ? `    commit: ${JSON.stringify(entry.commit)}`
    : `    version: ${yamlScalar(entry.version || 'latest')}`
}

/** Whether the form still holds a stack's containers as they were. */
export function sameDeclared(left: Declared[], right: Declared[]) {
  return (
    left.length === right.length &&
    left.every((entry, index) => {
      const other = right[index]
      return (
        entry.name === other.name &&
        entry.worker === other.worker &&
        entry.version === other.version &&
        entry.commit === other.commit
      )
    })
  )
}

/** A copied stack's YAML with the form's changes made to the lines of the
 *  containers they touch, and to `template:`: a removed container's lines
 *  go, a pin's line is rewritten, an added container is written after the
 *  last one. Every other line (comments, other keys) stays as written.
 *  Null when the stack is not laid out as the form writes one (containers
 *  two spaces in, their keys four), so it can't be changed line by line. */
export function patchYaml(
  original: string,
  before: Declared[],
  after: Declared[],
  template: string | null,
  beforeTemplate: string | null,
): string | null {
  const lines = original.split('\n')
  if (template !== beforeTemplate) {
    const at = lines.findIndex((line) => /^template:/.test(line))
    const written =
      template === null ? null : `template: ${yamlScalar(template)}`
    if (at >= 0 && written === null) lines.splice(at, 1)
    else if (at >= 0 && written) lines[at] = written
    else if (written)
      lines.splice(
        lines.findIndex((line) => /^iii:/.test(line)) + 1,
        0,
        written,
      )
  }
  const start = lines.findIndex((line) =>
    /^containers:\s*(\{\s*\})?\s*(#.*)?$/.test(line),
  )
  if (start < 0) return null
  let end = start + 1
  while (end < lines.length && !/^[^\s#]/.test(lines[end])) end += 1
  // Each container's lines: its header and through its last value line.
  const blocks = new Map<string, { from: number; to: number }>()
  let current: { from: number; to: number } | null = null
  for (let index = start + 1; index < end; index += 1) {
    const line = lines[index]
    if (line.trim() === '' || /^\s*#/.test(line)) continue
    const head = /^ {2}([A-Za-z0-9._-]+):\s*(#.*)?$/.exec(line)
    if (head) {
      current = { from: index, to: index + 1 }
      blocks.set(head[1], current)
    } else if (current && /^ {4}/.test(line)) current.to = index + 1
    else return null
  }
  if (before.some((entry) => !blocks.has(entry.name))) return null
  const was = new Map(before.map((entry) => [entry.name, entry]))
  const now = new Map(after.map((entry) => [entry.name, entry]))
  const added = after.filter((entry) => !was.has(entry.name))
  const tail = Math.max(
    start + 1,
    ...[...blocks.values()].map((block) => block.to),
  )
  const out: string[] = []
  for (let index = 0; index < lines.length; index += 1) {
    if (index === tail)
      for (const entry of added)
        out.push(`  ${yamlScalar(entry.name)}:`, ...containerLines(entry))
    if (index === start) {
      // `containers: {}` opens when one is added and closes when none is left.
      const empty = /\{\s*\}/.test(lines[index])
      out.push(
        empty === !after.length
          ? lines[index]
          : after.length
            ? 'containers:'
            : 'containers: {}',
      )
      continue
    }
    const name = [...blocks].find(([, block]) => block.from === index)?.[0]
    if (name === undefined) {
      out.push(lines[index])
      continue
    }
    const { to } = blocks.get(name) ?? { to: index + 1 }
    const entry = now.get(name)
    const body = lines.slice(index, to)
    index = to - 1
    if (!entry) continue
    const old = was.get(name)
    if (old?.version === entry.version && old.commit === entry.commit) {
      out.push(...body)
      continue
    }
    // The pin's line in place of the old one, else after the worker.
    const pins = body.flatMap((line, at) =>
      /^ {4}(version|commit):/.test(line) ? [at] : [],
    )
    const worker = body.findIndex((line) => /^ {4}worker:/.test(line))
    const at = pins[0] ?? (worker >= 0 ? worker + 1 : 1)
    const kept = body.filter((_, line) => !pins.includes(line))
    kept.splice(at - pins.filter((line) => line < at).length, 0, pinLine(entry))
    out.push(...kept)
  }
  if (tail >= lines.length)
    for (const entry of added)
      out.push(`  ${yamlScalar(entry.name)}:`, ...containerLines(entry))
  return out.join('\n')
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
    worker: container.worker,
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

export type PickGroup = {
  label: string
  sub: string
  items: PickItem[]
  /** A typed name the registry did not answer about, to ask again. */
  retry?: string
}

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
  // The template's own workers, by package: a path:// one is the project's
  // code, not something a stack pins.
  const tplWorkers = (tpl?.workers ?? []).filter((worker) =>
    packageName(worker.worker),
  )
  const tplNames = templatePackages(tpl)
  const has = new Set(
    declared.map((entry) => packageName(entry.worker) ?? entry.name),
  )
  const item = (name: string, worker = `${PACKAGE}${name}`): PickItem => {
    const own = packageName(worker) ?? name
    const isAdded = has.has(own)
    const inTpl = tplNames.includes(own)
    const ignored = Boolean(template) && !inTpl && !addedByExecutor(own)
    const host = template ? null : hostOf(own, declared, lookups)
    const off = isAdded || ignored || Boolean(host)
    const tag = isAdded
      ? 'added'
      : ignored
        ? 'not in the template, ignored'
        : host
          ? `arrives with ${host}`
          : inTpl
            ? 'pins it'
            : own === 'harness-e2e'
              ? 'the runner, always added'
              : !template
                ? ''
                : isProvider(own)
                  ? 'added with its model'
                  : own === 'canvas'
                    ? 'added for visual tests'
                    : own === 'iii-directory'
                      ? 'added with an agent profile'
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
            : `couldn’t reach the registry: ${found.message}`
    const off = typed.off || waiting || refused
    groups.push({
      label: 'From the iii registry',
      sub: 'checked as you type',
      retry: found?.state === 'failed' ? q : undefined,
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
    : `Runs the ${plural(declared.length, 'worker')} declared, plus what they depend on.`
  return {
    text:
      tpl || !template
        ? text
        : `${text} Its workers couldn’t be read, so nothing was checked.`,
    blocked: false,
    alert: false,
  }
}
