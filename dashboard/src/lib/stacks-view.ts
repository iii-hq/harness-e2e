/* What the Stacks page says about a stack, as the redesign canvas draws it
   (Stacks.dc.html): its pins, how it differs from the repository's default,
   its YAML's size and lines. Everything here reads what the runner answered;
   the runner alone decides what a stack declares and what it warns about. */
import type { Stack, StackContainer } from '@/lib/dashboard-data-source'
import { formatDateTime, plural } from '@/lib/format'

/** The stack a campaign measures unless it names another. */
export const BASE_STACK = 'default'

export function stacksSummary(stacks: Stack[]) {
  const repository = stacks.filter((stack) => stack.source !== 'local').length
  const local = stacks.length - repository
  return `${repository} in the repository, read-only · ${
    local ? `${local} in this Console` : 'none in this Console'
  }. A stack is where a suite runs: the iii release, an optional iii-hq/templates project and the workers Compose starts. Warnings never block a run.`
}

/** The file of a repository stack. */
export function stackFile(stack: Stack) {
  return `stacks/${stack.id}.yaml`
}

/** The line under a stack's name: its file, or its id and when it was saved. */
export function stackSub(stack: Stack, now = new Date()) {
  if (stack.source !== 'local') return stackFile(stack)
  return stack.updated_at
    ? `${stack.id} · saved ${formatDateTime(stack.updated_at, now)}`
    : stack.id
}

/** What a container pins: a commit, a version, or nothing. */
export function pinOf(container: StackContainer) {
  if (container.commit) return `commit ${container.commit.slice(0, 12)}`
  return container.version ?? 'no version'
}

/** A worker the runner warns about, as it does: anything but package://
 *  (a path on this machine, another scheme, or none at all). */
export function workerWarns(container: StackContainer) {
  return !container.worker?.startsWith('package://')
}

/** What a stack declares, in one line: `iii latest · template harness · 5
 *  workers · 1 warning`. */
export function stackDeclares(stack: Stack) {
  return [
    `iii ${stack.iii ?? '—'}`,
    stack.template ? `template ${stack.template}` : null,
    plural(stack.containers.length, 'worker'),
    stack.warnings.length ? plural(stack.warnings.length, 'warning') : null,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** How a stack differs from the repository's default; empty without one. */
export function stackDiff(stack: Stack, base: Stack | undefined) {
  if (!base) return ''
  if (stack.id === base.id)
    return 'The stack a campaign measures unless it names another.'
  const said: string[] = []
  if ((stack.template ?? '') !== (base.template ?? ''))
    said.push(
      stack.template ? `adds template ${stack.template}` : 'no template',
    )
  if (stack.iii !== base.iii) said.push(`iii ${stack.iii ?? 'unset'}`)
  const was = new Map(base.containers.map((entry) => [entry.name, entry]))
  const changed = stack.containers
    .filter((entry) => {
      const before = was.get(entry.name)
      return (
        !before ||
        before.worker !== entry.worker ||
        before.version !== entry.version ||
        before.commit !== entry.commit
      )
    })
    .map((entry) => entry.name)
  const kept = new Set(stack.containers.map((entry) => entry.name))
  const dropped = base.containers
    .filter((entry) => !kept.has(entry.name))
    .map((entry) => entry.name)
  if (changed.length) said.push(`changes ${changed.join(', ')}`)
  if (dropped.length) said.push(`drops ${dropped.join(', ')}`)
  return said.length
    ? `Against ${base.label}: ${said.join(' · ')}.`
    : `Same as ${base.label}.`
}

export function warningsTitle(count: number) {
  return count ? plural(count, 'warning') : 'No warnings'
}

/** The YAML's lines as shown, without the last newline. */
export function yamlLines(yaml: string) {
  return yaml.replace(/\n$/, '').split('\n')
}

/** `54 lines · 2.0 KB`. */
export function yamlMeta(yaml: string) {
  const bytes = new TextEncoder().encode(yaml).length
  return `${plural(yamlLines(yaml).length, 'line')} · ${(bytes / 1024).toFixed(1)} KB`
}

/** A line split for the read-only view: a mapping key is drawn apart from
 *  its value; a comment is faint. */
export function yamlLine(line: string): {
  key: string
  rest: string
  comment: boolean
} {
  if (line.trim().startsWith('#')) return { key: '', rest: line, comment: true }
  const key = /^(\s*[^:#\s][^:#]*:)(.*)$/.exec(line)
  return key
    ? { key: key[1], rest: key[2], comment: false }
    : { key: '', rest: line, comment: false }
}

export function iiiHint(iii: string | null) {
  if (!iii) return ''
  return iii === 'latest'
    ? 'The newest release candidate of iii-hq/iii, resolved when an execution starts.'
    : 'This release, every run.'
}

export function templateHint(template: string) {
  return template.includes('@')
    ? 'The iii-hq/templates project the groups start from, at that revision.'
    : 'The iii-hq/templates project the groups start from, as main has it when the execution starts.'
}

export function templateUrl(template: string) {
  return `https://github.com/iii-hq/templates/tree/main/iii/${encodeURIComponent(template.split('@')[0])}`
}

/** A saved stack's status in its editor. */
export function savedStatus(stack: Stack) {
  const count = stack.warnings.length
  return count ? `Saved with ${plural(count, 'warning')}.` : 'Saved.'
}
