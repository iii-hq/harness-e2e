import { ChevronDown, ChevronUp, Copy } from 'lucide-react'
import { useCallback, useState } from 'react'
import {
  buttonClassName,
  type OperationalStatus,
  Panel,
  StatusBadge,
} from '@/design-system'
import type { TestCriterion, TestSpec } from '@/lib/test-catalog'

/** Whether the panel starts open. One preference for the whole dashboard: a
 *  reader who knows their tests collapses it once, not once per test. */
export const ABOUT_PANEL_STORAGE_KEY = 'harness-e2e:about-test-open'

export function storedAboutPanelOpen(
  storage: Pick<Storage, 'getItem'>,
): boolean | null {
  try {
    const value = storage.getItem(ABOUT_PANEL_STORAGE_KEY)
    if (value === 'open') return true
    if (value === 'collapsed') return false
    return null
  } catch {
    // Storage can be unavailable in private or embedded contexts.
    return null
  }
}

function rememberAboutPanelOpen(open: boolean) {
  try {
    localStorage.setItem(ABOUT_PANEL_STORAGE_KEY, open ? 'open' : 'collapsed')
  } catch {
    // Storage can be unavailable in private or embedded contexts.
  }
}

/** Total possible weight, so a partial contract does not read as a percentage
 *  of 100 it never reached. */
export function totalWeight(criteria: TestCriterion[]): number {
  return criteria.reduce((total, criterion) => total + criterion.weight, 0)
}

export function hardGateCount(criteria: TestCriterion[]): number {
  return criteria.filter((criterion) => criterion.policy === 'hard_gate').length
}

export function criteriaCaption(criteria: TestCriterion[]): string {
  const gates = hardGateCount(criteria)
  const judged = criteria.some((criterion) => criterion.source === 'judge')
  return [
    `${criteria.length} ${criteria.length === 1 ? 'criterion' : 'criteria'}`,
    gates > 0 ? `${gates} hard ${gates === 1 ? 'gate' : 'gates'}` : null,
    judged ? 'judge-scored' : 'deterministic, no judge model',
  ]
    .filter(Boolean)
    .join(' · ')
}

function policyLabel(criterion: TestCriterion): string {
  return criterion.policy === 'hard_gate' ? 'hard gate' : 'score only'
}

/** Scenario prose marks identifiers with backticks, the way the Rust doc
 *  comments it is written beside do. Render them as code, not as literal
 *  backticks — this is the one piece of markup the panel understands. */
export function withInlineCode(text: string) {
  return text.split(/`([^`]+)`/).map((part, index) =>
    index % 2 === 0 ? (
      part
    ) : (
      // biome-ignore lint/suspicious/noArrayIndexKey: split order is the identity
      <code className="font-mono text-[0.8125rem]" key={index}>
        {part}
      </code>
    ),
  )
}

function formatBudget(execution: TestSpec['execution']): string {
  return [
    `${execution.max_turns} turns`,
    execution.max_output_tokens
      ? `${execution.max_output_tokens.toLocaleString('en-US')} output`
      : null,
    execution.max_total_tokens
      ? `${execution.max_total_tokens.toLocaleString('en-US')} total`
      : null,
    `${execution.stuck_timeout_seconds}s stuck`,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** The scored contract. `outcomes` pairs each criterion with what actually
 *  happened to it; without it the list reads as the requirement alone. */
export function TestCriteriaList({
  criteria,
  outcomes,
  headingId,
}: {
  criteria: TestCriterion[]
  outcomes?: Map<string, { label: string; status: OperationalStatus }>
  headingId?: string
}) {
  return (
    <div className="grid gap-0">
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <span className="ds-label" id={headingId}>
          {outcomes ? 'what this test required' : 'how it is scored'}
        </span>
        <span className="font-mono text-label text-ink-muted">
          {criteriaCaption(criteria)}
        </span>
      </div>
      <ul className="m-0 grid list-none gap-3.5 p-0 pt-3.5">
        {criteria.map((criterion) => {
          const outcome = outcomes?.get(criterion.id)
          const advisory = criterion.policy !== 'hard_gate'
          return (
            <li
              className="grid grid-cols-[2.5rem_minmax(0,1fr)] items-start gap-x-3.5 gap-y-1 @[720px]:grid-cols-[2.5rem_minmax(0,1fr)_auto]"
              data-criterion={criterion.id}
              key={criterion.id}
            >
              <span
                className={`text-right font-mono text-[0.9375rem] font-semibold tabular-nums ${advisory ? 'text-ink-soft' : 'text-ink'}`}
              >
                {criterion.weight}
              </span>
              <div className="min-w-0">
                <div
                  className={`font-mono text-xs ${advisory ? 'text-ink-soft' : 'text-ink'}`}
                >
                  {criterion.id}
                </div>
                <p className="m-0 mt-0.5 text-xs leading-5 text-ink-soft">
                  {withInlineCode(criterion.description)}
                </p>
              </div>
              <span className="col-start-2 flex flex-wrap items-center gap-2 @[720px]:col-start-3 @[720px]:justify-end">
                <span
                  className={`font-mono text-label ${advisory ? 'text-ink-muted' : 'text-ink'}`}
                >
                  {policyLabel(criterion)}
                </span>
                {outcome ? (
                  <StatusBadge label={outcome.label} status={outcome.status} />
                ) : null}
              </span>
            </li>
          )
        })}
      </ul>
    </div>
  )
}

function EnvironmentGrid({ spec }: { spec: TestSpec }) {
  const rows: Array<[string, string]> = [
    ['budget', formatBudget(spec.execution)],
    spec.denied_functions.length > 0
      ? ['denied', spec.denied_functions.join(' · ')]
      : null,
  ].filter((row): row is [string, string] => row !== null)

  return (
    <div className="grid content-start gap-0">
      <span className="ds-label">limits this run answers to</span>
      <dl className="m-0 grid grid-cols-[5.25rem_minmax(0,1fr)] content-start gap-x-3 gap-y-1.5 pt-2.5">
        {rows.map(([label, value]) => (
          <div className="contents" key={label}>
            <dt className="font-mono text-label text-ink-muted">{label}</dt>
            <dd className="m-0 font-mono text-xs leading-[18px] text-ink-soft">
              {value}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  )
}

function PromptBlock({ prompt, testId }: { prompt: string; testId: string }) {
  const [expanded, setExpanded] = useState(false)
  const [copied, setCopied] = useState(false)
  const promptId = `about-${testId}-prompt`

  const copy = useCallback(() => {
    void navigator.clipboard?.writeText(prompt).then(() => {
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1500)
    })
  }, [prompt])

  // Without a fade the clip is invisible — a prompt that happens to break on a
  // paragraph reads as complete. The length says there is more.
  const lineCount = prompt.split('\n').length

  return (
    <div className="grid gap-2">
      {/* The raised block is the prompt itself; the controls sit outside it on
          the panel fill, so nothing needs a rule to separate them. */}
      <div className="overflow-hidden rounded-[6px] bg-panel">
        <div className="flex flex-wrap items-center justify-between gap-3 px-4 pt-2.5">
          <span className="ds-label">prompt handed to the subject</span>
          <button
            className={buttonClassName({ variant: 'quiet', size: 'compact' })}
            onClick={copy}
            title="Copy the prompt the subject receives"
            type="button"
          >
            <Copy aria-hidden="true" size={13} />
            {copied ? 'copied' : 'copy prompt'}
          </button>
        </div>
        <pre
          className={`m-0 overflow-hidden whitespace-pre-wrap break-words px-4 pt-2 pb-3.5 font-mono text-xs leading-5 text-ink-soft ${expanded ? '' : 'max-h-[8.25rem]'}`}
          id={promptId}
        >
          {prompt}
        </pre>
      </div>
      <div className="flex justify-center">
        <button
          aria-controls={promptId}
          aria-expanded={expanded}
          className={buttonClassName({ variant: 'quiet', size: 'compact' })}
          onClick={() => setExpanded((open) => !open)}
          type="button"
        >
          {expanded ? 'hide prompt' : `show full prompt · ${lineCount} lines`}
          {expanded ? (
            <ChevronUp aria-hidden="true" size={13} />
          ) : (
            <ChevronDown aria-hidden="true" size={13} />
          )}
        </button>
      </div>
    </div>
  )
}

/** What the subject is asked to do, how it is scored, and the limits it runs
 *  under — the three things the test page never said (audit TH-20). */
export function AboutTestPanel({
  spec,
  testId,
  layout = 'columns',
  className,
}: {
  spec: TestSpec
  testId: string
  /** `columns` splits scoring and limits side by side; `stacked` is for the
   *  execution dialog and narrow containers. */
  layout?: 'columns' | 'stacked'
  className?: string
}) {
  // Read synchronously so the panel never flashes open before collapsing, and
  // guard the global: this component also renders outside a browser.
  const [open, setOpen] = useState<boolean>(() =>
    typeof localStorage === 'undefined'
      ? true
      : (storedAboutPanelOpen(localStorage) ?? true),
  )
  const toggle = () => {
    const next = !open
    setOpen(next)
    rememberAboutPanelOpen(next)
  }

  const headingId = `about-${testId}-heading`
  return (
    <Panel
      aria-labelledby={headingId}
      as="section"
      className={className}
      data-about-test={testId}
      padding="compact"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <h2 className="m-0 text-sm font-semibold text-ink" id={headingId}>
          about this test
        </h2>
        <button
          aria-expanded={open}
          className={buttonClassName({ variant: 'quiet', size: 'compact' })}
          onClick={toggle}
          type="button"
        >
          {open ? 'collapse' : 'expand'}
          {open ? (
            <ChevronUp aria-hidden="true" size={13} />
          ) : (
            <ChevronDown aria-hidden="true" size={13} />
          )}
        </button>
      </div>

      {open ? (
        <div className="mt-3.5 grid gap-4">
          {spec.summary ? (
            <p className="m-0 max-w-[96ch] text-sm leading-[22px] text-pretty text-ink">
              {withInlineCode(spec.summary)}
            </p>
          ) : null}
          <PromptBlock prompt={spec.prompt} testId={testId} />
          {spec.criteria.length > 0 ? (
            <div
              className={
                layout === 'columns'
                  ? 'grid gap-x-8 gap-y-4 @[900px]:grid-cols-[minmax(0,1fr)_minmax(0,20rem)]'
                  : 'grid gap-4'
              }
            >
              <TestCriteriaList criteria={spec.criteria} />
              <EnvironmentGrid spec={spec} />
            </div>
          ) : (
            <EnvironmentGrid spec={spec} />
          )}
        </div>
      ) : (
        <p className="m-0 mt-1.5 font-mono text-xs leading-5 text-ink-soft">
          {spec.summary ??
            `${criteriaCaption(spec.criteria)} · ${formatBudget(spec.execution)}`}
        </p>
      )}
    </Panel>
  )
}
