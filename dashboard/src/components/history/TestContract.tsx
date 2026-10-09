import { useState } from 'react'
import { plural } from '@/lib/format'
import type { TestSpec } from '@/lib/test-catalog'
import { copyText } from '@/lib/test-history'
import '@/pages/test-history.css'

/** `Budget per run: 256 turns · 65,536 output tokens · …`. */
export function budgetLine(execution: TestSpec['execution']) {
  const parts = [
    execution.max_turns
      ? `${execution.max_turns.toLocaleString('en-US')} turns`
      : null,
    execution.max_output_tokens
      ? `${execution.max_output_tokens.toLocaleString('en-US')} output tokens`
      : null,
    execution.max_total_tokens
      ? `${execution.max_total_tokens.toLocaleString('en-US')} tokens in total`
      : null,
    execution.stuck_timeout_seconds
      ? `stops after ${Math.round(execution.stuck_timeout_seconds / 60)} min without progress`
      : null,
  ].filter(Boolean)
  return parts.length ? `Budget per run: ${parts.join(' · ')}.` : ''
}

/** What the subject receives and how it is scored, side by side. */
export function TestContract({ spec }: { spec: TestSpec }) {
  const [open, setOpen] = useState(false)
  const [copied, setCopied] = useState<string | null>(null)
  const lines = spec.prompt.split('\n').length
  const points = spec.criteria.reduce((total, item) => total + item.weight, 0)
  const caption = `${plural(spec.criteria.length, 'criterion', 'criteria')} · ${points} points`
  return (
    <section className="th-two th-contract" aria-labelledby="th-prompt">
      <div className="th-contract-col">
        <div className="th-section-head">
          <h2 id="th-prompt" className="th-h2">
            Prompt
          </h2>
          <span className="th-faint">what the subject receives</span>
          <button
            type="button"
            className="th-act th-push"
            onClick={() =>
              void copyText(spec.prompt).then((ok) => {
                setCopied(ok ? 'Copied' : 'Not copied')
                window.setTimeout(() => setCopied(null), 2500)
              })
            }
          >
            {copied ?? 'Copy'}
          </button>
        </div>
        <pre className="th-prompt" data-open={open || undefined}>
          {spec.prompt}
        </pre>
        {lines > 10 ? (
          <button
            type="button"
            className="th-act th-flush"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
          >
            {open ? 'Show less' : `Show all ${lines} lines`}
          </button>
        ) : null}
      </div>
      <div className="th-contract-col">
        <div className="th-section-head">
          <h2 className="th-h2">How it’s scored</h2>
          <span className="th-faint">{caption}</span>
        </div>
        <ul className="th-criteria">
          {spec.criteria.map((criterion) => (
            <li key={criterion.id}>
              <span className="th-mono th-weight">{criterion.weight}</span>
              <span className="th-criterion">
                <span>{criterion.description}</span>
                <span className="th-mono th-faint">{criterion.id}</span>
                {criterion.gate ? (
                  <span className="th-gate">Blocks completion</span>
                ) : null}
              </span>
            </li>
          ))}
        </ul>
        <p className="th-faint th-empty-line">{budgetLine(spec.execution)}</p>
      </div>
    </section>
  )
}
