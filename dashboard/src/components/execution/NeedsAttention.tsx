import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@iii-dev/console-ui'
import { AlertCircle, CircleMinus, Ellipsis, TriangleAlert } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { InvestigationAction } from '@/components/InvestigationAction'
import { itemScore, runCriteria } from '@/components/ScenarioMatrix'
import { ownReason, type ScenarioMatrixItem } from '@/lib/scenario-matrix'
import './execution-page.css'

/** Error: it did not run or broke a gate; warning: its result cannot be
 *  trusted as is; neutral: it ran and lost points. */
export type AttentionTone = 'error' | 'warning' | 'neutral'

export type AttentionItem =
  | {
      kind: 'test'
      key: string
      scenarioId: string
      subjectId: string
      tone: AttentionTone
      /** What happened, after the test's id: one line. */
      summary: string
      /** The whole error, when the line cuts it short. */
      detail: string | null
    }
  | { kind: 'warning'; key: string; text: string }

const LONG_TEXT = 160
const TONE_ORDER: Record<AttentionTone, number> = {
  error: 0,
  warning: 1,
  neutral: 2,
}

function firstLine(text: string) {
  const line = text.split('\n')[0].trim()
  return line.length > LONG_TEXT
    ? `${line.slice(0, LONG_TEXT).trimEnd()}…`
    : line
}

/** Why a criterion it lost says it lost: the first gate, else the first. */
function lostCriterion(item: ScenarioMatrixItem) {
  const lost = runCriteria(item.primaryRun).filter(
    (criterion) => criterion.awarded < criterion.possible,
  )
  const picked = lost.find((criterion) => criterion.gate) ?? lost[0]
  return picked ? picked.reason || picked.description || picked.id : null
}

/** What a test that did not pass says after its id, and how loud. */
function testAttention(
  item: ScenarioMatrixItem,
): Pick<
  Extract<AttentionItem, { kind: 'test' }>,
  'tone' | 'summary' | 'detail'
> | null {
  // The row names the test: an error that starts with its id says it once.
  const reason = ownReason(item)
  const score = itemScore(item)
  const outOf = score === null ? '' : ` (${score}/100)`
  const because = (text: string | null) => (text ? `: ${firstLine(text)}` : '.')
  const detail =
    reason && (reason.includes('\n') || reason.length > LONG_TEXT)
      ? reason
      : null
  switch (item.objective.status) {
    case 'not-run':
    case 'unavailable':
      return reason
        ? { tone: 'error', summary: `left no run${because(reason)}`, detail }
        : {
            tone: 'warning',
            summary: 'left no run, so it has no score.',
            detail: null,
          }
    case 'inconclusive':
      return {
        tone: 'warning',
        summary: `${score === null ? 'is inconclusive' : `scored ${score} but is inconclusive`}${because(reason ?? lostCriterion(item))}`,
        detail,
      }
    case 'incomplete':
      return {
        tone: 'neutral',
        summary: `didn’t complete the task${outOf}${because(lostCriterion(item) ?? reason)}`,
        detail: null,
      }
    case 'failed': {
      const gate = runCriteria(item.primaryRun).some(
        (criterion) => criterion.gate && criterion.awarded < criterion.possible,
      )
      return gate
        ? {
            tone: 'error',
            summary: `failed a hard gate${outOf}${because(lostCriterion(item))}`,
            detail: null,
          }
        : { tone: 'error', summary: `failed${because(reason)}`, detail }
    }
    default:
      return null
  }
}

/** What needs a look first: every test that did not pass or left no run,
 *  with why, errors first; then what the execution recorded as a warning,
 *  less an execution error that only repeats a test's. */
export function attentionItems(
  items: ScenarioMatrixItem[],
  warnings: string[] = [],
): AttentionItem[] {
  const tests = items
    .flatMap((item) => {
      const attention = testAttention(item)
      return attention
        ? [
            {
              kind: 'test' as const,
              key: item.key,
              scenarioId: item.scenarioId,
              subjectId: item.subjectId,
              ...attention,
            },
          ]
        : []
    })
    .sort((left, right) => TONE_ORDER[left.tone] - TONE_ORDER[right.tone])
  const repeats = (text: string) =>
    items.some(
      (item) =>
        item.reason &&
        text === `Execution error: ${item.scenarioId}: ${item.reason}`,
    )
  return [
    ...tests,
    ...warnings
      .filter((text) => !repeats(text))
      .map((text, index) => ({
        kind: 'warning' as const,
        key: `warning-${index}`,
        text,
      })),
  ]
}

const ICONS: Record<AttentionTone, ReactNode> = {
  error: <AlertCircle size={16} aria-hidden="true" className="ep-alert-icon" />,
  warning: (
    <TriangleAlert size={16} aria-hidden="true" className="ep-warn-icon" />
  ),
  neutral: (
    <CircleMinus size={16} aria-hidden="true" className="ep-neutral-icon" />
  ),
}

/** One line: its icon, what happened, and the actions; the whole error
 *  shows under the line once asked for. */
function AttentionRow({
  item,
  executionId,
  onShow,
  onRerun,
}: {
  item: AttentionItem
  executionId?: string
  onShow: (key: string) => void
  onRerun?: (scenarioId: string) => void
}) {
  const [open, setOpen] = useState(false)
  const test = item.kind === 'test' ? item : null
  const text = test
    ? test.summary
    : firstLine(item.kind === 'warning' ? item.text : '')
  const detail = test
    ? test.detail
    : item.kind === 'warning' && text !== item.text.trim()
      ? item.text
      : null
  const rawId = `${item.key}-raw`
  return (
    <li
      className="ep-attention-row"
      data-attention-test={test?.scenarioId}
      data-attention-tone={test?.tone}
    >
      {ICONS[test?.tone ?? 'warning']}
      <span className="ep-attention-text">
        {test ? (
          <>
            <span className="ep-mono ep-strong">{test.scenarioId}</span>{' '}
          </>
        ) : null}
        {text}
        {detail && open ? (
          <pre id={rawId} className="ep-attention-raw">
            {detail}
          </pre>
        ) : null}
      </span>
      <span className="ep-attention-actions">
        {detail ? (
          <button
            type="button"
            className="ds-button ds-button-quiet ds-button-compact"
            aria-expanded={open}
            aria-controls={rawId}
            onClick={() => setOpen(!open)}
          >
            {open ? 'Hide error' : 'Show error'}
          </button>
        ) : null}
        {test ? (
          <button
            type="button"
            className="ds-button ds-button-quiet ds-button-compact"
            onClick={() => onShow(test.key)}
          >
            Show test
          </button>
        ) : null}
        {test && executionId ? (
          <InvestigationAction
            label="Investigate"
            buttonClass="ds-button ds-button-quiet ds-button-compact"
            executionId={executionId}
            focus={{ scenarioId: test.scenarioId, subjectId: test.subjectId }}
          />
        ) : null}
        {test && onRerun ? (
          <button
            type="button"
            className="ds-button ds-button-secondary ds-button-compact"
            onClick={() => onRerun(test.scenarioId)}
            aria-label={`Run ${test.scenarioId} again`}
            data-attention-rerun={test.scenarioId}
          >
            Run again
          </button>
        ) : null}
      </span>
    </li>
  )
}

export function NeedsAttention({
  items,
  executionId,
  onShow,
  onRerun,
}: {
  items: AttentionItem[]
  /** Investigate each test in chat, when the host has one. */
  executionId?: string
  onShow: (key: string) => void
  /** Run one test again, offered on each test item once finished. */
  onRerun?: (scenarioId: string) => void
}) {
  if (items.length === 0) return null
  return (
    <section className="ep-attention" aria-labelledby="needs-attention">
      <div className="ep-section-head">
        <h2 id="needs-attention" className="ep-h2">
          Needs attention
        </h2>
        <span className="ep-count" data-attention-count>
          {items.length}
        </span>
      </div>
      <ul>
        {items.map((item) => (
          <AttentionRow
            key={item.key}
            item={item}
            executionId={executionId}
            onShow={onShow}
            onRerun={onRerun}
          />
        ))}
      </ul>
    </section>
  )
}

/** Record actions that are not everyday ones: rename, copy link, delete. */
export function ExecutionMoreMenu({
  onRename,
  onCopyLink,
  onCopyId,
  githubUrl,
  onDelete,
  deleteDisabled,
}: {
  onRename?: () => void
  onCopyLink: () => void
  onCopyId?: () => void
  /** The GitHub run page, for executions that ran there. */
  githubUrl?: string | null
  onDelete?: () => void
  deleteDisabled?: boolean
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="ds-button ds-button-quiet ds-button-default ep-more"
          aria-label="More actions"
        >
          <Ellipsis size={16} aria-hidden="true" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {onRename ? (
          <DropdownMenuItem onSelect={onRename}>Rename</DropdownMenuItem>
        ) : null}
        <DropdownMenuItem onSelect={onCopyLink}>Copy link</DropdownMenuItem>
        {onCopyId ? (
          <DropdownMenuItem onSelect={onCopyId}>
            Copy execution id
          </DropdownMenuItem>
        ) : null}
        {githubUrl ? (
          <DropdownMenuItem
            onSelect={() => window.open(githubUrl, '_blank', 'noopener')}
          >
            Open on GitHub
          </DropdownMenuItem>
        ) : null}
        {onDelete ? (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem disabled={deleteDisabled} onSelect={onDelete}>
              Delete…
            </DropdownMenuItem>
          </>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
