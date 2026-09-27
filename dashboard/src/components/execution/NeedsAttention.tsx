import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@iii-dev/console-ui'
import { AlertCircle, Ellipsis, TriangleAlert } from 'lucide-react'
import { useState } from 'react'
import type { ScenarioMatrixItem } from '@/lib/scenario-matrix'
import './execution-page.css'

export type AttentionItem =
  | {
      kind: 'test'
      key: string
      scenarioId: string
      label: string
      reason: string
    }
  | { kind: 'warning'; key: string; text: string }

/** What needs a look first: every test that did not pass, with why, then
 *  what the execution recorded as a warning. */
export function attentionItems(
  items: ScenarioMatrixItem[],
  warnings: string[] = [],
): AttentionItem[] {
  const tests: AttentionItem[] = items
    .filter((item) =>
      ['failed', 'inconclusive'].includes(item.objective.status),
    )
    .map((item) => ({
      kind: 'test',
      key: item.key,
      scenarioId: item.scenarioId,
      label: item.objective.label,
      reason:
        item.reason ?? 'No reason recorded; open the test for its criteria.',
    }))
  return [
    ...tests,
    ...warnings.map((text, index) => ({
      kind: 'warning' as const,
      key: `warning-${index}`,
      text,
    })),
  ]
}

const LONG_TEXT = 160

function AttentionText({ text, id }: { text: string; id: string }) {
  const [open, setOpen] = useState(false)
  const long = text.length > LONG_TEXT || text.includes('\n')
  return (
    <>
      <span className="ep-attention-reason">
        {long && !open
          ? `${text.split('\n')[0].slice(0, LONG_TEXT).trimEnd()}…`
          : null}
        {!long ? text : null}
      </span>
      {long && open ? (
        <pre id={id} className="ep-attention-raw">
          {text}
        </pre>
      ) : null}
      {long ? (
        <button
          type="button"
          className="ds-button ds-button-quiet ds-button-compact"
          aria-expanded={open}
          aria-controls={id}
          onClick={() => setOpen(!open)}
        >
          {open ? 'Hide error' : 'Show error'}
        </button>
      ) : null}
    </>
  )
}

export function NeedsAttention({
  items,
  onShow,
  onRerun,
}: {
  items: AttentionItem[]
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
        {items.map((item) =>
          item.kind === 'test' ? (
            <li
              key={item.key}
              className="ep-attention-row"
              data-attention-test={item.scenarioId}
            >
              <AlertCircle
                size={16}
                aria-hidden="true"
                className="ep-alert-icon"
              />
              <span className="ep-attention-text">
                <span className="ep-mono ep-strong">{item.scenarioId}</span>{' '}
                <AttentionText text={item.reason} id={`${item.key}-raw`} />
              </span>
              <span className="ep-attention-actions">
                <button
                  type="button"
                  className="ds-button ds-button-quiet ds-button-compact"
                  onClick={() => onShow(item.key)}
                >
                  Show test
                </button>
                {onRerun ? (
                  <button
                    type="button"
                    className="ds-button ds-button-secondary ds-button-compact"
                    onClick={() => onRerun(item.scenarioId)}
                    aria-label={`Run ${item.scenarioId} again`}
                    data-attention-rerun={item.scenarioId}
                  >
                    Run again
                  </button>
                ) : null}
              </span>
            </li>
          ) : (
            <li key={item.key} className="ep-attention-row">
              <TriangleAlert
                size={16}
                aria-hidden="true"
                className="ep-warn-icon"
              />
              <span className="ep-attention-text">
                <AttentionText text={item.text} id={`${item.key}-raw`} />
              </span>
            </li>
          ),
        )}
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
