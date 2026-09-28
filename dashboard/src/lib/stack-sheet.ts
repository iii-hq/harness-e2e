/* The stack sheet: one screen to create, edit and view a stack, its Form
   and YAML tabs over one draft, the YAML text. The Form reads the draft
   through the runner's preview (stack-preview) and writes back by patching
   lines, so comments and every other key stay as written; what it can't
   patch safely is left to the YAML tab, field by field. The rules that hold
   Create or Save back are the builder's (lib/stack-builder.ts) and apply to
   whatever either tab wrote. */
import type {
  IiiRelease,
  IiiReleases,
  StackContainer,
  StackPreview,
} from '@/lib/dashboard-data-source'
import { formatDayLabel } from '@/lib/format'
import {
  builderStatus,
  containerLines,
  type Declared,
  pinLine,
  unfinished,
  type Verdict,
  yamlScalar,
} from '@/lib/stack-builder'

/** A draft the runner read, as opposed to one it refuses. */
export type Read = Exclude<StackPreview, { refused: string }>

export type Tab = 'form' | 'yaml'

export const IN_YAML = 'Change it in the YAML tab.'

/** `<id>@<revision>` as a stack writes it; no revision follows main. */
export function templateRef(
  template: { id: string; revision: string | null } | null,
) {
  if (!template) return null
  return template.revision ? `${template.id}@${template.revision}` : template.id
}

/** `template:` as written, split as the runner splits it. */
export function splitTemplate(template: string | null) {
  if (!template) return null
  const at = template.indexOf('@')
  return at < 0
    ? { id: template, revision: null }
    : { id: template.slice(0, at), revision: template.slice(at + 1) || null }
}

/** What a stack as listed already says, as a preview would. */
export function readOf(stack: {
  iii: string | null
  template: string | null
  containers: StackContainer[]
  warnings: string[]
}): Read {
  return {
    iii: stack.iii,
    template: splitTemplate(stack.template),
    containers: stack.containers,
    warnings: stack.warnings,
  }
}

/** What the form declares, as a container a preview lists. */
export function containerOf(entry: Declared): StackContainer {
  return {
    name: entry.name,
    worker: entry.worker,
    version: entry.commit === null ? entry.version : null,
    commit: entry.commit,
  }
}

/** A value on one line the form can rewrite: plain, or quoted, with an
 *  optional comment after it; not an anchor, an alias, a tag, a block
 *  scalar or a flow collection. */
const SIMPLE_VALUE =
  /^\s*(?:(?:[^\s#&*!|>{}[\],'"%@`][^#]*?|"(?:[^"\\]|\\.)*"|'(?:[^']|'')*')\s*)?(?:\s#.*)?$/

/** A top-level key's value and the comment after it. */
function splitLine(line: string) {
  const at = line.indexOf(':')
  const rest = line.slice(at + 1)
  const quoted = /^(\s*)("(?:[^"\\]|\\.)*"|'(?:[^']|'')*')(.*)$/.exec(rest)
  if (quoted) return { head: line.slice(0, at + 1), comment: quoted[3] }
  const comment = /\s#.*$/.exec(rest)
  return {
    head: line.slice(0, at + 1),
    comment: comment ? comment[0] : '',
  }
}

/** The lines where a top-level key is written. */
function keyLines(lines: string[], key: string) {
  const head = new RegExp(`^${key}\\s*:`)
  return lines.flatMap((line, index) => (head.test(line) ? [index] : []))
}

/** Whether a line opens more of the value above it (indented, not a
 *  comment or blank). */
function continues(line: string | undefined) {
  return line !== undefined && /^\s+\S/.test(line) && !/^\s*#/.test(line)
}

/** Why the form can't change a top-level scalar in place, or null. */
function scalarLock(lines: string[], key: string) {
  const at = keyLines(lines, key)
  if (at.length > 1) return `\`${key}\` is written more than once. ${IN_YAML}`
  if (!at.length) return null
  const value = lines[at[0]].slice(lines[at[0]].indexOf(':') + 1)
  if (!SIMPLE_VALUE.test(value) || continues(lines[at[0] + 1]))
    return `\`${key}\` is written in a way the form doesn’t rewrite (an anchor, a tag or more than one line). ${IN_YAML}`
  return null
}

/** `key: value` in place, the comment after it kept; removed when null;
 *  written where the form writes it when absent: iii before the first key,
 *  template after iii. */
export function setScalar(
  yaml: string,
  key: 'iii' | 'template',
  value: string | null,
) {
  const lines = yaml.split('\n')
  const [at] = keyLines(lines, key)
  if (at !== undefined) {
    if (value === null) lines.splice(at, 1)
    else {
      const { head, comment } = splitLine(lines[at])
      lines[at] = `${head} ${yamlScalar(value)}${comment}`
    }
    return lines.join('\n')
  }
  if (value === null) return yaml
  const written = `${key}: ${yamlScalar(value)}`
  const iii = keyLines(lines, 'iii')[0]
  const first = lines.findIndex((line) => /^[^\s#]/.test(line))
  const where =
    key === 'template' && iii !== undefined
      ? iii + 1
      : first >= 0
        ? first
        : lines.length
  lines.splice(where, 0, written)
  return lines.join('\n')
}

type Blocks = {
  start: number
  end: number
  /** Each container's lines: its header through its last value line. */
  blocks: Map<string, { from: number; to: number }>
}

const LAYOUT = `Its containers aren’t laid out as the form writes them (two spaces in, their keys four). ${IN_YAML}`

/** The containers mapping as the form writes it, or why it isn't. */
function containerBlocks(lines: string[]): Blocks | string {
  const heads = keyLines(lines, 'containers')
  if (heads.length !== 1) return LAYOUT
  const start = heads[0]
  if (!/^containers:\s*(\{\s*\})?\s*(#.*)?$/.test(lines[start])) return LAYOUT
  let end = start + 1
  while (end < lines.length && !/^[^\s#]/.test(lines[end])) end += 1
  const blocks = new Map<string, { from: number; to: number }>()
  let current: { from: number; to: number } | null = null
  for (let index = start + 1; index < end; index += 1) {
    const line = lines[index]
    if (line.trim() === '' || /^\s*#/.test(line)) continue
    if (/\t/.test(line)) return LAYOUT
    if (/(:\s+|\s-\s)[&*]|^\s*<<\s*:/.test(line))
      return `Its containers use anchors or aliases, which the form doesn’t rewrite. ${IN_YAML}`
    const head = /^ {2}([A-Za-z0-9._-]+):\s*(#.*)?$/.exec(line)
    if (head) {
      current = { from: index, to: index + 1 }
      blocks.set(head[1], current)
    } else if (current && /^ {4}/.test(line)) current.to = index + 1
    else return LAYOUT
  }
  return { start, end, blocks }
}

/** Why the form can't change a container's worker line or pin, or null:
 *  its worker, version or commit written as more than a value on its line. */
function pinLock(lines: string[], block: { from: number; to: number }) {
  for (let index = block.from + 1; index < block.to; index += 1) {
    const key = /^ {4}(worker|version|commit):(.*)$/.exec(lines[index])
    if (!key) continue
    if (
      !SIMPLE_VALUE.test(key[2]) ||
      /^ {5,}\S/.test(lines[index + 1] ?? '')
    )
      return `Its ${key[1]} is written in a way the form doesn’t rewrite. ${IN_YAML}`
  }
  return null
}

/** What the form may change of a draft, each with why not when it can't:
 *  the iii release, the template and the containers (added, removed, and
 *  each one's pin). */
export type Locks = {
  iii: string | null
  template: string | null
  containers: string | null
  pins: Record<string, string | null>
}

/** Everything locked, for one reason. */
export function lockedAll(reason: string): Locks {
  return { iii: reason, template: reason, containers: reason, pins: {} }
}

export function formLocks(yaml: string, read: Read): Locks {
  const lines = yaml.split('\n')
  const found = containerBlocks(lines)
  const names = read.containers.map((container) => container.name)
  const containers =
    typeof found === 'string'
      ? found
      : names.length !== found.blocks.size ||
          names.some((name) => !found.blocks.has(name))
        ? LAYOUT
        : null
  const pins: Record<string, string | null> = {}
  for (const name of names)
    pins[name] =
      containers ??
      (typeof found === 'string'
        ? found
        : pinLock(lines, found.blocks.get(name) ?? { from: 0, to: 0 }))
  return {
    iii: scalarLock(lines, 'iii'),
    template: scalarLock(lines, 'template'),
    containers,
    pins,
  }
}

/** The draft with the form's changes to its containers made to their lines:
 *  a removed container's lines go, a pin's line is rewritten in place, an
 *  added container is written after the last one. Every other line stays as
 *  written. Null when the containers aren't laid out as the form writes
 *  them (see formLocks). */
export function patchContainers(
  original: string,
  before: Declared[],
  after: Declared[],
): string | null {
  const lines = original.split('\n')
  const found = containerBlocks(lines)
  if (typeof found === 'string') return null
  const { start, blocks } = found
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

/** What holds Create or Save back, each said once, with the container whose
 *  lines it marks: a worker the template would ignore or another brings, a
 *  pin left empty, a revision that doesn't resolve, a YAML the runner
 *  refuses, no name. */
export type Block = { text: string; container?: string }

export function sheetBlocks({
  name,
  refused,
  revisionError,
  declared,
  verdicts,
}: {
  name: string
  refused: string | null
  revisionError: string | null
  declared: Declared[]
  verdicts: Array<Verdict | null>
}): Block[] {
  if (refused) return [{ text: `The runner refuses this YAML: ${refused}` }]
  const blocks: Block[] = []
  declared.forEach((entry, index) => {
    const verdict = verdicts[index]
    if (verdict?.tone === 'block')
      blocks.push({
        text: `${entry.name}: ${verdict.lead} ${verdict.text}`,
        container: entry.name,
      })
    else if (unfinished(entry))
      blocks.push({
        text: `${entry.name}: its ${entry.commit === '' ? 'commit' : 'version'} is empty.`,
        container: entry.name,
      })
  })
  if (revisionError) blocks.push({ text: revisionError })
  if (!name.trim()) blocks.push({ text: 'Name the stack.' })
  return blocks
}

/** The footer: what Create or Save would make, or why it can't yet. A
 *  draft typed in the YAML tab waits for the runner's read; one it refuses,
 *  or a template revision that doesn't resolve, holds it back. */
export function sheetStatus(
  args: Parameters<typeof builderStatus>[0] & {
    pending: boolean
    refused: string | null
    revisionError: string | null
  },
): { text: string; blocked: boolean; alert: boolean } {
  const verb = args.verb ?? 'create'
  if (args.pending)
    return { text: 'Reading the YAML…', blocked: true, alert: false }
  if (args.refused)
    return {
      text: `Can’t ${verb} it: the runner refuses this YAML.`,
      blocked: true,
      alert: true,
    }
  const status = builderStatus(args)
  if (args.revisionError && !status.alert)
    return {
      text: `Can’t ${verb} it yet: ${args.revisionError}`,
      blocked: true,
      alert: true,
    }
  return status
}

/** A line of the YAML tab's editor marked when its container holds Create
 *  or Save back. */
export function blockedLines(yaml: string, containers: Set<string>) {
  let inBad = false
  return yaml.split('\n').map((text) => {
    const head = /^ {2}([^\s:#]+):\s*(#.*)?$/.exec(text)
    if (head) inBad = containers.has(head[1])
    else if (!/^ {4}/.test(text) && text.trim() !== '') inBad = false
    return inBad && text.trim() !== ''
  })
}

/** `X.Y.Z[-pre]` compared as semver orders releases: by core, then a
 *  pre-release before its release, then its parts. Null when either isn't
 *  a version. */
export function compareVersions(left: string, right: string) {
  const parse = (value: string) => {
    const match = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+.*)?$/.exec(
      value.trim(),
    )
    return match
      ? {
          core: [match[1], match[2], match[3]].map(Number),
          pre: match[4]?.split('.') ?? null,
        }
      : null
  }
  const a = parse(left)
  const b = parse(right)
  if (!a || !b) return null
  for (let index = 0; index < 3; index += 1)
    if (a.core[index] !== b.core[index]) return a.core[index] - b.core[index]
  if (!a.pre || !b.pre) return a.pre ? -1 : b.pre ? 1 : 0
  for (let index = 0; index < Math.max(a.pre.length, b.pre.length); index += 1) {
    const x = a.pre[index]
    const y = b.pre[index]
    if (x === undefined || y === undefined) return x === undefined ? -1 : 1
    if (x === y) continue
    const numeric = /^\d+$/.test(x) && /^\d+$/.test(y)
    return numeric ? Number(x) - Number(y) : x < y ? -1 : 1
  }
  return 0
}

/** The release `iii:` installs: `latest` is the newest release candidate. */
export function iiiResolved(iii: string | null, releases: IiiReleases | null) {
  if (iii === 'latest') return releases?.latest_candidate ?? null
  return iii
}

/** A warning when the template needs a newer iii than the one picked. */
export function belowMinimum(
  iii: string | null,
  releases: IiiReleases | null,
  template: { id: string; min_iii_version?: string } | null,
) {
  const min = template?.min_iii_version
  const version = iiiResolved(iii, releases)
  if (!min || !version) return null
  const order = compareVersions(version, min)
  return order !== null && order < 0
    ? `The ${template.id} template needs iii ${min} or newer; ${iii === 'latest' ? `latest is ${version}` : `this is ${version}`}.`
    : null
}

/** When a release came out: `today`, `yesterday`, `Sep 21`. */
export function releasedOn(release: IiiRelease, now = new Date()) {
  if (!release.published_at) return ''
  const day = formatDayLabel(release.published_at, now)
  return day === 'Today' || day === 'Yesterday' ? day.toLowerCase() : day
}

/** What the iii button says: `latest · 0.24.3-rc.1` or the version. */
export function iiiLabel(iii: string | null, releases: IiiReleases | null) {
  if (iii === null) return 'not set'
  if (iii === 'latest' && releases?.latest_candidate)
    return `latest · ${releases.latest_candidate}`
  return iii
}

/** What the draft's iii is, as the menu offers it. */
export function iiiChoice(iii: string | null, releases: IiiReleases | null) {
  if (iii === 'latest') return 'latest'
  if (iii && releases?.releases.some((release) => release.version === iii))
    return `release:${iii}`
  return 'other'
}

/** Why a typed iii version can't be used, or null when it can. */
export function iiiRefusal(check: IiiReleases['checked']) {
  if (!check) return null
  if (!check.release)
    return `iii-hq/iii has no release iii/v${check.version}.`
  if (!check.release.cli)
    return `iii/v${check.version} publishes no iii CLI for x86_64 Linux, which a group installs.`
  return null
}
